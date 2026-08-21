//! 热键线程：把 `vox-input-win` 的按键事件转成 `Runtime::on_hotkey`。
//!
//! 线程在 `HotkeyListener::start` 里自己起（25 ms 轮询 `GetAsyncKeyState`），
//! 这里只负责搭桥。初始绑定不用我们灌——`Runtime::set_hotkey_host` 自己会
//! 立刻 `rebind_hotkeys()` 一次，绑定清单只由内核一处生成（坑 6）。

use std::sync::{Arc, OnceLock};

use vox_core::ports::{HotkeyBindings, PortResult};
use vox_core::runtime::Runtime;
use vox_input_win::HotkeyListener;

/// 留一份句柄给 `stop()`。
///
/// 不能指望 `Drop` 来收：句柄同时被内核的 `Inner.hotkeys` 持着，而内核的
/// `Inner` 又被 listener 闭包间接持着，退出时 `Drop` 链不一定跑得到（见
/// events.rs 里 `Weak` 的说明）。退出必须显式停——否则热键线程会在
/// `persist.flush()` 之后还在轮询，用户松手/误按就把账本改脏，那份改动永远
/// 不落盘；更糟的是它能在进程退出中途 `toggle()` 拉起新的工作线程去开麦。
static LISTENER: OnceLock<Arc<HotkeyListener>> = OnceLock::new();

pub fn start(runtime: Runtime) -> PortResult<Arc<HotkeyListener>> {
    let listener = HotkeyListener::start(
        HotkeyBindings::default(),
        Box::new(move |event| runtime.on_hotkey(event)),
    )?;
    let _ = LISTENER.set(Arc::clone(&listener));
    Ok(listener)
}

/// 停热键线程并等它退出（最多一个 25 ms 轮询周期）。可重复调用。
pub fn stop() {
    if let Some(l) = LISTENER.get() {
        l.stop();
    }
}
