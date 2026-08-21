//! GDI 字形栅格化：把一个字画成 8 位覆盖率掩码，交给 `Canvas` 自己合成。
//!
//! 为什么不让 GDI 直接往分层窗的位图上写字：GDI 完全不管 alpha 通道。
//! 开抗锯齿时它把字的边缘跟位图里**已有的颜色**混（我们的位图是全透明黑），
//! 于是半覆盖的边缘像素变成"一半文字色 + 一半黑"、alpha 却还是 0 或者被原样留着，
//! 贴到屏幕上就是一圈黑边——中日韩字笔画多、边缘像素占比高，黑边尤其明显。
//!
//! 所以走覆盖率路线：白字画在纯黑底上，读回任一颜色通道就是 0..255 的覆盖率，
//! 再由 `Canvas::blend_mask` 算 `alpha = 覆盖率 × 逐字 alpha` 并**手工预乘**文字色。
//! GDI 从头到尾没碰过真正要上屏的那块位图的 alpha。
//!
//! 另外故意用 `ANTIALIASED_QUALITY` 而不是 ClearType：ClearType 的 RGB 三个通道
//! 是三份不同的次像素覆盖率，压不进单一 alpha 通道，硬用就会出彩边。

use std::collections::HashMap;

use vox_core::ports::{PortError, PortResult};
use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DeleteDC, DeleteObject, ExtTextOutW,
    GdiFlush, GetTextExtentPoint32W, GetTextMetricsW, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, ETO_OPAQUE, FF_DONTCARE, FW_DEMIBOLD, HBITMAP,
    HDC, HFONT, HGDIOBJ, LOGFONTW, OPAQUE, OUT_TT_PRECIS, TEXTMETRICW,
};

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

/// 字形掩码不是无限缓存：长时间跑多语种字幕时，不同字符可能持续增长。
const MAX_CACHED_GLYPHS: usize = 2048;

struct CachedGlyph {
    glyph: Glyph,
    last_used: u64,
}

/// 字体度量。
#[derive(Debug, Clone, Copy, Default)]
pub struct FontMetrics {
    pub line_height: i32,
    pub ascent: i32,
}

/// GDI 字体 + 一块用来画字的临时位图 + 字形缓存。
///
/// 只能在创建它的那个线程上用：GDI 的 DC 不是线程安全的，而且悬浮窗本来就规定
/// 所有绘制都在窗口线程上做。
pub struct FontRaster {
    dc: HDC,
    font: HFONT,
    old_font: HGDIOBJ,
    /// 临时位图：画字用的画布，宽高够装下最大的一个字。
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u8,
    scratch_w: i32,
    scratch_h: i32,
    metrics: FontMetrics,
    /// 字距：旧版给了 101.5% 的字间距，这里折算成每个字多出来的像素。
    tracking: i32,
    cache: HashMap<char, CachedGlyph>,
    cache_clock: u64,
}

impl FontRaster {
    /// 建一个字体。`size_pt` 是设置里的字号，`dpi` 是窗口所在显示器的 DPI。
    ///
    /// 字号按 DPI 换算：`px = pt * dpi / 72`。进程的 DPI 感知由外层 Tauri 应用设置，
    /// 这里只查不假设，所以传进来的 `dpi` 必须是 `GetDpiForWindow` 的实测值。
    pub fn new(family: &str, size_pt: u32, dpi: u32) -> PortResult<Self> {
        let dpi = if (72..=1200).contains(&dpi) { dpi } else { 96 };
        let px = ((size_pt.max(1) as i64 * dpi as i64) / 72).max(1) as i32;

        // SAFETY: CreateCompatibleDC(None) 用屏幕 DC 做模板，失败返回空句柄，下面立刻查。
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(PortError::new("创建字体用的内存 DC 失败"));
        }

        let mut lf = LOGFONTW {
            // 负值表示"字符高度"而不是"含行距的单元格高度"，这样字号跟视觉大小对得上。
            lfHeight: -px,
            lfWeight: FW_DEMIBOLD.0 as i32,
            lfCharSet: DEFAULT_CHARSET,
            lfOutPrecision: OUT_TT_PRECIS,
            lfClipPrecision: CLIP_DEFAULT_PRECIS,
            // 逐字 alpha 要求单通道覆盖率，ClearType 的次像素覆盖率表达不了。
            lfQuality: ANTIALIASED_QUALITY,
            lfPitchAndFamily: DEFAULT_PITCH.0 | FF_DONTCARE.0,
            ..Default::default()
        };
        write_face_name(&mut lf.lfFaceName, family);

        // SAFETY: lf 是本地栈上的完整结构，CreateFontIndirectW 只读不留引用。
        let font = unsafe { CreateFontIndirectW(&lf) };
        if font.is_invalid() {
            // SAFETY: dc 是上面刚建的有效 DC，出错路径上必须还回去。
            unsafe {
                let _ = DeleteDC(dc);
            }
            return Err(PortError::new(format!(
                "创建字体失败: {family} {size_pt}pt"
            )));
        }
        // SAFETY: dc 和 font 都有效；返回的旧字体句柄留着，销毁前要还原。
        let old_font = unsafe { SelectObject(dc, font.into()) };

        let mut tm = TEXTMETRICW::default();
        // SAFETY: dc 里已经选好字体，tm 是本地可写结构。
        let ok = unsafe { GetTextMetricsW(dc, &mut tm) }.as_bool();
        if !ok {
            // SAFETY: 逐个还原并释放刚建的 GDI 对象。
            unsafe {
                SelectObject(dc, old_font);
                let _ = DeleteObject(font.into());
                let _ = DeleteDC(dc);
            }
            return Err(PortError::new("读取字体度量失败"));
        }
        let metrics = FontMetrics {
            line_height: (tm.tmHeight + tm.tmExternalLeading).max(1),
            ascent: tm.tmAscent.max(0),
        };

        // 临时位图留足余量：中日韩字最宽也就一个字身，两倍字身足够容纳抗锯齿溢出和重音。
        let scratch_w = (px * 3).max(16);
        let scratch_h = (metrics.line_height * 2 + px).max(16);
        let mut raster = Self {
            dc,
            font,
            old_font,
            bitmap: HBITMAP::default(),
            old_bitmap: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            scratch_w,
            scratch_h,
            metrics,
            // 101.5% 字距：不足 1 px 时算 0，不硬凑。
            tracking: (px as f32 * 0.015).round() as i32,
            cache: HashMap::new(),
            cache_clock: 0,
        };
        raster.create_scratch()?;
        Ok(raster)
    }

    fn create_scratch(&mut self) -> PortResult<()> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.scratch_w,
                // 负高度 = 自上而下排布，行序跟 Canvas 一致，省一次翻转。
                biHeight: -self.scratch_h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: info 描述的就是下面要用的尺寸；ppvbits 收到的指针由这块位图拥有，
        // 生命周期跟 self.bitmap 绑定，Drop 里一起释放。
        let bitmap =
            unsafe { CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| PortError::new(format!("创建字形临时位图失败: {e}")))?;
        if bits.is_null() {
            // SAFETY: bitmap 刚建成功，这条路径上要还回去。
            unsafe {
                let _ = DeleteObject(bitmap.into());
            }
            return Err(PortError::new("字形临时位图没有返回像素指针"));
        }
        // SAFETY: dc 有效，bitmap 刚建好且尚未被选进任何 DC。
        let old_bitmap = unsafe { SelectObject(self.dc, bitmap.into()) };
        self.bitmap = bitmap;
        self.old_bitmap = old_bitmap;
        self.bits = bits.cast();

        // SAFETY: dc 里已选好位图和字体，下面三个 Set* 只改 DC 状态。
        unsafe {
            SetBkMode(self.dc, OPAQUE);
            SetBkColor(self.dc, COLORREF(0x0000_0000));
            SetTextColor(self.dc, COLORREF(0x00ff_ffff));
        }
        Ok(())
    }

    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// 量一串字的宽度（含字距）。
    pub fn measure(&mut self, text: &str) -> i32 {
        text.chars().map(|c| self.advance(c)).sum()
    }

    /// 量单个字的步进。
    pub fn advance(&mut self, ch: char) -> i32 {
        match self.glyph(ch) {
            Some(g) => g.advance,
            None => 0,
        }
    }

    /// 取一个字的覆盖率掩码，失败返回 `None`（这一个字不画，别拖垮整帧）。
    pub fn glyph(&mut self, ch: char) -> Option<&Glyph> {
        // 控制字符统一当空格，免得跑出诡异的字形或者负宽度。
        let ch = if ch.is_control() { ' ' } else { ch };
        self.cache_clock = self.cache_clock.wrapping_add(1);
        let now = self.cache_clock;
        if self.cache.contains_key(&ch) {
            let cached = self.cache.get_mut(&ch)?;
            cached.last_used = now;
            return Some(&cached.glyph);
        }

        let glyph = self.rasterize(ch)?;
        if self.cache.len() >= MAX_CACHED_GLYPHS {
            let oldest = self
                .cache
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(&key, _)| key);
            if let Some(oldest) = oldest {
                self.cache.remove(&oldest);
            }
        }
        self.cache.insert(
            ch,
            CachedGlyph {
                glyph,
                last_used: now,
            },
        );
        self.cache.get(&ch).map(|cached| &cached.glyph)
    }

    fn rasterize(&mut self, ch: char) -> Option<Glyph> {
        let mut utf16 = [0u16; 2];
        let units: &[u16] = ch.encode_utf16(&mut utf16);

        let mut size = SIZE::default();
        // SAFETY: dc 里字体已选好；units 是本地缓冲的切片，长度由 encode_utf16 保证。
        let ok = unsafe { GetTextExtentPoint32W(self.dc, units, &mut size) }.as_bool();
        if !ok {
            return None;
        }
        let advance = size.cx.max(0) + self.tracking;

        // 空白字符没有可见像素，直接给个零面积掩码，省掉一次栅格化。
        if size.cx <= 0 || ch == ' ' || ch == '\u{3000}' {
            return Some(Glyph {
                w: 0,
                h: 0,
                off_x: 0,
                off_y: 0,
                advance,
                cov: Vec::new(),
            });
        }

        // 左右各留一像素余量：抗锯齿会溢出字身一点，斜体和某些字形也会。
        let pad = 1;
        let w = (size.cx + 2 * pad).min(self.scratch_w);
        let h = self.metrics.line_height.min(self.scratch_h);
        if w <= 0 || h <= 0 {
            return None;
        }

        let rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        // SAFETY: rect 在临时位图范围内；ETO_OPAQUE 先用背景色(黑)刷掉上一个字的残留，
        // 所以不需要额外清屏。units.len() 就是要画的 UTF-16 码元数。
        let drawn = unsafe {
            ExtTextOutW(
                self.dc,
                pad,
                0,
                ETO_OPAQUE,
                Some(&rect),
                windows::core::PCWSTR(units.as_ptr()),
                units.len() as u32,
                None,
            )
        }
        .as_bool();
        if !drawn {
            return None;
        }
        // GDI 的绘制是排队的，CPU 直接读位图前必须让它落地。
        // SAFETY: 无参数，只是刷新当前线程的 GDI 批处理队列。
        unsafe {
            let _ = GdiFlush();
        }

        let stride = (self.scratch_w * 4) as usize;
        let mut cov = vec![0u8; (w * h) as usize];
        for y in 0..h {
            // SAFETY: bits 指向 scratch_w * scratch_h * 4 字节，w/h 已夹到该范围内，
            // 每行按 stride 定位，读的都是这块位图自己的内存。
            let row =
                unsafe { std::slice::from_raw_parts(self.bits.add(y as usize * stride), stride) };
            for x in 0..w {
                // 白字黑底，任一通道即覆盖率；取蓝通道（BGRA 的第 0 字节）。
                let v = row.get((x * 4) as usize).copied().unwrap_or(0);
                if let Some(slot) = cov.get_mut((y * w + x) as usize) {
                    *slot = v;
                }
            }
        }

        Some(trim(Glyph {
            w,
            h,
            off_x: -pad,
            off_y: -self.metrics.ascent,
            advance,
            cov,
        }))
    }
}

impl Drop for FontRaster {
    fn drop(&mut self) {
        // SAFETY: 先把 DC 里选中的对象换回原来的，再删自己建的对象——顺序反了会泄漏。
        unsafe {
            if !self.old_bitmap.is_invalid() {
                SelectObject(self.dc, self.old_bitmap);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.old_font.is_invalid() {
                SelectObject(self.dc, self.old_font);
            }
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font.into());
            }
            if !self.dc.is_invalid() {
                let _ = DeleteDC(self.dc);
            }
        }
    }
}

/// 把掩码四周全 0 的行列裁掉：缓存小一点，合成时少扫一片空白。
fn trim(g: Glyph) -> Glyph {
    if g.w <= 0 || g.h <= 0 {
        return g;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (g.w, g.h, -1, -1);
    for y in 0..g.h {
        for x in 0..g.w {
            if g.cov.get((y * g.w + x) as usize).copied().unwrap_or(0) != 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < 0 {
        return Glyph {
            w: 0,
            h: 0,
            cov: Vec::new(),
            ..g
        };
    }
    let (nw, nh) = (max_x - min_x + 1, max_y - min_y + 1);
    let mut cov = vec![0u8; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let src = ((y + min_y) * g.w + (x + min_x)) as usize;
            if let (Some(d), Some(s)) = (cov.get_mut((y * nw + x) as usize), g.cov.get(src)) {
                *d = *s;
            }
        }
    }
    Glyph {
        w: nw,
        h: nh,
        off_x: g.off_x + min_x,
        off_y: g.off_y + min_y,
        advance: g.advance,
        cov,
    }
}

/// 往 `LOGFONTW.lfFaceName` 里写字体名。字段是定长 32 且必须以 0 结尾，超长直接截断
/// ——GDI 自己也是这么处理的，截断后匹配不上会退回系统默认字体，不会失败。
fn write_face_name(dst: &mut [u16; 32], family: &str) {
    let mut i = 0;
    for u in family.encode_utf16() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = u;
        i += 1;
    }
    dst[i] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_name_is_null_terminated() {
        let mut buf = [0xffffu16; 32];
        write_face_name(&mut buf, "Microsoft YaHei UI");
        let end = buf.iter().position(|&u| u == 0).expect("必须有终止符");
        assert_eq!(end, "Microsoft YaHei UI".len());
        let round: String = String::from_utf16_lossy(&buf[..end]);
        assert_eq!(round, "Microsoft YaHei UI");
    }

    #[test]
    fn face_name_truncates_instead_of_overflowing() {
        let mut buf = [0xffffu16; 32];
        write_face_name(&mut buf, &"字".repeat(100));
        assert_eq!(buf[31], 0, "最后一格必须是终止符");
        let end = buf.iter().position(|&u| u == 0).expect("必须有终止符");
        assert!(end <= 31);
    }

    #[test]
    fn trim_drops_empty_border() {
        // 5x3，只有中间一个像素有覆盖率。
        const W: usize = 5;
        let (row, col) = (1usize, 2usize);
        let mut cov = vec![0u8; 15];
        cov[row * W + col] = 200;
        let g = trim(Glyph {
            w: 5,
            h: 3,
            off_x: 0,
            off_y: -10,
            advance: 7,
            cov,
        });
        assert_eq!((g.w, g.h), (1, 1));
        assert_eq!(g.cov, vec![200]);
        assert_eq!(g.off_x, 2, "裁掉的空白要折进偏移里");
        assert_eq!(g.off_y, -9);
        assert_eq!(g.advance, 7, "裁剪不该改步进");
    }

    #[test]
    fn trim_of_blank_glyph_yields_zero_area() {
        let g = trim(Glyph {
            w: 4,
            h: 4,
            off_x: 0,
            off_y: 0,
            advance: 12,
            cov: vec![0u8; 16],
        });
        assert_eq!((g.w, g.h), (0, 0));
        assert!(g.cov.is_empty());
        assert_eq!(g.advance, 12, "空白字仍然要占位");
    }

    /// 需要 GDI，跑在有桌面的机器上：`cargo test -p vox-overlay-win -- --ignored`
    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn rasterizes_cjk_and_latin_with_sane_coverage() {
        let mut f = FontRaster::new("Microsoft YaHei UI", 30, 96).unwrap();
        assert!(f.metrics().line_height > 0 && f.metrics().ascent > 0);
        for ch in ['测', '试', 'A', 'g', '，'] {
            let g = f.glyph(ch).unwrap().clone();
            assert!(g.advance > 0, "{ch} 步进为 0");
            assert!(g.w > 0 && g.h > 0, "{ch} 没有可见像素");
            assert_eq!(g.cov.len(), (g.w * g.h) as usize);
            assert!(
                g.cov.iter().any(|&c| c > 200),
                "{ch} 应该有接近全覆盖的笔画中心"
            );
            assert!(
                g.cov.iter().any(|&c| (1..=200).contains(&c)),
                "{ch} 应该有抗锯齿的中间覆盖率，否则说明抗锯齿没开"
            );
        }
        // 空格没有可见像素但要占宽度。
        let sp = f.glyph(' ').unwrap().clone();
        assert!(sp.advance > 0 && sp.w == 0);
        // 缓存命中不该改变结果。
        let a = f.glyph('测').unwrap().clone();
        let b = f.glyph('测').unwrap().clone();
        assert_eq!(a.cov, b.cov);
    }

    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn dpi_scaling_makes_bigger_glyphs() {
        let mut lo = FontRaster::new("Microsoft YaHei UI", 30, 96).unwrap();
        let mut hi = FontRaster::new("Microsoft YaHei UI", 30, 192).unwrap();
        assert!(hi.advance('测') > lo.advance('测'), "200% 缩放下字应当更宽");
        assert!(hi.metrics().line_height > lo.metrics().line_height);
    }

    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn missing_font_falls_back_instead_of_failing() {
        // GDI 匹配不上字体名会退回系统字体，这是我们想要的行为：设置里填错不该让窗口起不来。
        let mut f = FontRaster::new("This Font Does Not Exist 中文", 24, 96).unwrap();
        assert!(f.advance('测') > 0);
    }

    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn glyph_cache_is_bounded() {
        let mut f = FontRaster::new("Microsoft YaHei UI", 24, 96).unwrap();
        for offset in 0..(MAX_CACHED_GLYPHS as u32 + 64) {
            if let Some(ch) = char::from_u32(0x4e00 + offset) {
                let _ = f.glyph(ch);
            }
        }
        assert!(
            f.cache.len() <= MAX_CACHED_GLYPHS,
            "字形缓存不能随运行时间无限增长"
        );
    }
}
