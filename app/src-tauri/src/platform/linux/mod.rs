//! Linux 侧的平台实现。
//!
//! 现状（P0 收尾）：时钟、密钥库、设备目录已经是真实现；音频采集/播放、全局热键、
//! 悬浮字幕窗还没写（P1/P2/P3）。**没写的部分一律返回明确错误，不静默降级**：
//! 用户按下"对外说话"就该看到"这个平台还没做好"，绝不能让他以为自己已经开麦了。

mod audio;
mod clock;
mod secrets;

use std::path::PathBuf;
use std::sync::Arc;

use vox_core::pipeline::{CaptureFactory, PlaybackFactory};
use vox_core::ports::{Clock, DeviceRegistry, HotkeyHost, PortError, PortResult, SecretStore};
use vox_core::runtime::Runtime;
use vox_core::settings::SubtitleSettings;

use super::{GeometryCallback, VirtualDeviceStatus};
use crate::state::OverlayHandle;

/// 启动前短路：只有 Windows 那条 VB-CABLE 默认设备写回走这条路。
pub fn pre_main() -> bool {
    false
}

pub fn clock() -> Arc<dyn Clock> {
    Arc::new(clock::LocalClock::new())
}

pub fn secret_store(_path: PathBuf) -> Arc<dyn SecretStore> {
    // Linux 不用文件存密钥：走 Secret Service（gnome-keyring / KWallet）。
    // 参数保留是为了跟 Windows 那边签名一致。
    Arc::new(secrets::SecretServiceStore::new())
}

pub fn alert(title: &str, body: &str) {
    // 没有 MessageBoxW：发布构建在 Linux 上是从终端或 .desktop 起的，stderr 看得见。
    eprintln!("{title}\n{body}");
    tracing::error!("{title}：{body}");
}

pub fn capture_factory() -> CaptureFactory {
    audio::capture_factory()
}

pub fn playback_factory() -> PlaybackFactory {
    audio::playback_factory()
}

pub fn registry() -> Arc<dyn DeviceRegistry> {
    audio::registry()
}

/// 全局热键未实现（P3：evdev 监听 `/dev/input`）。
///
/// 这里**故意返回错误**：装配层会把它翻成界面上的 `Notice`，用户能看到
/// "只能用界面上的开关"，而不是按了热键毫无反应还以为是 bug。
pub fn start_hotkeys(_runtime: Runtime) -> PortResult<Arc<dyn HotkeyHost>> {
    Err(PortError::new(
        "Linux 全局热键尚未实现（P3：读 /dev/input 的 evdev 监听）",
    ))
}

pub fn stop_hotkeys() {}

/// 悬浮字幕窗未实现（P2：GTK + XWayland）。
pub fn spawn_overlay(
    _settings: &SubtitleSettings,
    _on_geometry: GeometryCallback,
) -> PortResult<OverlayHandle> {
    Err(PortError::new(
        "Linux 悬浮字幕窗尚未实现（P2：GTK + XWayland）",
    ))
}

/// 悬浮窗本来就没起起来，帧线程不会跑，所以这里恒为 false。
pub fn overlay_running() -> bool {
    false
}

pub fn shutdown_overlay() {}

/// GTK 自己就认 `tauri.conf.json` 的 `minWidth` / `minHeight`，不需要 Windows 那套
/// `WM_GETMINMAXINFO` 子类化。P2 真机验收时要确认这一点。
pub fn enforce_min_size(_window: &tauri::WebviewWindow) {}

pub fn virtual_device_status() -> VirtualDeviceStatus {
    // PipeWire 原生就能建虚拟 sink，没有"安装"这一步：前端看到
    // `not_applicable` 就整块隐藏 VB-CABLE 管理页，只留一句"去目标程序里选虚拟麦"。
    VirtualDeviceStatus {
        status: "not_applicable",
        multichannel_status: "absent",
    }
}

pub fn startup_notes() -> Vec<String> {
    let mut notes = Vec::new();
    if !vox_audio_linux::pipewire_available() {
        notes.push(
            "没连上 PipeWire：Linux 音频后端以 PipeWire 为前提（主流发行版默认就有）。".to_string(),
        );
    }
    notes
}
