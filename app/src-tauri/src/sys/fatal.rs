//! 启动期致命错误的最后一道提示。
//!
//! 发布构建是 `windows_subsystem = "windows"`：没有控制台，stderr 进黑洞，
//! panic 信息用户一个字也看不到。如果 `assemble()` 失败就直接 panic，用户看到的
//! 只是"双击了图标，什么都没发生"——没有窗口、没有托盘、没有报错，无从下手。
//!
//! 所以这里用 `MessageBoxW` 弹一个模态框把原因摆出来。只在启动路径用，
//! 运行期的问题走 `Notice` 显示在界面里，不该拿模态框打断用户。

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_SYSTEMMODAL,
};

/// 弹一个错误框。`MB_SYSTEMMODAL | MB_SETFOREGROUND` 保证它不会被压在别的窗口
/// 底下——这时候我们还没有主窗口，压下去就等于没弹。
pub fn alert(title: &str, body: &str) {
    let title = HSTRING::from(title);
    let body = HSTRING::from(body);
    // SAFETY：两个 HSTRING 活过整个调用；hwnd 传 None 表示无属主窗口。
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SYSTEMMODAL | MB_SETFOREGROUND,
        );
    }
}
