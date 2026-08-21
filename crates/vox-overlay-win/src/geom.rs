//! 整数矩形。
//!
//! 不用 Win32 的 `RECT`：布局、命中测试、默认摆位这些纯逻辑要能在没有桌面的
//! 环境下单测，所以几何类型自己定一份，只在真正调 Win32 时才换成 `RECT`。

/// 左上角 + 宽高。宽高为负当空矩形处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// 从两个角点建。给 Win32 的 `RECT`（left/top/right/bottom）转过来用。
    pub const fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// 含左上、不含右下，跟 Win32 的命中语义一致。
    pub const fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// 四边同时向内收 `d`（负数是向外扩）。
    pub const fn inset(&self, d: i32) -> Self {
        Self {
            x: self.x + d,
            y: self.y + d,
            w: self.w - 2 * d,
            h: self.h - 2 * d,
        }
    }

    pub const fn center_x(&self) -> i32 {
        self.x + self.w / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_roundtrip() {
        let r = RectI::from_edges(10, 20, 110, 70);
        assert_eq!(r, RectI::new(10, 20, 100, 50));
        assert_eq!((r.right(), r.bottom()), (110, 70));
    }

    #[test]
    fn contains_excludes_right_and_bottom_edge() {
        let r = RectI::new(0, 0, 10, 10);
        assert!(r.contains(0, 0));
        assert!(r.contains(9, 9));
        assert!(!r.contains(10, 5), "右边界不算命中，跟 Win32 一致");
        assert!(!r.contains(5, 10));
        assert!(!r.contains(-1, 5));
    }

    #[test]
    fn inset_can_go_empty() {
        let r = RectI::new(0, 0, 10, 10);
        assert_eq!(r.inset(2), RectI::new(2, 2, 6, 6));
        assert!(r.inset(6).is_empty(), "收过头要变空，别产生负宽矩形");
    }
}
