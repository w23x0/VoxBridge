//! 全局热键：直接读 `/dev/input/event*`（evdev）。
//!
//! 为什么只能这样：GNOME 不支持 `org.freedesktop.portal.GlobalShortcuts`（KDE 支持），
//! XWayland 下 `XGrabKey` 只在有 X11 窗口拿到焦点时有效，两者都不是"全局"。
//! 直接读内核输入事件是唯一稳的路，顺带解决两件事：
//!
//! - **键盘侧键**（`BTN_SIDE` / `BTN_EXTRA`，对应 Windows 的 XButton1/2）；
//! - **按住说话需要"松开"事件**——事件驱动天然有。
//!
//! 权限：`/dev/input/event*` 是 `root:input 0660`，用户得在 `input` 组里
//! （`sudo usermod -aG input $USER` 之后重新登录）。没权限时**明确报错**并给出这条命令，
//! 不静默失败——用户按了热键没反应还以为是 bug。

mod codes;
mod hotkey;

pub use hotkey::HotkeyListener;
