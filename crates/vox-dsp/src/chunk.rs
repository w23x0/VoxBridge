//! 把连续的音频样本切成固定大小的块。
//!
//! Windows（WASAPI 事件回调）和 Linux（PipeWire 的 process 回调）都要这一步，
//! 逻辑一模一样，所以放在平台中立的 `vox-dsp` 里。
//!
//! 块长按**帧**算而不是按样本：立体声设备一个 10 ms 块是 480 帧 = 960 个样本，
//! 按样本算会让两个平台的块长对不上（内核的 `INPUT_BLOCK_MS` 指的是时间）。

use vox_core::ports::AudioChunk;

/// 连续样本流 → 定长块。
pub struct Blocker {
    frames_per_block: usize,
    channels: u16,
    sample_rate: u32,
    staging: Vec<f32>,
}

impl Blocker {
    /// `block_ms` 为 0 或过小时按 10 ms 兜底：再小的块只会让下游白挨调用开销。
    pub fn new(sample_rate: u32, channels: u16, block_ms: u32) -> Self {
        let block_ms = block_ms.max(10);
        let channels = channels.max(1);
        let frames_per_block = ((sample_rate as u64 * block_ms as u64) / 1000).max(1) as usize;
        Self {
            frames_per_block,
            channels,
            sample_rate,
            staging: Vec::with_capacity(frames_per_block * channels as usize * 2),
        }
    }

    pub fn frames_per_block(&self) -> usize {
        self.frames_per_block
    }

    fn block_samples(&self) -> usize {
        self.frames_per_block * self.channels.max(1) as usize
    }

    /// 吃进一段交错样本，凑够一块就调一次回调。不足一块的留在内部等下一次。
    pub fn feed(&mut self, samples: &[f32], on_chunk: &mut dyn FnMut(AudioChunk)) {
        self.staging.extend_from_slice(samples);
        let block = self.block_samples();
        while self.staging.len() >= block {
            let rest = self.staging.split_off(block);
            let chunk = std::mem::replace(&mut self.staging, rest);
            on_chunk(AudioChunk {
                samples: chunk,
                sample_rate: self.sample_rate,
                channels: self.channels,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_only_full_blocks() {
        let mut blocker = Blocker::new(48_000, 1, 20); // 960 帧一块
        let sizes = std::cell::RefCell::new(Vec::<usize>::new());
        let mut sink = |c: AudioChunk| sizes.borrow_mut().push(c.samples.len());
        blocker.feed(&vec![0.0; 1000], &mut sink);
        assert_eq!(*sizes.borrow(), vec![960], "不满一块的不该出货");
        blocker.feed(&vec![0.0; 920], &mut sink);
        assert_eq!(*sizes.borrow(), vec![960, 960], "攒够第二块再出一次");
    }

    #[test]
    fn counts_frames_not_samples_for_stereo() {
        let mut blocker = Blocker::new(48_000, 2, 10); // 480 帧 = 960 个样本
        let sizes = std::cell::RefCell::new(Vec::<usize>::new());
        let mut sink = |c: AudioChunk| {
            assert_eq!(c.channels, 2);
            assert_eq!(c.sample_rate, 48_000);
            sizes.borrow_mut().push(c.samples.len());
        };
        blocker.feed(&vec![0.0; 960], &mut sink);
        assert_eq!(*sizes.borrow(), vec![960]);
    }

    #[test]
    fn keeps_sample_order_across_blocks() {
        let mut blocker = Blocker::new(1000, 1, 10); // 10 帧一块
        let mut flat = Vec::new();
        let mut sink = |c: AudioChunk| flat.extend(c.samples);
        let input: Vec<f32> = (0..25).map(|i| i as f32).collect();
        blocker.feed(&input, &mut sink);
        assert_eq!(flat, (0..20).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn tiny_block_ms_is_clamped() {
        assert_eq!(Blocker::new(48_000, 1, 0).frames_per_block(), 480);
        assert_eq!(Blocker::new(48_000, 1, 9).frames_per_block(), 480);
    }
}
