//! WASAPI 共享模式的低延迟初始化。
//!
//! Windows 10+ 优先走 IAudioClient3 的最短共享周期；查询不到或初始化失败时，
//! 调用方应重新激活一个 IAudioClient，再走 `initialize_default_period`。失败过的
//! IAudioClient 不能可靠地重复 Initialize，所以这里不在同一实例上重试。

use windows::Win32::Media::Audio::{
    IAudioClient, IAudioClient3, AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX,
};
use windows_core::Interface;

/// 尝试最短共享周期。返回实际请求的周期帧数。
pub(crate) unsafe fn initialize_min_period(
    client: &IAudioClient,
    stream_flags: u32,
    format: *const WAVEFORMATEX,
) -> windows_core::Result<u32> {
    let client3: IAudioClient3 = client.cast()?;
    let mut default_period = 0;
    let mut fundamental_period = 0;
    let mut min_period = 0;
    let mut max_period = 0;
    unsafe {
        client3.GetSharedModeEnginePeriod(
            format,
            &mut default_period,
            &mut fundamental_period,
            &mut min_period,
            &mut max_period,
        )?;
    }
    let period = min_period.max(fundamental_period).max(1);
    unsafe {
        client3.InitializeSharedAudioStream(stream_flags, period, format, None)?;
    }
    Ok(period)
}

/// 兼容路径：让系统按默认引擎周期分配最小共享缓冲。
pub(crate) unsafe fn initialize_default_period(
    client: &IAudioClient,
    stream_flags: u32,
    format: *const WAVEFORMATEX,
) -> windows_core::Result<()> {
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, stream_flags, 0, 0, format, None) }
}
