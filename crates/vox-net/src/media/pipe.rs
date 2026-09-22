//! 媒体面的管子：抖动缓冲、重切块、paced drain、欠载/过载（S3 设计稿 §2.4.2 / §2.6.2）。
//!
//! **纯逻辑**：不读时钟（时间戳一律由调用方传进来）、不起线程、不碰 socket。
//! 这样"预填/补静音/丢最旧/节拍"这些事可以在注入时钟下确定性单测，
//! 线程与 socket 都留在 `server.rs` / `client.rs` 里。
//!
//! 两侧各一个状态机：
//! - [`JitterBuffer`]：入站（收帧 → 环 → 每 `block_ms` 出一块）。
//! - [`OutPipe`]：出站（`push` 攒块 → 每 `block_ms` 一帧）。

use std::collections::VecDeque;

use super::frame::{self, FrameHeader, MediaError};

/// 管子参数。缺省值逐字照设计稿 §2.3.4（也就是 `Settings.net` 的缺省）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeConfig {
    /// 一块的长度（ms）。v1 恒等于 `INPUT_BLOCK_MS`。
    pub block_ms: u32,
    /// 抖动缓冲目标深度（ms）：攒够这么多才开始排空。
    pub jitter_ms: u32,
    /// 连续欠载最多补多久静音（ms）。超过就停止回调（"没有输入"不许被静音伪装）。
    pub pad_ms: u32,
    /// 环形缓冲上限（ms）。满了丢最旧。
    pub queue_ms: u32,
    /// 出站空闲多久发一个保活帧（ms）。
    pub keepalive_ms: u32,
    /// 多久没收到任何帧就判对端掉线（ms）。出站侧用它退避重连。
    pub peer_timeout_ms: u32,
}

impl Default for PipeConfig {
    fn default() -> Self {
        Self {
            block_ms: 20,
            jitter_ms: 40,
            pad_ms: 200,
            queue_ms: 160,
            keepalive_ms: 1_000,
            peer_timeout_ms: 3_000,
        }
    }
}

impl PipeConfig {
    /// 一块多少样本。
    pub fn block_samples(&self, rate: u32) -> usize {
        samples_for_ms(rate, self.block_ms)
    }

    /// 预填要多少样本。
    pub fn prefill_samples(&self, rate: u32) -> usize {
        samples_for_ms(rate, self.jitter_ms)
    }

    /// 环形缓冲的上限（样本数）。
    pub fn queue_samples(&self, rate: u32) -> usize {
        samples_for_ms(rate, self.queue_ms)
    }
}

/// 毫秒 → 样本数（向下取整，至少 1：0 会让下游死循环）。
pub fn samples_for_ms(rate: u32, ms: u32) -> usize {
    ((rate as u64 * ms as u64) / 1000).max(1) as usize
}

/// 样本数 → 毫秒（向下取整）。
fn samples_to_ms(samples: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    samples * 1000 / rate as u64
}

/// 管子对外报的统计（只读投影）。`*_ms` 都是毫秒。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PipeStats {
    pub frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// 过载丢最旧的时长。
    pub dropped_ms: u64,
    /// 欠载补静音的时长。
    pub padded_ms: u64,
    /// 连续欠载超过 `pad_ms`、已停止回调的时长。
    pub idle_ms: u64,
    /// 收到的 seq 不连续（丢帧/乱序）的次数。
    pub seq_gaps: u64,
    /// 坏帧（magic/版本/种类/flags/声道/超长/保活带载荷/文本帧）的条数。
    pub bad_frames: u64,
    /// 采样率不符的帧数。**与 `bad_frames` 分开计**：它不是"内容坏了"，是"会话不一致"。
    pub rate_mismatch: u64,
    /// 此刻对端在不在。
    pub connected: bool,
    /// 链路断过之后又连上的次数（含"对端先不在、后来起来"这一种）。
    pub reconnects: u64,
    /// 距最后一帧的时长（毫秒）。
    pub last_frame_age_ms: u64,
}

/// 方向无关的计数器。两侧共用同一份形状，`stats()` 也只写一次。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Counters {
    pub frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub dropped_samples: u64,
    pub padded_samples: u64,
    pub idle_ms: u64,
    pub seq_gaps: u64,
    pub bad_frames: u64,
    pub rate_mismatch: u64,
    pub connected: bool,
    pub reconnects: u64,
    pub last_frame_ms: u64,
    /// 上一帧的 seq（`None` = 还没见过帧）。
    seq: Option<u32>,
}

impl Counters {
    /// 收到一帧：计数 + 检测 seq 不连续。
    pub(crate) fn on_frame(&mut self, header: &FrameHeader, bytes: usize, now_ms: u64) {
        self.frames_in += 1;
        self.bytes_in += bytes as u64;
        self.last_frame_ms = now_ms;
        match self.seq {
            None => self.seq = Some(header.seq),
            Some(last) => {
                if header.seq != last.wrapping_add(1) {
                    self.seq_gaps += 1;
                }
                self.seq = Some(header.seq);
            }
        }
    }

    pub fn stats(&self, rate: u32, now_ms: u64) -> PipeStats {
        PipeStats {
            frames_in: self.frames_in,
            frames_out: self.frames_out,
            bytes_in: self.bytes_in,
            bytes_out: self.bytes_out,
            dropped_ms: samples_to_ms(self.dropped_samples, rate),
            padded_ms: samples_to_ms(self.padded_samples, rate),
            idle_ms: self.idle_ms,
            seq_gaps: self.seq_gaps,
            bad_frames: self.bad_frames,
            rate_mismatch: self.rate_mismatch,
            connected: self.connected,
            reconnects: self.reconnects,
            last_frame_age_ms: now_ms.saturating_sub(self.last_frame_ms),
        }
    }

    pub fn on_disconnect(&mut self) {
        self.connected = false;
        self.seq = None;
    }

    /// 记录一条协议错误。采样率不符走 `rate_mismatch`，其余坏帧走 `bad_frames`。
    pub fn on_protocol_error(&mut self, error: &MediaError) {
        match error {
            MediaError::Rate { .. } => self.rate_mismatch += 1,
            _ => self.bad_frames += 1,
        }
    }
}

/// 排空线程还能睡多久（毫秒）。
const IDLE_POLL_MS: u64 = 100;

/// 一次 `tick` 排出来的东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    /// 一块真实音频。
    Data,
    /// 一块补的静音（欠载）。
    Silence,
}

/// 入站状态机（收帧 → 抖动环 → paced drain）。
#[derive(Debug)]
pub struct JitterBuffer {
    cfg: PipeConfig,
    rate: u32,
    /// 环形缓冲（PCM16，与线上同一种样本）。
    ring: Vec<i16>,
    head: usize,
    len: usize,
    state: State,
    /// 下一次该出块的**绝对**时刻（毫秒；绝对时刻表，不按到达时刻）。
    next_due_ms: u64,
    /// 连续欠载累计了多久（毫秒）。
    under_ms: u64,
    /// 停止回调的起点（进入 `Idle` 时钉下）。
    idle_since_ms: Option<u64>,
    /// 这个管线之前接过对端没有（决定 `reconnects` 算不算）。
    ever_connected: bool,
    counters: Counters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// 还没有对端：不排空、也不补静音。
    Waiting,
    /// 对端在，但还没攒够 `jitter_ms`：一次回调都不发。
    Prefill,
    /// 按 `block_ms` 的绝对时刻表出块。
    Running,
    /// 连续欠载超过 `pad_ms`：已停止回调，等新帧把它拉回 `Prefill`。
    Idle,
}

impl JitterBuffer {
    pub fn new(cfg: PipeConfig, rate: u32) -> Self {
        let cap = cfg.queue_samples(rate);
        Self {
            cfg,
            rate,
            ring: vec![0; cap],
            head: 0,
            len: 0,
            state: State::Waiting,
            next_due_ms: 0,
            under_ms: 0,
            idle_since_ms: None,
            ever_connected: false,
            counters: Counters::default(),
        }
    }

    /// 对端连上了：清掉上一个对端的时间线，重新预填（设计稿 §2.4.2）。
    pub fn on_connected(&mut self, now_ms: u64) {
        self.head = 0;
        self.len = 0;
        self.under_ms = 0;
        self.state = State::Prefill;
        self.counters.connected = true;
        self.counters.last_frame_ms = now_ms;
        if self.ever_connected {
            self.counters.reconnects += 1;
        }
        self.ever_connected = true;
    }

    /// 对端走了（断线 / 掉线 / 被掐）：环里剩下的丢掉，**不补静音**。
    pub fn on_disconnected(&mut self, now_ms: u64) {
        self.close_idle(now_ms);
        self.head = 0;
        self.len = 0;
        self.under_ms = 0;
        self.state = State::Waiting;
        self.counters.connected = false;
        self.counters.on_disconnect();
    }

    /// 收下一帧（已经过 [`frame::decode`]）。
    pub fn on_frame(&mut self, header: &FrameHeader, payload: &[u8], now_ms: u64) {
        if self.state == State::Idle {
            // 又来声音了：把 idle 时长结掉，重新预填。
            self.close_idle(now_ms);
            self.state = State::Prefill;
            self.under_ms = 0;
        } else if self.state == State::Waiting {
            // 没登记连接就来了帧（正常流程不会发生）：按"刚连上"处理。
            self.state = State::Prefill;
            self.counters.connected = true;
        }
        self.counters.on_frame(header, payload.len(), now_ms);
        if header.kind == frame::FrameKind::KeepAlive {
            return;
        }
        for pair in payload.as_chunks::<2>().0 {
            self.push_sample(i16::from_le_bytes(*pair));
        }
    }

    /// 收下一条坏帧的记账（调用方随后会断连接）。
    pub fn on_protocol_error(&mut self, error: &MediaError) {
        self.counters.on_protocol_error(error);
    }

    /// 记一帧**我们自己**发出去的（入站侧只在空闲时发保活帧）。
    pub fn on_own_frame(&mut self, bytes: usize) {
        self.counters.frames_out += 1;
        self.counters.bytes_out += bytes as u64;
    }

    /// 排空线程该睡多久。
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        match self.state {
            State::Running => self
                .next_due_ms
                .saturating_sub(now_ms)
                .clamp(1, u64::from(self.cfg.block_ms.max(1))),
            _ => IDLE_POLL_MS,
        }
    }

    /// 按绝对时刻表出一块（落后一拍就补一拍，**最多补一拍**，防雪崩）。
    ///
    /// `None` = 这次不回调（没对端 / 预填中 / 已进 idle / 还没到点）。
    /// 出块时把 `out` 清空后填成一块样本（真实音频或静音）。
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<f32>) -> Option<Emit> {
        match self.state {
            State::Waiting | State::Idle => return None,
            State::Prefill => {
                if self.len < self.cfg.prefill_samples(self.rate) {
                    return None;
                }
                self.state = State::Running;
                self.next_due_ms = now_ms + u64::from(self.cfg.block_ms);
            }
            State::Running => {}
        }
        if now_ms < self.next_due_ms {
            return None;
        }
        let block_ms = u64::from(self.cfg.block_ms);
        self.next_due_ms = if now_ms >= self.next_due_ms + block_ms {
            now_ms + block_ms
        } else {
            self.next_due_ms + block_ms
        };

        let need = self.cfg.block_samples(self.rate);
        out.clear();
        if self.len >= need {
            out.reserve(need);
            for _ in 0..need {
                out.push(frame::sample_from_pcm16(self.pop_front()));
            }
            self.under_ms = 0;
            return Some(Emit::Data);
        }
        // 欠载：补静音，但连续欠载超过 `pad_ms` 就停止回调。
        if self.under_ms + block_ms > u64::from(self.cfg.pad_ms) {
            self.state = State::Idle;
            self.idle_since_ms = Some(now_ms);
            self.under_ms = 0;
            return None;
        }
        self.under_ms += block_ms;
        self.counters.padded_samples += need as u64;
        out.resize(need, 0.0);
        Some(Emit::Silence)
    }

    /// 此刻环里有多少样本。
    pub fn buffered_samples(&self) -> usize {
        self.len
    }

    pub fn stats(&self, now_ms: u64) -> PipeStats {
        let mut stats = self.counters.stats(self.rate, now_ms);
        if let Some(since) = self.idle_since_ms {
            stats.idle_ms += now_ms.saturating_sub(since);
        }
        stats
    }

    fn push_sample(&mut self, sample: i16) {
        if self.len == self.ring.len() {
            // 过载：丢最旧（与 `INPUT_QUEUE_SIZE` 同策略），深度不变。
            self.pop_front();
            self.counters.dropped_samples += 1;
        }
        let tail = (self.head + self.len) % self.ring.len();
        self.ring[tail] = sample;
        self.len += 1;
    }

    fn pop_front(&mut self) -> i16 {
        let sample = self.ring[self.head];
        self.head = (self.head + 1) % self.ring.len();
        self.len -= 1;
        sample
    }

    fn close_idle(&mut self, now_ms: u64) {
        if let Some(since) = self.idle_since_ms.take() {
            self.counters.idle_ms += now_ms.saturating_sub(since);
        }
    }
}

/// 出站状态机（`push` 攒块 → 每 `block_ms` 一帧 → 余头交给 `flush`）。
#[derive(Debug)]
pub struct OutPipe {
    cfg: PipeConfig,
    rate: u32,
    /// 还没攒成一块的样本（PCM16；线上就是这种样本）。
    pending: VecDeque<i16>,
    /// 队列上限（样本数）。
    cap: usize,
}

impl OutPipe {
    pub fn new(cfg: PipeConfig, rate: u32) -> Self {
        let cap = cfg.queue_samples(rate);
        Self {
            cfg,
            rate,
            pending: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// 送一批样本进来。返回**丢掉的样本数**（队满丢最旧，绝不阻塞调用方）。
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let mut dropped = 0usize;
        for &sample in samples {
            if self.pending.len() == self.cap {
                self.pending.pop_front();
                dropped += 1;
            }
            self.pending.push_back(frame::sample_to_pcm16(sample));
        }
        dropped
    }

    /// 攒够一块了吗？够就把载荷写进 `out`（先清空）。
    pub fn take_block(&mut self, out: &mut Vec<u8>) -> bool {
        let block = self.cfg.block_samples(self.rate);
        if self.pending.len() < block {
            return false;
        }
        out.clear();
        out.reserve(block * 2);
        for _ in 0..block {
            let sample = self.pending.pop_front().expect("刚刚数过长度");
            out.extend_from_slice(&sample.to_le_bytes());
        }
        true
    }

    /// 不足一块的余头（`flush` 用）：有空就发货，返回是否发了。
    pub fn take_remainder(&mut self, out: &mut Vec<u8>) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        out.clear();
        out.reserve(self.pending.len() * 2);
        for sample in self.pending.drain(..) {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        true
    }

    /// 还没发出去的样本数。
    pub fn pending_samples(&self) -> usize {
        self.pending.len()
    }

    pub fn block_samples(&self) -> usize {
        self.cfg.block_samples(self.rate)
    }

    pub fn config(&self) -> PipeConfig {
        self.cfg
    }
}

/// 每条连接从随机值起的 seq 种子。
///
/// 这不是密码学随机：`seq` 的用途只有一个——检测丢帧/乱序，所以拿系统时钟的高位
/// 混一下足够，也就不为此新增依赖（设计稿只要求"每条连接从随机值起"）。
pub(crate) fn random_seq_seed(extra: u64) -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    (nanos ^ extra.rotate_left(17)).wrapping_mul(2_654_435_761) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::frame::{FrameHeader, FrameKind};

    fn cfg() -> PipeConfig {
        PipeConfig::default()
    }

    fn payload(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn header(seq: u32) -> FrameHeader {
        FrameHeader {
            kind: FrameKind::Pcm16Le,
            seq,
            ts_ms: 0,
            rate: 16_000,
            channels: 1,
        }
    }

    #[test]
    fn defaults_are_the_design_table() {
        let cfg = cfg();
        assert_eq!(cfg.block_ms, 20);
        assert_eq!(cfg.jitter_ms, 40);
        assert_eq!(cfg.pad_ms, 200);
        assert_eq!(cfg.queue_ms, 160);
        assert_eq!(cfg.keepalive_ms, 1_000);
        assert_eq!(cfg.peer_timeout_ms, 3_000);
        assert_eq!(cfg.block_samples(16_000), 320, "@16k 一块 20 ms = 320 样本");
        assert_eq!(cfg.block_samples(24_000), 480, "@24k 一块 20 ms = 480 样本");
        assert_eq!(cfg.queue_samples(16_000), 2_560, "160 ms @16k");
        assert_eq!(samples_for_ms(16_000, 0), 1, "0 ms 也不能是 0 样本");
    }

    #[test]
    fn catch_up_is_capped_to_one_block_per_tick() {
        let mut jb = JitterBuffer::new(cfg(), 16_000);
        jb.on_connected(0);
        let payload = payload(&vec![1000i16; 320 * 8]);
        let header = FrameHeader {
            kind: FrameKind::Pcm16Le,
            seq: 0,
            ts_ms: 0,
            rate: 16_000,
            channels: 1,
        };
        jb.on_frame(&header, &payload, 0);
        let mut out = Vec::new();
        // 预填在第一次 tick 时满足（2560 样本 ≥ 40 ms），第一拍的 due 在 t=20。
        assert_eq!(jb.tick(0, &mut out), None, "刚满足预填，还没到第一拍");
        assert_eq!(jb.tick(20, &mut out), Some(Emit::Data));
        // 排空线程被卡了 100 ms：只补一拍，并把时刻表追到"现在之后的一拍"。
        assert_eq!(jb.tick(120, &mut out), Some(Emit::Data));
        assert_eq!(jb.tick(120, &mut out), None, "同一时刻不许连出两块");
        assert_eq!(jb.tick(139, &mut out), None);
        assert_eq!(jb.tick(140, &mut out), Some(Emit::Data));
    }

    #[test]
    fn out_pipe_drops_the_oldest_when_full() {
        let mut pipe = OutPipe::new(cfg(), 16_000);
        let cap = cfg().queue_samples(16_000);
        let dropped = pipe.push(&vec![0.25f32; cap + 10]);
        assert_eq!(dropped, 10, "多出来的 10 个样本该从最旧那头丢");
        assert_eq!(pipe.pending_samples(), cap);
        let mut out = Vec::new();
        assert!(pipe.take_block(&mut out));
        assert_eq!(out.len(), 640, "一块 @16k = 320 样本 = 640 字节");
    }

    #[test]
    fn a_reconnect_starts_a_fresh_timeline() {
        let mut jb = JitterBuffer::new(cfg(), 16_000);
        let mut out = Vec::new();

        jb.on_connected(0);
        jb.on_frame(&header(0), &payload(&[100; 640]), 0);
        assert_eq!(jb.tick(0, &mut out), None);
        assert_eq!(jb.tick(20, &mut out), Some(Emit::Data));
        assert_eq!(jb.stats(20).reconnects, 0, "第一次连上不算重连");

        // 对端走了：环里剩下的丢掉，**不补静音**（"对端不在" ≠ "对端在静音"）。
        jb.on_disconnected(40);
        assert_eq!(jb.tick(60, &mut out), None, "没有对端就不该有回调");
        assert_eq!(jb.stats(60).padded_ms, 0);
        assert!(!jb.stats(60).connected);

        // 新对端连上：重新预填，不沿用上一个对端的时间线。
        jb.on_connected(100);
        assert_eq!(jb.stats(100).reconnects, 1);
        jb.on_frame(&header(0), &payload(&[200; 640]), 100);
        assert_eq!(jb.tick(100, &mut out), None, "重新预填，先不回调");
        assert_eq!(jb.tick(120, &mut out), Some(Emit::Data));
        assert_eq!(
            out[0],
            frame::sample_from_pcm16(200),
            "出来的是新对端的声音"
        );
    }

    #[test]
    fn seq_gaps_counts_discontinuities_only() {
        let mut jb = JitterBuffer::new(cfg(), 16_000);
        jb.on_connected(0);
        let payload = payload(&[1; 2]);
        for seq in [7u32, 8, 10, 11, 14] {
            jb.on_frame(&header(seq), &payload, 0);
        }
        assert_eq!(jb.stats(0).seq_gaps, 2, "8→10 与 11→14 各一次；连续的不算");
        assert_eq!(jb.stats(0).frames_in, 5);

        // 环绕不算断：u32::MAX → 0 是连续的下一个。
        jb.on_frame(&header(u32::MAX), &payload, 0);
        jb.on_frame(&header(0), &payload, 0);
        assert_eq!(jb.stats(0).seq_gaps, 3, "14→MAX 是一次断，MAX→0 不是");
    }
}
