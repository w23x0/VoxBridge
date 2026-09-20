//! 悬浮字幕窗的**平台中立**部分：几何、配色、CPU 画布、布局、帧合成。
//!
//! Windows（`vox-overlay-win` 的 Win32 分层窗）和 Linux（`vox-overlay-linux` 的
//! GTK 窗）共用这一份渲染逻辑，各自只提供两样平台相关的东西：
//!
//! | 平台相关 | 谁提供 |
//! | --- | --- |
//! | 字形光栅器（量字宽、出覆盖率蒙版） | Windows：GDI `ExtTextOutW`；Linux：`swash`/`fontdb` |
//! | 窗口 + 呈现（把画好的 BGRA 贴到屏幕上） | Windows：`UpdateLayeredWindow`；Linux：GTK + cairo |
//!
//! 渲染器只认 [`text::GlyphSource`] 这个 trait，所以两边能共用同一套布局与动画
//! （逐字淡出、换行上移、双行分色都是这里算的）。
//!
//! 这条分层不是事后补的：`canvas.rs` / `geom.rs` / `layout.rs` 从写下来那天起就是
//! 纯运算、能脱离桌面单测（见各文件头的说明），搬到共享 crate 只是把事实摆正。

pub mod canvas;
pub mod color;
pub mod geom;
pub mod layout;
pub mod render;
pub mod text;
