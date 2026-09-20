//! 颜色与**预乘 alpha** 运算。
//!
//! 分层窗走 `UpdateLayeredWindow` + `AC_SRC_ALPHA`，GDI 认定源位图里的颜色通道
//! **已经乘过 alpha**。不预乘的话半透明像素会被当成"很亮 + 很透"，字形边缘会
//! 泛出一圈亮边（halo）。所以这个模块里的每个构造函数都在出口处预乘，
//! `Bgra` 一旦造出来就保证 `b,g,r <= a`。

/// 未预乘的直色，从 `#rrggbb` 解析出来的东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const WHITE: Self = Self::new(255, 255, 255);
}

/// DIB 里的一个像素：BGRA 顺序、**已预乘**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bgra {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl Bgra {
    /// 全透明。整幅画布的初值——分层窗里这些像素完全不上屏。
    pub const CLEAR: Self = Self {
        b: 0,
        g: 0,
        r: 0,
        a: 0,
    };

    /// 直色 + alpha → 预乘像素。**唯一的预乘入口**，别在别处手写这三行。
    pub fn premultiplied(color: Rgb, alpha: u8) -> Self {
        Self {
            b: mul_u8(color.b, alpha),
            g: mul_u8(color.g, alpha),
            r: mul_u8(color.r, alpha),
            a: alpha,
        }
    }

    /// 反解回直色，只给测试和自检用（alpha 为 0 时无法反解，返回黑）。
    pub fn unpremultiplied(&self) -> Rgb {
        if self.a == 0 {
            return Rgb::BLACK;
        }
        let a = self.a as u32;
        Rgb {
            r: ((self.r as u32 * 255 + a / 2) / a).min(255) as u8,
            g: ((self.g as u32 * 255 + a / 2) / a).min(255) as u8,
            b: ((self.b as u32 * 255 + a / 2) / a).min(255) as u8,
        }
    }

    /// 预乘不变量：任一颜色通道都不能超过 alpha。超了就是会冒亮边的坏像素。
    pub fn is_valid_premultiplied(&self) -> bool {
        self.b <= self.a && self.g <= self.a && self.r <= self.a
    }
}

/// `x * y / 255`，四舍五入。整数做，避免浮点在边界上抖出 ±1。
pub fn mul_u8(x: u8, y: u8) -> u8 {
    let t = x as u32 * y as u32 + 128;
    (((t >> 8) + t) >> 8) as u8
}

/// 0.0..=1.0 的不透明度转 u8。NaN 当 0，越界夹回来。
pub fn alpha_to_u8(alpha: f32) -> u8 {
    // NaN 单独挡掉：任何比较都是 false，落到后面会被当成有效值。
    if alpha.is_nan() || alpha <= 0.0 {
        return 0;
    }
    if alpha >= 1.0 {
        return 255;
    }
    (alpha * 255.0 + 0.5) as u8
}

/// 源(预乘) over 目标(预乘)。两边都预乘时合成就是这一个式子。
pub fn blend_over(dst: Bgra, src: Bgra) -> Bgra {
    let inv = 255 - src.a;
    Bgra {
        b: src.b.saturating_add(mul_u8(dst.b, inv)),
        g: src.g.saturating_add(mul_u8(dst.g, inv)),
        r: src.r.saturating_add(mul_u8(dst.r, inv)),
        a: src.a.saturating_add(mul_u8(dst.a, inv)),
    }
}

/// 解析 `#rrggbb`。只认这一种写法——内核的 `normalize_color` 已经保证了格式，
/// 这里再挡一道是为了别让手写的字面量悄悄退化成黑色。
pub fn parse_hex_rgb(text: &str) -> Option<Rgb> {
    let s = text.trim();
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte_at = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Rgb {
        r: byte_at(0)?,
        g: byte_at(2)?,
        b: byte_at(4)?,
    })
}

/// 解析失败就用兜底色，并留个日志——配置里塞了怪颜色时不该整行字幕变黑。
pub fn parse_hex_rgb_or(text: &str, fallback: Rgb) -> Rgb {
    match parse_hex_rgb(text) {
        Some(rgb) => rgb,
        None => {
            tracing::warn!(color = text, "字幕颜色不是 #rrggbb，用兜底色");
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_canonical_form() {
        assert_eq!(parse_hex_rgb("#eef6ff"), Some(Rgb::new(0xee, 0xf6, 0xff)));
        assert_eq!(parse_hex_rgb("#fff4de"), Some(Rgb::new(0xff, 0xf4, 0xde)));
        assert_eq!(parse_hex_rgb("#000000"), Some(Rgb::BLACK));
        assert_eq!(
            parse_hex_rgb("  #FFFFFF  "),
            Some(Rgb::WHITE),
            "大写和空白可以"
        );
    }

    #[test]
    fn hex_rejects_malformed_input() {
        for bad in [
            "",
            "#",
            "eef6ff",   // 少井号
            "#eef6f",   // 少一位
            "#eef6fff", // 多一位
            "#eef6fg",  // 非十六进制
            "#eef 6ff", // 中间有空格
            "红色",
            "#红色红",
            "rgb(1,2,3)",
        ] {
            assert!(parse_hex_rgb(bad).is_none(), "{bad:?} 不该被接受");
        }
    }

    #[test]
    fn fallback_kicks_in_for_garbage() {
        assert_eq!(parse_hex_rgb_or("nope", Rgb::WHITE), Rgb::WHITE);
        assert_eq!(parse_hex_rgb_or("#010203", Rgb::WHITE), Rgb::new(1, 2, 3));
    }

    #[test]
    fn mul_u8_is_rounded_and_exact_at_ends() {
        assert_eq!(mul_u8(255, 255), 255);
        assert_eq!(mul_u8(255, 0), 0);
        assert_eq!(mul_u8(0, 255), 0);
        assert_eq!(mul_u8(255, 128), 128, "半透明的白应当正好是 alpha");
        assert_eq!(mul_u8(128, 128), 64);
        // 跟浮点参考值差不超过 1。
        for x in 0..=255u8 {
            for y in (0..=255u8).step_by(17) {
                let want = (x as f32 * y as f32 / 255.0).round() as i32;
                let got = mul_u8(x, y) as i32;
                assert!((want - got).abs() <= 1, "mul_u8({x},{y})={got} 期望≈{want}");
            }
        }
    }

    #[test]
    fn premultiplied_never_violates_invariant() {
        // 最容易出事的是近白色 + 低 alpha：不预乘就会冒亮边。
        for alpha in 0..=255u8 {
            for color in [
                Rgb::WHITE,
                Rgb::new(0xee, 0xf6, 0xff),
                Rgb::new(0xff, 0xf4, 0xde),
            ] {
                let px = Bgra::premultiplied(color, alpha);
                assert!(
                    px.is_valid_premultiplied(),
                    "{color:?} @ {alpha} 预乘后越界: {px:?}"
                );
                assert_eq!(px.a, alpha);
            }
        }
    }

    #[test]
    fn premultiply_roundtrips_within_one_step() {
        let color = Rgb::new(0xee, 0xf6, 0xff);
        for alpha in [8u8, 32, 96, 128, 200, 255] {
            let back = Bgra::premultiplied(color, alpha).unpremultiplied();
            let drift = (back.r as i32 - color.r as i32)
                .abs()
                .max((back.g as i32 - color.g as i32).abs())
                .max((back.b as i32 - color.b as i32).abs());
            // 低 alpha 下量化误差按 255/alpha 放大，这是预乘存储的固有精度。
            let tolerance = (255 / alpha.max(1) as i32) + 1;
            assert!(
                drift <= tolerance,
                "alpha={alpha} 色偏 {drift} 超过 {tolerance}"
            );
        }
    }

    #[test]
    fn alpha_conversion_clamps_and_survives_nan() {
        assert_eq!(alpha_to_u8(0.0), 0);
        assert_eq!(alpha_to_u8(1.0), 255);
        assert_eq!(alpha_to_u8(0.5), 128);
        assert_eq!(alpha_to_u8(-3.0), 0);
        assert_eq!(alpha_to_u8(9.0), 255);
        assert_eq!(alpha_to_u8(f32::NAN), 0, "NaN 不能变成不透明");
        assert_eq!(alpha_to_u8(f32::INFINITY), 255);
    }

    #[test]
    fn blend_over_clear_keeps_source() {
        let src = Bgra::premultiplied(Rgb::new(0xee, 0xf6, 0xff), 200);
        assert_eq!(blend_over(Bgra::CLEAR, src), src);
    }

    #[test]
    fn blend_over_opaque_source_replaces_destination() {
        let dst = Bgra::premultiplied(Rgb::BLACK, 255);
        let src = Bgra::premultiplied(Rgb::WHITE, 255);
        assert_eq!(blend_over(dst, src), src);
    }

    #[test]
    fn blend_over_preserves_premultiplied_invariant() {
        let plate = Bgra::premultiplied(Rgb::new(8, 12, 18), 165);
        for alpha in (0..=255u8).step_by(5) {
            let glyph = Bgra::premultiplied(Rgb::new(0xff, 0xf4, 0xde), alpha);
            let out = blend_over(plate, glyph);
            assert!(out.is_valid_premultiplied(), "合成结果越界: {out:?}");
        }
    }

    #[test]
    fn blending_transparent_source_is_a_no_op() {
        let dst = Bgra::premultiplied(Rgb::new(1, 2, 3), 40);
        assert_eq!(blend_over(dst, Bgra::CLEAR), dst);
    }
}
