//! RNNoise 降噪封装（nnnoiseless）。
//!
//! `deep_filter` crate (DeepFilterNet 的 Rust 包) 只提供 FFT/ERB 基础运算，
//! 不含神经网络推理和模型权重——要完整跑起来还需要 ONNX Runtime + 外部权重文件。
//! 因此退而选择 nnnoiseless：纯 Rust 移植的 Xiph RNNoise，BSD-3 许可，
//! 权重内嵌，48 kHz / 480 帧，无外部依赖，MSVC 直接编译通过。
//!
//! 降噪质量：RNNoise 在常见办公/居家噪声下表现良好，对键盘声和风扇声有效。
//! 不如 DeepFilterNet 对复杂背景声的效果，但够用且零部署成本。

use nnnoiseless::DenoiseState;
use vox_core::ports::PortResult;

/// 降噪后端名称，供设置 UI 显示。
pub fn backend_name() -> &'static str {
    "RNNoise (nnnoiseless)"
}

/// nnnoiseless 工作在 48 kHz。流水线须确保喂进来的音频已经是这个采样率。
pub const NATIVE_SAMPLE_RATE: u32 = 48_000;

/// nnnoiseless 每帧处理的样本数。
const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE; // 480

/// 流式降噪器。接受任意长度 48 kHz 单声道 f32（[-1.0, 1.0] 范围），
/// 内部缓冲到 480 帧后处理，输出已降噪的样本。
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    /// 等待凑满一帧的输入缓冲。
    buffer: Vec<f32>,
    /// 第一帧有 fade-in 伪影，丢弃。
    first_frame_discarded: bool,
}

impl Denoiser {
    /// 创建降噪器。纯内存分配，不会失败（但为了接口一致性返回 PortResult）。
    pub fn new() -> PortResult<Self> {
        Ok(Self {
            state: DenoiseState::new(),
            buffer: Vec::with_capacity(FRAME_SIZE),
            first_frame_discarded: false,
        })
    }

    /// 喂入任意长度的 f32 样本（范围 [-1.0, 1.0]），返回已降噪的输出。
    /// 输入不够一帧时返回空 Vec。
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.buffer.extend_from_slice(input);
        let mut output = Vec::new();
        while self.buffer.len() >= FRAME_SIZE {
            let chunk: Vec<f32> = self.buffer.drain(..FRAME_SIZE).collect();
            let processed = self.process_one_frame(&chunk);
            if let Some(frame) = processed {
                output.extend_from_slice(&frame);
            }
        }
        output
    }

    /// 复位内部状态。新一轮录音开始时调用。
    pub fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.buffer.clear();
        self.first_frame_discarded = false;
    }

    /// 处理恰好一帧。nnnoiseless 用 i16 刻度的浮点，需要来回缩放。
    fn process_one_frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        // nnnoiseless 期望 [-32768, 32767] 范围的 f32。
        let scaled_in: Vec<f32> = frame.iter().map(|&s| s * 32767.0).collect();
        let mut out_buf = [0.0f32; FRAME_SIZE];
        self.state.process_frame(&mut out_buf, &scaled_in);

        if !self.first_frame_discarded {
            // 第一帧包含 fade-in 伪影，丢弃。
            self.first_frame_discarded = true;
            return None;
        }

        // 缩放回 [-1.0, 1.0]。
        let normalized: Vec<f32> = out_buf.iter().map(|&s| s / 32767.0).collect();
        Some(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denoiser_creates_successfully() {
        let d = Denoiser::new();
        assert!(d.is_ok());
    }

    #[test]
    fn preserves_length_across_stream_of_odd_chunks() {
        let mut d = Denoiser::new().unwrap();
        // 喂入 4800 样本 = 10 帧，散碎块。第一帧被丢弃，产出 9 帧 = 4320 样本。
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let chunk_sizes = [137, 253, 91, 499, 311, 4800 - 137 - 253 - 91 - 499 - 311];
        let mut total_output = 0;
        let mut offset = 0;
        for &sz in &chunk_sizes {
            let out = d.process(&input[offset..offset + sz]);
            total_output += out.len();
            offset += sz;
        }
        // 4800 样本 = 10 帧，第一帧丢弃 → 9 * 480 = 4320。
        assert_eq!(
            total_output, 4320,
            "10 帧输入扣掉首帧应产出 4320 样本，实际 {total_output}"
        );
    }

    #[test]
    fn does_not_blow_up_on_silence() {
        let mut d = Denoiser::new().unwrap();
        let silence = vec![0.0f32; 960]; // 2 帧
        let out = d.process(&silence);
        // 第一帧丢弃，第二帧产出 480 样本（全是接近零的值）。
        assert_eq!(out.len(), 480);
        // 静音输入产出也应接近零。
        let max_abs = out.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs < 0.01,
            "静音输入的输出不该有大幅值，实际最大 {max_abs}"
        );
    }

    #[test]
    fn does_not_blow_up_on_chunk_shorter_than_one_frame() {
        let mut d = Denoiser::new().unwrap();
        // 只喂 100 样本，不够一帧。
        let input = vec![0.1f32; 100];
        let out = d.process(&input);
        assert!(out.is_empty(), "不够一帧时不应产出");
    }

    #[test]
    fn reset_clears_state() {
        let mut d = Denoiser::new().unwrap();
        // 喂一点数据建立状态。
        d.process(&vec![0.5f32; 480]);
        d.reset();
        // reset 后首帧应再次被丢弃。
        let out = d.process(&vec![0.0f32; 960]);
        // 2 帧输入，首帧丢弃 → 1 帧输出。
        assert_eq!(out.len(), 480, "reset 后首帧应重新被丢弃");
    }

    #[test]
    fn backend_name_is_rnnoise() {
        assert!(backend_name().contains("RNNoise"));
    }

    #[test]
    fn native_sample_rate_is_48k() {
        assert_eq!(NATIVE_SAMPLE_RATE, 48_000);
    }
}
