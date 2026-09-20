//! Linux 悬浮字幕窗：GTK 透明窗 + XWayland 定位置顶 + swash 字形光栅。
//!
//! 实现 `vox_core::ports::SubtitleView`。渲染逻辑在平台中立的 `vox-overlay-core`
//! 里，这里只做两件平台相关的事：**字形光栅**（`font.rs`，swash）与**窗口+呈现**
//! （`window.rs`，GTK + cairo）。
//!
//! 非 Linux 上编译成空 lib，理由同其它 `-win`/`-linux` 兄弟。

#![cfg(target_os = "linux")]

pub mod font;

pub use font::font_factory;
