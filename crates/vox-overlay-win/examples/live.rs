//! 真开一个纯显示悬浮窗，灌假字幕进去，手动看效果。
//!
//! ```text
//! cargo run -p vox-overlay-win --example live        # 默认 120 秒
//! cargo run -p vox-overlay-win --example live -- 15  # 只跑 15 秒
//! ```
//!
//! 检查点：空白时鼠标穿透、有字幕时可以拖动/缩放、字幕后面没有实心方块、
//! 两行分色且逐字淡出。
//!
//! 只在 Windows 上有意义（`vox-overlay-win` 在别的平台是空 lib）。

#[cfg(windows)]
#[path = "live/windows.rs"]
mod windows;

#[cfg(windows)]
fn main() {
    windows::main();
}

#[cfg(not(windows))]
fn main() {}
