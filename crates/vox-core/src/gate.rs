//! 激活门：决定采集到的音频哪些块要上传。
//!
//! 两种门：
//!
//! * `Manual`：由外部状态驱动（开关 / 按住说话模式下的按键）。激活时放行并先
//!   冲出 preroll 缓冲；从激活转为未激活的瞬间发一段 `tail_ms` 的零样本，触发
//!   服务端断句；未激活时样本进 preroll 缓冲，保证下次激活时起音不被削掉。
//! * `Level`：单个 RMS 阈值门（开关模式的电平判断与听人说话）。`threshold <= 0`
//!   时全部放行；否则 RMS 超过阈值放行，低于阈值时先走 tail 再收尾，静音期进
//!   preroll 缓冲。
//!
//! 门控参数（threshold / tail_ms / preroll_ms）与状态机逻辑是上一版调好的资产，
//! 逐行移植，不要重新推导。

/// 门的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateKind {
    Manual,
    Level,
}

/// 一种激活门的完整参数。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GateConfig {
    pub kind: GateKind,
    /// Level 门使用；<= 0 表示全部放行。
    pub threshold: f32,
    pub tail_ms: u32,
    pub preroll_ms: u32,
}

impl GateConfig {
    /// 开关 / 按住说话共用的手动门（原按住说话预设的调好参数）。
    pub const MANUAL: Self = Self {
        kind: GateKind::Manual,
        threshold: 0.012,
        tail_ms: 150,
        preroll_ms: 100,
    };

    /// 听人说话的电平门不设阈值（`level(0.0)` = 无条件放行）——环回的数字源
    /// 有假底噪，电平门会被底噪骗"正在说话"，与其烧 token 不如不设门。
    /// 要一个"带阈值但预设好参数"的电平门，直接调 `level(threshold)`。
    /// 开关模式的电平门：单个可调阈值，tail/preroll 用调好的默认值。
    pub fn level(threshold: f32) -> Self {
        Self {
            kind: GateKind::Level,
            threshold: threshold.max(0.0),
            tail_ms: 600,
            preroll_ms: 200,
        }
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self::MANUAL
    }
}

/// 门当前处在状态机的哪一步，用于驱动 UI 的实时电平指示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Empty,
    /// 手动门：按键按住，放行中。
    Manual,
    /// 手动门：刚松开，这一拍发的是断句用的静音尾。
    Released,
    /// 手动门：没按住，样本进 preroll 缓冲。
    Waiting,
    /// 电平门：阈值 <= 0，无条件放行。
    Always,
    /// 电平门：RMS 过阈值，放行中。
    Speech,
    /// 电平门：掉到阈值下但还在 tail 窗口内，继续放行。
    Tail,
    /// 电平门：tail 窗口走完，这一段结束。
    TailEnd,
    /// 电平门：静音，样本进 preroll 缓冲。
    Silence,
}

/// 一次 `process` 的状态快照。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GateStatus {
    pub kind: GateKind,
    pub state: GateState,
    pub rms: f32,
    pub active: bool,
    /// 这一拍是否是一段语音的收尾（可用于提示服务端已断句）。
    pub ended: bool,
}

/// 把 f32 样本块流过滤成要上传的部分。
pub struct ActivationGate {
    config: GateConfig,
    sample_rate: u32,
    active: bool,
    silent_samples: usize,
    prebuffer: std::collections::VecDeque<Vec<f32>>,
    prebuffer_samples: usize,
    external_active: bool,
}

impl ActivationGate {
    pub fn new(config: GateConfig, sample_rate: u32) -> Self {
        Self {
            config,
            sample_rate,
            active: false,
            silent_samples: 0,
            prebuffer: std::collections::VecDeque::new(),
            prebuffer_samples: 0,
            external_active: false,
        }
    }

    pub fn config(&self) -> GateConfig {
        self.config
    }

    /// 手动门的外部驱动（按键按下/松开、手动纠正）。
    pub fn set_external_active(&mut self, active: bool) {
        self.external_active = active;
    }

    pub fn external_active(&self) -> bool {
        self.external_active
    }

    /// 热切换门参数；内部状态复位，但保留外部激活状态。
    pub fn set_config(&mut self, config: GateConfig) {
        let external_active = self.external_active;
        self.config = config;
        self.reset();
        self.external_active = external_active;
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.silent_samples = 0;
        self.prebuffer.clear();
        self.prebuffer_samples = 0;
        self.external_active = false;
    }

    /// 过滤一个样本块，返回要上传的块（可能为空）加当前状态。
    pub fn process(&mut self, samples: &[f32]) -> (Vec<Vec<f32>>, GateStatus) {
        if samples.is_empty() {
            let active = self.active;
            return (
                Vec::new(),
                self.status(GateState::Empty, 0.0, active, false),
            );
        }

        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();

        match self.config.kind {
            GateKind::Manual => self.process_manual(samples, rms),
            GateKind::Level => self.process_level(samples, rms),
        }
    }

    fn process_manual(&mut self, samples: &[f32], rms: f32) -> (Vec<Vec<f32>>, GateStatus) {
        if self.external_active {
            let mut accepted = Vec::new();
            if !self.active {
                accepted.extend(self.flush_prebuffer());
                self.active = true;
            }
            accepted.push(samples.to_vec());
            return (accepted, self.status(GateState::Manual, rms, true, false));
        }
        if self.active {
            // 刚从激活转为未激活：补一段零样本触发服务端断句。
            self.active = false;
            let tail_samples =
                (self.sample_rate as u64 * self.config.tail_ms as u64 / 1000).max(1) as usize;
            let tail = vec![0.0f32; tail_samples];
            return (
                vec![tail],
                self.status(GateState::Released, rms, false, true),
            );
        }
        self.push_prebuffer(samples);
        (
            Vec::new(),
            self.status(GateState::Waiting, rms, false, false),
        )
    }

    fn process_level(&mut self, samples: &[f32], rms: f32) -> (Vec<Vec<f32>>, GateStatus) {
        if self.config.threshold <= 0.0 {
            return (
                vec![samples.to_vec()],
                self.status(GateState::Always, rms, true, false),
            );
        }

        if rms >= self.config.threshold {
            let mut accepted = Vec::new();
            if !self.active {
                accepted.extend(self.flush_prebuffer());
                self.active = true;
            }
            accepted.push(samples.to_vec());
            self.silent_samples = 0;
            return (accepted, self.status(GateState::Speech, rms, true, false));
        }

        if self.active {
            self.silent_samples += samples.len();
            let tail_samples =
                (self.sample_rate as u64 * self.config.tail_ms as u64 / 1000) as usize;
            let ended = self.silent_samples >= tail_samples;
            if ended {
                self.active = false;
                self.silent_samples = 0;
            }
            let state = if ended {
                GateState::TailEnd
            } else {
                GateState::Tail
            };
            return (
                vec![samples.to_vec()],
                self.status(state, rms, !ended, ended),
            );
        }

        self.push_prebuffer(samples);
        (
            Vec::new(),
            self.status(GateState::Silence, rms, false, false),
        )
    }

    fn status(&self, state: GateState, rms: f32, active: bool, ended: bool) -> GateStatus {
        GateStatus {
            kind: self.config.kind,
            state,
            rms,
            active,
            ended,
        }
    }

    fn push_prebuffer(&mut self, samples: &[f32]) {
        let max_samples = (self.sample_rate as u64 * self.config.preroll_ms as u64 / 1000) as usize;
        if max_samples == 0 {
            return;
        }
        self.prebuffer.push_back(samples.to_vec());
        self.prebuffer_samples += samples.len();
        while self.prebuffer_samples > max_samples {
            match self.prebuffer.pop_front() {
                Some(first) => self.prebuffer_samples -= first.len(),
                None => break,
            }
        }
    }

    fn flush_prebuffer(&mut self) -> Vec<Vec<f32>> {
        self.prebuffer_samples = 0;
        self.prebuffer.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn loud(n: usize) -> Vec<f32> {
        vec![0.5; n]
    }

    fn quiet(n: usize) -> Vec<f32> {
        vec![0.0001; n]
    }

    #[test]
    fn manual_gate_passes_only_while_active_and_emits_tail() {
        let mut gate = ActivationGate::new(GateConfig::MANUAL, RATE);

        // 没按住：不放行，攒 preroll。
        let (out, st) = gate.process(&loud(160));
        assert!(out.is_empty());
        assert_eq!(st.state, GateState::Waiting);

        // 按住：先冲出 preroll，再放行当前块。
        gate.set_external_active(true);
        let (out, st) = gate.process(&loud(160));
        assert_eq!(st.state, GateState::Manual);
        assert!(st.active);
        assert_eq!(out.len(), 2, "preroll 应当被冲出来跟当前块一起走");

        // 松开：发一段静音尾触发断句。
        gate.set_external_active(false);
        let (out, st) = gate.process(&loud(160));
        assert_eq!(st.state, GateState::Released);
        assert!(st.ended);
        assert_eq!(out.len(), 1);
        let expected_tail = RATE as usize * GateConfig::MANUAL.tail_ms as usize / 1000;
        assert_eq!(out[0].len(), expected_tail);
        assert!(out[0].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn level_gate_opens_on_speech_and_closes_after_tail() {
        let mut gate = ActivationGate::new(GateConfig::level(0.012), RATE);
        let block = 160; // 10 ms

        let (out, st) = gate.process(&quiet(block));
        assert!(out.is_empty());
        assert_eq!(st.state, GateState::Silence);

        let (out, st) = gate.process(&loud(block));
        assert_eq!(st.state, GateState::Speech);
        assert_eq!(out.len(), 2, "上升沿应冲出 preroll");

        // tail 是 600 ms；静音块要放行到走完 tail 才收尾。
        let tail_blocks = (RATE as usize * 600 / 1000) / block;
        for i in 0..tail_blocks {
            let (out, st) = gate.process(&quiet(block));
            assert_eq!(out.len(), 1, "tail 期间静音也要放行");
            if i + 1 < tail_blocks {
                assert_eq!(st.state, GateState::Tail);
                assert!(st.active);
            } else {
                assert_eq!(st.state, GateState::TailEnd);
                assert!(st.ended);
                assert!(!st.active);
            }
        }

        let (out, st) = gate.process(&quiet(block));
        assert!(out.is_empty());
        assert_eq!(st.state, GateState::Silence);
    }

    #[test]
    fn level_gate_with_zero_threshold_passes_everything() {
        let mut gate = ActivationGate::new(GateConfig::level(0.0), RATE);
        let (out, st) = gate.process(&quiet(160));
        assert_eq!(out.len(), 1);
        assert_eq!(st.state, GateState::Always);
    }

    #[test]
    fn preroll_is_bounded() {
        let mut gate = ActivationGate::new(GateConfig::MANUAL, RATE);
        // preroll 100 ms = 1600 样本；灌远超上限的量。
        for _ in 0..50 {
            gate.process(&loud(160));
        }
        gate.set_external_active(true);
        let (out, _) = gate.process(&loud(160));
        let flushed: usize = out.iter().map(|c| c.len()).sum::<usize>() - 160;
        let cap = RATE as usize * GateConfig::MANUAL.preroll_ms as usize / 1000;
        assert!(
            flushed <= cap + 160,
            "preroll 不能无限涨: {flushed} > {cap}"
        );
    }

    #[test]
    fn set_config_keeps_external_active() {
        let mut gate = ActivationGate::new(GateConfig::MANUAL, RATE);
        gate.set_external_active(true);
        gate.set_config(GateConfig::level(0.02));
        assert!(gate.external_active(), "热切换门参数不该丢掉按键状态");
        assert_eq!(gate.config().kind, GateKind::Level);
    }
}
