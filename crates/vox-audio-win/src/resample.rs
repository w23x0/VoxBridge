//! 过渡用的线性流式重采样。
//!
//! 说明：这只是临时件。真正的重采样应该走 vox-dsp（rubato），等那边稳定后
//! 由 app 层接进来，这里就能删掉。之所以先自带一份：播放路径要能独立跑通，
//! 不能等别的 crate。
//!
//! 线性插值的质量对 24 kHz 语音够用（旧版就是这么干的，用了一年没人抱怨），
//! 但会有轻微高频镜像。跨块状态（小数读位置 + 尾巴样本）必须保留，
//! 否则每个块的接缝处都会咔嗒一声。

/// 有状态的线性重采样器。
pub(crate) struct LinearResampler {
    input_rate: u32,
    target_rate: u32,
    /// 上一块没消化完的尾巴，下一块要接在前面。
    remainder: Vec<f32>,
    /// 小数读位置，跨块保留。
    position: f64,
    scratch: Vec<f32>,
}

impl LinearResampler {
    pub(crate) fn new(input_rate: u32, target_rate: u32) -> Self {
        Self {
            input_rate,
            target_rate,
            remainder: Vec::new(),
            position: 0.0,
            scratch: Vec::new(),
        }
    }

    /// 采样率一致时是直通，不做任何计算。
    pub(crate) fn is_passthrough(&self) -> bool {
        self.input_rate == self.target_rate || self.input_rate == 0 || self.target_rate == 0
    }

    pub(crate) fn target_rate(&self) -> u32 {
        self.target_rate
    }

    /// 清空跨块状态。打断说话后必须调，否则残留尾巴会混进下一句。
    pub(crate) fn reset(&mut self) {
        self.remainder.clear();
        self.position = 0.0;
    }

    /// 处理一块样本，输出重采样结果。
    pub(crate) fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        if self.is_passthrough() {
            return samples.to_vec();
        }

        self.scratch.clear();
        self.scratch.reserve(self.remainder.len() + samples.len());
        self.scratch.extend_from_slice(&self.remainder);
        self.scratch.extend_from_slice(samples);
        let buffer = &self.scratch;

        if buffer.len() < 2 {
            self.remainder = buffer.clone();
            return Vec::new();
        }

        let ratio = self.input_rate as f64 / self.target_rate as f64;
        let limit = (buffer.len() - 1) as f64;
        let mut out =
            Vec::with_capacity(((limit - self.position) / ratio).ceil().max(0.0) as usize);

        let mut pos = self.position;
        let mut last = f64::NAN;
        while pos < limit {
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            // idx + 1 一定存在：pos < len-1 保证 idx <= len-2。
            out.push(buffer[idx] * (1.0 - frac) + buffer[idx + 1] * frac);
            last = pos;
            pos += ratio;
        }

        if out.is_empty() {
            self.remainder = buffer.clone();
            return Vec::new();
        }

        let next_position = last + ratio;
        let keep_start = (next_position as usize).min(buffer.len() - 1);
        self.position = next_position - keep_start as f64;
        self.remainder = buffer[keep_start..].to_vec();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rates_match() {
        let mut r = LinearResampler::new(24_000, 24_000);
        assert!(r.is_passthrough());
        assert_eq!(r.process(&[1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn upsampling_roughly_doubles_length() {
        let mut r = LinearResampler::new(24_000, 48_000);
        let input: Vec<f32> = (0..240).map(|i| i as f32 / 240.0).collect();
        let out = r.process(&input);
        // 线性插值的边界会差一两个样本，允许 4 个的误差。
        assert!(
            (out.len() as i64 - 480).abs() <= 4,
            "输出 {} 个，期望约 480 个",
            out.len()
        );
    }

    #[test]
    fn downsampling_roughly_halves_length() {
        let mut r = LinearResampler::new(48_000, 24_000);
        let input: Vec<f32> = (0..480).map(|i| i as f32 / 480.0).collect();
        let out = r.process(&input);
        assert!((out.len() as i64 - 240).abs() <= 4, "输出 {} 个", out.len());
    }

    #[test]
    fn ramp_stays_monotonic_across_chunks() {
        // 接缝处如果丢了状态，斜坡会出现回跳，这个断言就会挂。
        let mut r = LinearResampler::new(24_000, 44_100);
        let mut all = Vec::new();
        let mut v = 0.0f32;
        for _ in 0..5 {
            let chunk: Vec<f32> = (0..120)
                .map(|_| {
                    v += 0.001;
                    v
                })
                .collect();
            all.extend(r.process(&chunk));
        }
        assert!(all.len() > 500);
        for w in all.windows(2) {
            assert!(w[1] >= w[0] - 1e-6, "斜坡回跳：{} -> {}", w[0], w[1]);
        }
    }

    #[test]
    fn total_length_tracks_ratio_over_many_chunks() {
        let mut r = LinearResampler::new(24_000, 48_000);
        let mut total = 0usize;
        for _ in 0..50 {
            total += r.process(&vec![0.1; 240]).len();
        }
        // 50 * 240 输入 -> 约 24000 输出；累计误差应该是常数级而不是线性增长。
        assert!(
            (total as i64 - 24_000).abs() < 10,
            "累计 {total} 个，期望约 24000 个"
        );
    }

    #[test]
    fn single_sample_chunks_are_buffered_not_dropped() {
        let mut r = LinearResampler::new(24_000, 48_000);
        assert!(r.process(&[1.0]).is_empty());
        let out = r.process(&[2.0]);
        assert!(!out.is_empty());
    }

    #[test]
    fn reset_clears_seam_state() {
        let mut r = LinearResampler::new(24_000, 48_000);
        r.process(&vec![0.5; 100]);
        r.reset();
        let out = r.process(&[1.0, 1.0]);
        // 复位后第一个输出样本就是新数据，不带上一段的影子。
        assert_eq!(out.first().copied(), Some(1.0));
    }
}
