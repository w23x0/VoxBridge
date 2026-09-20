//! 平台分派：装配层只跟这一层打交道。
//!
//! `lib.rs` 的装配流程本身是平台无关的——建 Runtime、注端口、起线程、挂事件桥。
//! 平台差异全部收在这里：
//!
//! | 能力 | Windows | Linux |
//! | --- | --- | --- |
//! | 时钟 | `GetLocalTime` | `chrono::Local` |
//! | 密钥库 | DPAPI 加密落盘 | Secret Service（`keyring`） |
//! | 启动期致命提示 | `MessageBoxW` | stderr + 日志 |
//! | 音频三件套 | WASAPI（`vox-audio-win`） | PipeWire（`vox-audio-linux`） |
//! | 全局热键 | `GetAsyncKeyState` 轮询 | evdev |
//! | 悬浮字幕窗 | Win32 分层窗 | GTK + XWayland |
//! | 虚拟麦克风 | VB-CABLE（要装） | PipeWire sink（原生，不需要装） |
//! | 托盘宿主 | 通知区域永远在 → `true` | 问 D-Bus 有没有 StatusNotifier 宿主 |
//!
//! 两边都实现同一组函数，`lib.rs` 不写 `#[cfg]`。
//!
//! 托盘那一行是后加的：`TrayIconBuilder::build()` 在"没人显示托盘"的环境下照样返回
//! Ok（GNOME 默认就是这样），所以"建成功"不足以决定关窗要不要 `hide()`。

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

/// 虚拟麦克风在界面上要显示的状态。
///
/// 三个平台事实不同，字段就按"界面要显示什么"定，而不是按谁的系统 API 长什么样：
/// - Windows 上 VB-CABLE 是需要用户**安装**的第三方驱动，所以有"装了没/要不要重启"；
/// - Linux 上 PipeWire 原生就能建虚拟 sink，没有"安装"这一步，状态恒为 `not_applicable`。
pub struct VirtualDeviceStatus {
    /// 前端认的值：`installed` / `install_pending_reboot` / `uninstall_incomplete` /
    /// `not_installed` / `not_applicable`（Linux）。`not_applicable` 时界面应当
    /// 整块隐藏 VB-CABLE 管理页。
    pub status: &'static str,
    pub multichannel_status: &'static str,
}

/// 用户在悬浮窗上拖完/缩放完，把新几何回写设置时用。
pub type GeometryCallback =
    std::sync::Arc<dyn Fn(vox_core::settings::OverlayGeometry) + Send + Sync + 'static>;
