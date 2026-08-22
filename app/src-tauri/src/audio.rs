//! 音频端口的实现：全部来自 `vox-audio-win`。
//!
//! 三个工厂都只是 `new()` 的包装——真正的 WASAPI 活儿在那个 crate 里。
//! 采集用**严格版** `WinCapture::new()`：系统不支持进程环回时直接报错，
//! 不偷偷退化成整机环回。抓错声音（把用户自己的麦也翻译一遍）比报错更糟。

use std::sync::Arc;

use vox_core::pipeline::{CaptureFactory, PlaybackFactory};
use vox_core::ports::DeviceRegistry;

pub fn capture_factory() -> CaptureFactory {
    Box::new(|| Box::new(vox_audio_win::WinCapture::new()))
}

pub fn playback_factory() -> PlaybackFactory {
    Box::new(|| {
        let rf = crate::dsp::resample_factory();
        Box::new(vox_audio_win::WinPlayback::new(rf))
    })
}

/// 设备枚举。无状态，全进程共用一个。
pub fn registry() -> Arc<dyn DeviceRegistry> {
    Arc::new(vox_audio_win::WinDeviceRegistry::new())
}
