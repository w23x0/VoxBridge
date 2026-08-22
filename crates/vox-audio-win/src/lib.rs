//! Windows 音频 I/O：麦克风采集、进程环回采集、按设备名输出、VB-CABLE 检测与安装。
//!
//! 实现 `vox_core::ports` 里的 `CaptureSource` / `PlaybackSink` / `DeviceRegistry`。
//!
//! 几条贯穿全 crate 的规矩：
//! - 音频回调线程只做“搬数据”，不分配、不加锁、不打日志（见 ARCHITECTURE.md §6）；
//! - 每个自己开的线程自己初始化 COM（MTA），退出时反初始化；
//! - COM 错误一律翻成带 HRESULT 的中文 `PortError`，永不 panic；
//! - 库代码里没有 `unwrap()` / `expect()`，测试里可以有。

pub mod cable;
mod capture;
mod client;
mod com;
mod devices;
mod osver;
mod playback;
mod policy;
mod proc;
mod rates;
mod registry;
mod resample;
mod ring;
mod sessions;
mod wave;

pub use cable::{
    multichannel_endpoint_status, set_multichannel_endpoint_enabled, uninstall_with_audio_reset,
    CableStatus, DownloadOutcome, EndpointToggleOutcome, InstallOutcome,
    MultichannelEndpointStatus, ProductDisclosure, DONATION_URL, PRODUCT_NAME, PRODUCT_URL,
    UNINSTALL_ARGS,
};
pub use capture::{EndpointLoopbackCapture, WinCapture};
// 只重导出 examples/smoke.rs 和 app/src-tauri 实际用到的 osver 入口。
// process_loopback_supported / MIN_PROCESS_LOOPBACK_BUILD 仅 crate 内部用，不对外暴露。
pub use osver::{os_build_number, process_loopback_available};
pub use playback::WinPlayback;
pub use policy::{
    capture_default_endpoints, elevate_and_restore_defaults, restore_via_args_if_requested,
};
pub use registry::WinDeviceRegistry;

/// 当前仍持有 VB-CABLE 播放/录音会话的应用。调用方无需预先初始化 COM。
pub fn virtual_cable_blocking_apps() -> vox_core::ports::PortResult<Vec<vox_core::ports::AudioApp>>
{
    let _com = com::ComGuard::mta()?;
    sessions::cable_sessions()
}
