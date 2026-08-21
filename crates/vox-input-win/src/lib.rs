//! 全局热键轮询。
//!
//! 实现 `vox_core::ports` 里的 `HotkeyHost`。
//! 热键线程每 25 ms 用 `GetAsyncKeyState` 轮询，只取高位判断当前状态，
//! 自己跟踪边沿——不用 RegisterHotKey（拿不到 release），也不用低级钩子
//! （需要消息泵且被反作弊标记）。

mod edge;
mod hotkey;

pub use hotkey::HotkeyListener;
