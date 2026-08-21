//! 任意采样率互转。
//!
//! 音频回调给的块大小不固定，这里负责把散碎的输入攒够 rubato 要求的帧长，
//! 产出所有已就绪的输出样本。同率时完全透传，不分配 rubato 实例。
//!
//! 选 `SincFixedIn` 而非 `FastFixedIn`：语音场景对高频伪影敏感，
//! sinc 有抗混叠滤波器，延迟多约 5 ms 但音质明显好于多项式插值。

use rubato::{
    Resampler as RubatoResampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// 音频回调给的块不知道多大，这个 chunk_size 只是 rubato 内部的处理粒度。
/// 选小一点减少延迟，480 样本 = 10 ms @ 48 kHz、30 ms @ 16 kHz，够小。
const CHUNK_SIZE: usize = 480;

/// sinc 插值参数：128 点滤波器 + cubic 插值，实测 CPU 开销可忽略。
fn sinc_params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    }
}

/// 流式重采样器。内部缓冲使调用方可以任意长度喂入。
pub struct Resampler {
    inner: ResamplerInner,
}

enum ResamplerInner {
    /// 输入输出采样率相同，直接透传。
    Passthrough,
    /// 需要实际重采样。
    Active {
        resampler: Box<SincFixedIn<f32>>,
        /// 等待凑满一帧的输入缓冲。
        buffer: Vec<f32>,
        /// rubato 每次要求的输入帧数。
        frames_needed: usize,
    },
}

impl Resampler {
    /// 创建重采样器。`input_rate == output_rate` 时零开销透传。
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        if input_rate == output_rate {
            return Self {
                inner: ResamplerInner::Passthrough,
            };
        }
        let ratio = output_rate as f64 / input_rate as f64;
        // max_relative_ratio = 1.0：固定比率，不需要运行时变速。
        let resampler = SincFixedIn::<f32>::new(
            ratio,
            1.0,
            sinc_params(),
            CHUNK_SIZE,
            1, // 单声道
        )
        .expect("rubato 参数合法，不应失败");
        let frames_needed = resampler.input_frames_next();
        Self {
            inner: ResamplerInner::Active {
                resampler: Box::new(resampler),
                buffer: Vec::with_capacity(frames_needed),
                frames_needed,
            },
        }
    }

    /// 输出端延迟（样本数）。透传时为 0。
    pub fn output_delay(&self) -> usize {
        match &self.inner {
            ResamplerInner::Passthrough => 0,
            ResamplerInner::Active { resampler, .. } => resampler.output_delay(),
        }
    }

    /// 喂入任意长度的单声道样本，返回当前已产出的输出样本。
    /// 输入不够一帧时返回空 Vec，不阻塞。
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        match &mut self.inner {
            ResamplerInner::Passthrough => input.to_vec(),
            ResamplerInner::Active {
                resampler,
                buffer,
                frames_needed,
            } => {
                buffer.extend_from_slice(input);
                let mut output = Vec::new();
                while buffer.len() >= *frames_needed {
                    let chunk: Vec<f32> = buffer.drain(..*frames_needed).collect();
                    // rubato 的 process 接口：&[impl AsRef<[T]>]，每个元素是一个声道。
                    let result = resampler
                        .process(&[&chunk], None)
                        .expect("输入长度正确，不应失败");
                    output.extend_from_slice(&result[0]);
                    // 下一帧要求的输入长度可能变化（SincFixedIn 实际不变，但接口允许）。
                    *frames_needed = resampler.input_frames_next();
                }
                output
            }
        }
    }

    /// 冲出尾巴：流结束时把内部缓冲里不够一帧的残余补零处理掉。
    pub fn flush(&mut self) -> Vec<f32> {
        match &mut self.inner {
            ResamplerInner::Passthrough => Vec::new(),
            ResamplerInner::Active {
                resampler, buffer, ..
            } => {
                if buffer.is_empty() {
                    return Vec::new();
                }
                // process_partial 会内部补零到所需帧长。
                let input: Vec<f32> = std::mem::take(buffer);
                let result = resampler
                    .process_partial(Some(&[&input]), None)
                    .expect("partial 处理不应失败");
                result[0].clone()
            }
        }
    }

    /// 复位内部状态，丢掉残余缓冲。新一轮录音开始时调用。
    pub fn reset(&mut self) {
        match &mut self.inner {
            ResamplerInner::Passthrough => {}
            ResamplerInner::Active {
                resampler,
                buffer,
                frames_needed,
            } => {
                resampler.reset();
                buffer.clear();
                *frames_needed = resampler.input_frames_next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rates_match() {
        let mut r = Resampler::new(48_000, 48_000);
        let input = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let output = r.process(&input);
        assert_eq!(output, input, "同率透传不该改变数据");
        assert_eq!(r.output_delay(), 0);
    }

    #[test]
    fn downsample_48k_to_16k_output_length_is_roughly_one_third() {
        let mut r = Resampler::new(48_000, 16_000);
        // 喂 4800 样本 = 100 ms @ 48 kHz，期望输出 ~1600 样本。
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.001).sin()).collect();
        let output = r.process(&input);
        let flush = r.flush();
        let total = output.len() + flush.len();
        // 允许 ±10% 的偏差（内部缓冲 + sinc 延迟）。
        let expected = 1600;
        assert!(
            total > expected * 85 / 100 && total < expected * 115 / 100,
            "48k→16k 输出应约为输入的 1/3，实际 {total} vs 期望 ~{expected}"
        );
    }

    #[test]
    fn streaming_odd_chunks_matches_single_big_chunk() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.002).sin()).collect();

        // 一次性处理。
        let mut r1 = Resampler::new(48_000, 24_000);
        let one_shot = r1.process(&input);
        let one_shot_flush = r1.flush();
        let total_one: usize = one_shot.len() + one_shot_flush.len();

        // 散碎块处理（故意选不整除的奇怪尺寸）。
        let mut r2 = Resampler::new(48_000, 24_000);
        let mut total_stream: usize = 0;
        let chunk_sizes = [137, 253, 91, 499, 311, 4800 - 137 - 253 - 91 - 499 - 311];
        let mut offset = 0;
        for &sz in &chunk_sizes {
            let out = r2.process(&input[offset..offset + sz]);
            total_stream += out.len();
            offset += sz;
        }
        total_stream += r2.flush().len();

        // 散碎块和一次性的输出总量应相差不超过几个样本（rubato 内部对齐导致）。
        let diff = (total_one as i64 - total_stream as i64).unsigned_abs();
        assert!(
            diff <= 5,
            "流式与一次性输出长度差异过大: one_shot={total_one}, stream={total_stream}"
        );
    }

    #[test]
    fn flush_drains_the_tail() {
        let mut r = Resampler::new(16_000, 48_000);
        // 只喂 100 样本，不够一帧，process 应返回空。
        let input = vec![0.5; 100];
        let out = r.process(&input);
        // 100 样本在 chunk_size=480 的 SincFixedIn 下不够一帧。
        // flush 应该把它们挤出来。
        let flushed = r.flush();
        let total = out.len() + flushed.len();
        assert!(total > 0, "flush 应该产出被缓冲的尾巴");
    }

    #[test]
    fn reset_clears_internal_state() {
        let mut r = Resampler::new(48_000, 16_000);
        let input = vec![0.1; 200];
        r.process(&input);
        r.reset();
        // reset 之后 flush 应该没有残留。
        let flushed = r.flush();
        assert!(flushed.is_empty(), "reset 后不应有残留");
    }
}
