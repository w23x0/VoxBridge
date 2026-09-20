//! Linux 音频三个工厂。
//!
//! 设备目录**已经是真的**（`vox-audio-linux` 的 PipeWire 枚举）。
//! 采集与播放还没写（P1），这里给的是"上机就报错"的占位实现——
//! **不返回静音、不假装成功**：`CaptureSource::start` / `PlaybackSink::open`
//! 会带着明确原因失败，装配层把它当流水线错误显示给用户。

use std::sync::Arc;

use vox_core::pipeline::{CaptureFactory, PlaybackFactory};
use vox_core::ports::{
    AudioChunk, CaptureFormat, CaptureSource, CaptureTarget, DeviceRegistry, PlaybackSink,
    PortError, PortResult,
};

/// 设备枚举：PipeWire 图里数节点、按程序名合并（跟 Windows 侧语义对齐）。
pub fn registry() -> Arc<dyn DeviceRegistry> {
    Arc::new(vox_audio_linux::LinuxDeviceRegistry::new())
}

pub fn capture_factory() -> CaptureFactory {
    Box::new(|| Box::new(UnimplementedCapture) as Box<dyn CaptureSource>)
}

pub fn playback_factory() -> PlaybackFactory {
    Box::new(|| Box::new(UnimplementedPlayback) as Box<dyn PlaybackSink>)
}

const CAPTURE_TODO: &str = "Linux 音频采集尚未实现（P1：PipeWire 采集流 + 按程序建链）";
const PLAYBACK_TODO: &str = "Linux 音频播放尚未实现（P1：PipeWire 播放流）";

struct UnimplementedCapture;

impl CaptureSource for UnimplementedCapture {
    fn start(
        &mut self,
        _target: &CaptureTarget,
        _block_ms: u32,
        _on_chunk: Box<dyn FnMut(AudioChunk) + Send>,
    ) -> PortResult<CaptureFormat> {
        Err(PortError::new(CAPTURE_TODO))
    }

    fn stop(&mut self) {}
}

struct UnimplementedPlayback;

impl PlaybackSink for UnimplementedPlayback {
    fn open(&mut self, _device: Option<&str>, _source_rate: u32) -> PortResult<u32> {
        Err(PortError::new(PLAYBACK_TODO))
    }

    fn push(&mut self, _samples: &[f32]) {}

    fn flush(&mut self) {}

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_ports_fail_loudly() {
        let mut capture = UnimplementedCapture;
        let err = capture
            .start(&CaptureTarget::Microphone(None), 20, Box::new(|_| {}))
            .expect_err("占位实现必须报错，不能返回静默音频");
        assert!(
            err.message.contains("P1"),
            "错误里要指向待办：{}",
            err.message
        );

        let mut playback = UnimplementedPlayback;
        let err = playback
            .open(None, 24_000)
            .expect_err("占位实现必须报错，不能假装打开了设备");
        assert!(
            err.message.contains("P1"),
            "错误里要指向待办：{}",
            err.message
        );
    }
}
