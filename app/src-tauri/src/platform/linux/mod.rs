//! Linux 侧的平台实现。
//!
//! 现状（P0 收尾）：时钟、密钥库、设备目录已经是真实现；音频采集/播放、全局热键、
//! 悬浮字幕窗还没写（P1/P2/P3）。**没写的部分一律返回明确错误，不静默降级**：
//! 用户按下"对外说话"就该看到"这个平台还没做好"，绝不能让他以为自己已经开麦了。

mod audio;
mod clock;
mod secrets;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use vox_core::pipeline::{CaptureFactory, PlaybackFactory};
use vox_core::ports::{Clock, DeviceRegistry, HotkeyHost, PortResult, SecretStore};
use vox_core::runtime::Runtime;
use vox_core::settings::SubtitleSettings;

use super::{GeometryCallback, VirtualDeviceStatus};
use crate::state::OverlayHandle;

/// 悬浮窗的具体句柄留一份：装配层 `AppState` 里存的是 trait object，
/// 而关窗/查存活是 `Overlay` 的固有方法。
static OVERLAY: OnceLock<Arc<vox_overlay_linux::Overlay>> = OnceLock::new();

/// 热键监听句柄留一份，退出时显式停（理由同 Windows 侧：不能指望 Drop）。
static HOTKEYS: OnceLock<Arc<vox_input_linux::HotkeyListener>> = OnceLock::new();

/// 启动前短路：只有 Windows 那条 VB-CABLE 默认设备写回走这条路。
///
/// Linux 这边顺手做一件必须**在 GTK 初始化之前**做的事：GNOME 的 Wayland 会话下
/// GTK 客户端不能自定坐标、不能置顶（协议层就没有），而 XWayland 下两样都成立
/// （实测见 `docs/PLATFORM_LINUX.md` §2.3）。所以 Wayland 会话里把整个应用切到
/// X11 后端；已经有 `DISPLAY` 才切，没有就保持原样（那样悬浮窗会由合成器摆位）。
pub fn pre_main() -> bool {
    let wayland = std::env::var("XDG_SESSION_TYPE")
        .map(|value| value.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false);
    let has_x11 = std::env::var_os("DISPLAY").is_some();
    if wayland && has_x11 && std::env::var_os("GDK_BACKEND").is_none() {
        // SAFETY：单线程启动早期设置环境变量，GTK 还没初始化，也没有别的线程读它。
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
        tracing::info!("Wayland 会话：切到 X11 后端（XWayland），悬浮窗才能定位置顶");
    }
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

/// 起热键监听（evdev 直读 `/dev/input`）。
///
/// 权限不足时返回的错误里带着**能照做的命令**（`usermod -aG input`），装配层会把它
/// 翻成界面上的 `Notice`——用户看到的是"怎么修"，而不是按了热键没反应。
pub fn start_hotkeys(runtime: Runtime) -> PortResult<Arc<dyn HotkeyHost>> {
    let listener = vox_input_linux::HotkeyListener::start(
        vox_core::ports::HotkeyBindings::default(),
        Box::new(move |event| runtime.on_hotkey(event)),
    )?;
    let _ = HOTKEYS.set(Arc::clone(&listener));
    Ok(listener)
}

pub fn stop_hotkeys() {
    if let Some(listener) = HOTKEYS.get() {
        listener.stop();
    }
}

/// 起悬浮字幕窗。GTK 只能在主线程建窗，而装配层的 `assemble()` 就在主线程，
/// 所以这里直接建；不是主线程会拿到明确错误（见 `window.rs`）。
pub fn spawn_overlay(
    settings: &SubtitleSettings,
    _on_geometry: GeometryCallback,
) -> PortResult<OverlayHandle> {
    // 几何回调先不接：Linux 侧是永久鼠标穿透（`DECISIONS.md` A5），窗口拖不动，
    // 也就没有"用户改了几何"这回事。
    let overlay = vox_overlay_linux::spawn(settings)?;
    let _ = OVERLAY.set(Arc::clone(&overlay));
    Ok(overlay)
}

/// 窗口还活着吗。帧线程靠它发现"窗被关了"。
pub fn overlay_running() -> bool {
    OVERLAY.get().is_some_and(|overlay| overlay.is_running())
}

pub fn shutdown_overlay() {
    if let Some(overlay) = OVERLAY.get() {
        overlay.shutdown();
    }
}

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
    if !tray_host_available() {
        notes.push(format!(
            "系统托盘不会显示图标：关窗会最小化，不会收进托盘。{}",
            tray_missing_hint()
        ));
    }
    notes
}

/// 有没有东西在显示托盘图标。
///
/// 只问 D-Bus 上那两个标准名字在不在（GNOME 的 AppIndicator 扩展占 `org.kde.…`，
/// KDE 自己那套平时不跑、注册图标时靠 D-Bus 激活拉起来，所以要连"可激活"一起算）。
/// **不能**只看 `TrayIconBuilder::build()` 成没成功：没有宿主时它照样返回 Ok，
/// 图标对象建出来了但没有任何东西会画它——用户关窗之后就再也叫不回界面。
pub fn tray_host_available() -> bool {
    /// StatusNotifier 规范里的宿主名字。KDE 与 Ayatana 两套实现各占一个。
    const WATCHERS: [&str; 2] = [
        "org.kde.StatusNotifierWatcher",
        "org.ayatana.StatusNotifierWatcher",
    ];

    // 拿不到会话总线（比如从 ssh/无桌面环境里跑）就是没有宿主。
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(bus) = zbus::blocking::fdo::DBusProxy::new(&connection) else {
        return false;
    };
    if WATCHERS.iter().any(|name| has_owner(&bus, name)) {
        return true;
    }
    bus.list_activatable_names()
        .map(|names| {
            names
                .iter()
                .any(|name| WATCHERS.iter().any(|watcher| name.as_str() == *watcher))
        })
        .unwrap_or(false)
}

/// 托盘看不见时给用户的解释。
pub fn tray_missing_hint() -> &'static str {
    "GNOME 默认不带系统托盘，装上 AppIndicator/KStatusNotifier 扩展后图标才会出现。"
}

fn has_owner(bus: &zbus::blocking::fdo::DBusProxy, name: &str) -> bool {
    let Ok(name) = zbus::names::BusName::try_from(name) else {
        return false;
    };
    bus.name_has_owner(name).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_host_probe_never_panics() {
        // 有宿主（本机 GNOME + AppIndicator 扩展）→ true；没有会话总线（CI 容器）
        // → false。两种都算对，这里只钉住"不许 panic、不许卡住"。
        let _ = tray_host_available();
    }

    #[test]
    fn hint_is_not_empty() {
        assert!(!tray_missing_hint().is_empty());
    }
}
