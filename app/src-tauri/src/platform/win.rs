//! Windows 侧的平台实现：全部是对现有 `-win` crate 的薄包装。
//!
//! 这里**不做任何判断**，只是把 `lib.rs` 认的那组函数名对着 Win32 实现接上。
//! 真正的活儿在 `crates/vox-audio-win`、`vox-input-win`、`vox-overlay-win` 里。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use vox_core::pipeline::{CaptureFactory, PlaybackFactory};
use vox_core::ports::{Clock, DeviceRegistry, HotkeyHost, PortResult, SecretStore};
use vox_core::runtime::Runtime;

use super::{GeometryCallback, VirtualDeviceStatus};
use crate::state::OverlayHandle;

/// 悬浮窗的具体句柄留一份：`SubtitleView` trait 上没有关闭方法（`shutdown` 是
/// `Overlay` 的固有方法），而 `AppState` 里存的是 trait object，退出时得靠这个关窗。
static OVERLAY: OnceLock<Arc<vox_overlay_win::Overlay>> = OnceLock::new();

/// 启动前的一次性短路：`--vox-restore-defaults` 只做默认设备写回就退出。
/// 这是 VB-CABLE 卸载流程的收尾手段，只在 Windows 上有意义。
pub fn pre_main() -> bool {
    vox_audio_win::restore_via_args_if_requested()
}

pub fn clock() -> Arc<dyn Clock> {
    Arc::new(crate::sys::clock::SystemClock::new())
}

pub fn secret_store(path: PathBuf) -> Arc<dyn SecretStore> {
    Arc::new(crate::sys::secrets::DpapiSecretStore::new(path))
}

pub fn alert(title: &str, body: &str) {
    crate::sys::fatal::alert(title, body);
}

pub fn capture_factory() -> CaptureFactory {
    // 采集用**严格版**：系统不支持进程环回就报错，不偷偷退化成整机环回。
    Box::new(|| Box::new(vox_audio_win::WinCapture::new()))
}

pub fn playback_factory() -> PlaybackFactory {
    Box::new(|| {
        let rf = crate::dsp::resample_factory();
        Box::new(vox_audio_win::WinPlayback::new(rf))
    })
}

pub fn registry() -> Arc<dyn DeviceRegistry> {
    Arc::new(vox_audio_win::WinDeviceRegistry::new())
}

/// 起热键线程。事件回调由 `vox-input-win` 直接推给 `Runtime::on_hotkey`。
pub fn start_hotkeys(runtime: Runtime) -> PortResult<Arc<dyn HotkeyHost>> {
    let listener = crate::input::start(runtime)?;
    Ok(listener)
}

pub fn stop_hotkeys() {
    crate::input::stop();
}

pub fn spawn_overlay(
    settings: &vox_core::settings::SubtitleSettings,
    on_geometry: GeometryCallback,
) -> PortResult<OverlayHandle> {
    let overlay = vox_overlay_win::Overlay::spawn_with_geometry(settings, Some(on_geometry))?;
    let _ = OVERLAY.set(Arc::clone(&overlay));
    Ok(overlay)
}

/// 悬浮窗还活着吗。帧线程靠它发现"窗口自己没了"（比如 Windows 上用户把窗关了）。
pub fn overlay_running() -> bool {
    OVERLAY.get().is_some_and(|overlay| overlay.is_running())
}

pub fn shutdown_overlay() {
    if let Some(overlay) = OVERLAY.get() {
        overlay.shutdown();
    }
}

/// 透明无边框窗口下 `tauri.conf.json` 的 `minHeight` 压不住（实测会被压到 ~30px），
/// `set_min_size` 走 tao 的 subclass 链同样压不到，所以在 `WM_GETMINMAXINFO`
/// 最底层强制最小尺寸。
pub fn enforce_min_size(window: &tauri::WebviewWindow) {
    if let Ok(hwnd) = window.hwnd() {
        crate::winminmax::enforce_min_size(hwnd.0, 640, 38);
    }
}

pub fn virtual_device_status() -> VirtualDeviceStatus {
    let status = match vox_audio_win::cable::detect() {
        vox_audio_win::CableStatus::Installed => "installed",
        vox_audio_win::CableStatus::InstalledPendingReboot => "install_pending_reboot",
        vox_audio_win::CableStatus::UninstallIncomplete => "uninstall_incomplete",
        vox_audio_win::CableStatus::NotInstalled => "not_installed",
    };
    let multichannel = match vox_audio_win::multichannel_endpoint_status() {
        vox_audio_win::MultichannelEndpointStatus::Enabled => "visible",
        vox_audio_win::MultichannelEndpointStatus::Disabled => "hidden",
        vox_audio_win::MultichannelEndpointStatus::NotPresent => "absent",
    };
    VirtualDeviceStatus {
        status,
        multichannel_status: multichannel,
    }
}

/// 启动时要提醒用户的事。Windows 上没有额外前置检查（VB-CABLE 缺失由界面引导）。
pub fn startup_notes() -> Vec<String> {
    Vec::new()
}
