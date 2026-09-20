//! 离屏自检：不开窗口，用跟真窗口**完全相同**的 `Renderer` + `Canvas` 画出像素，
//! 合成到刻意刁难的背景上写成 PNG，然后人眼过一遍。
//!
//! 背景为什么这么选、要盯什么，见 `snapshot/windows.rs`。
//!
//! 跑法：`cargo run -p vox-overlay-win --example snapshot`
//! 产物：`target/overlay-snapshots/*.png`
//!
//! 只在 Windows 上有意义（`vox-overlay-win` 在别的平台是空 lib）。

#[cfg(windows)]
#[path = "snapshot/windows.rs"]
mod windows;

#[cfg(windows)]
fn main() {
    windows::main();
}

#[cfg(not(windows))]
fn main() {}
