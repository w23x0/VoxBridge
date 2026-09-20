//! 字形数据与光栅器接口（平台中立）。
//!
//! 只有数据形状和 trait 在这儿；**怎么把字变成覆盖率蒙版**是两个平台各自的事：
//! Windows 用 GDI（`vox-overlay-win/src/font.rs`），Linux 用 `swash` + `fontdb`。
//!
//! `Glyph` 的字段含义是从 GDI 那版照搬过来的（掩码左上角相对"起笔点 + 基线"的偏移、
//! 步进宽度、覆盖率），换光栅器时保持这套语义，布局代码就不用改。

use vox_core::ports::PortResult;

/// 一个字栅格化出来的结果。
#[derive(Debug, Clone)]
pub struct Glyph {
    /// 掩码宽。
    pub w: i32,
    /// 掩码高。
    pub h: i32,
    /// 掩码左上角相对"起笔点 + 基线"的偏移。
    pub off_x: i32,
    pub off_y: i32,
    /// 步进宽度（下一个字的起笔点要往右挪多少）。
    pub advance: i32,
    /// `w * h` 个覆盖率。
    pub cov: Vec<u8>,
}

/// 字体度量。
#[derive(Debug, Clone, Copy, Default)]
pub struct FontMetrics {
    pub line_height: i32,
    pub ascent: i32,
}

/// 光栅器：量字宽、取字形掩码。**只能在创建它的那个线程上用**（GDI 的 DC、
/// swash 的 cache 都不是线程安全的），悬浮窗本来就规定所有绘制在窗口线程上做。
pub trait GlyphSource {
    fn metrics(&self) -> FontMetrics;
    /// 单字步进宽度（像素）。
    fn advance(&mut self, ch: char) -> i32;
    /// 取字形；拿不到（字体里没有这个字）返回 `None`，调用方自己决定退什么。
    fn glyph(&mut self, ch: char) -> Option<&Glyph>;
    /// 一段文字的宽度。默认按 `advance` 累加——够用，光栅器想更准可以覆盖。
    fn measure(&mut self, text: &str) -> i32 {
        text.chars().map(|ch| self.advance(ch)).sum()
    }
}

/// 建字体的工厂。设置里的字体族/字号/DPI 一变就要重建光栅器（字形缓存跟着换）。
///
/// 是个 `Box<dyn Fn>` 而不是直接存光栅器：`Renderer::sync_font` 要能重建它，
/// 而重建的方式是平台相关的（Windows 换 HFONT、Linux 换 swash 的 FontRef）。
pub type FontFactory = Box<dyn Fn(&str, u32, u32) -> PortResult<Box<dyn GlyphSource>> + Send>;

/// 把平台光栅器包成工厂的小工具，省得每个调用点都写一遍闭包。
pub fn font_factory<F>(build: F) -> FontFactory
where
    F: Fn(&str, u32, u32) -> PortResult<Box<dyn GlyphSource>> + Send + 'static,
{
    Box::new(build)
}
