//! 布局：两行字 + 字号 + 屏幕矩形 → 窗口高度、每行底衬矩形和基线。
//!
//! 纯算术，没有 Win32，能单测。字宽由调用方量出来（GDI 或假的量尺）再传进来，
//! 这样布局逻辑跟字体引擎解耦。
//!
//! 尺寸一律是**物理像素**：DPI 缩放在算 `Metrics` 时一次性折进去，之后的代码
//! 不再关心缩放系数。

use crate::geom::RectI;

/// 一行字量出来的度量。
#[derive(Debug, Clone, Default)]
pub struct RowMetrics {
    /// 每个字的步进宽度，跟字符一一对应。
    pub advances: Vec<i32>,
    /// 行高（含内部行距）。
    pub line_height: i32,
    /// 基线到行顶的距离。
    pub ascent: i32,
}

impl RowMetrics {
    pub fn total_width(&self) -> i32 {
        self.advances.iter().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.advances.is_empty()
    }
}

/// 布局常量，按 DPI 缩放后的物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// 窗口四周留白。
    pub padding: i32,
    /// 两行之间的间距。
    pub row_gap: i32,
    /// 底衬左右内边距。
    pub plate_pad_x: i32,
    /// 底衬上下内边距。
    pub plate_pad_y: i32,
    /// 底衬圆角。
    pub plate_radius: i32,
}

impl Metrics {
    /// 逻辑像素下的一套值（padding 18 / 盒子 14×4 / 圆角 6）。
    pub const LOGICAL: Self = Self {
        padding: 18,
        row_gap: 6,
        plate_pad_x: 14,
        plate_pad_y: 4,
        plate_radius: 6,
    };

    /// 按 DPI 缩放。`dpi` 为 0 或异常值时退回 96，绝不产生 0 尺寸。
    pub fn for_dpi(dpi: u32) -> Self {
        let base = Self::LOGICAL;
        let dpi = if (72..=1200).contains(&dpi) { dpi } else { 96 };
        let scale = |v: i32| ((v as i64 * dpi as i64) / 96).max(1) as i32;
        Self {
            padding: scale(base.padding),
            row_gap: scale(base.row_gap),
            plate_pad_x: scale(base.plate_pad_x),
            plate_pad_y: scale(base.plate_pad_y),
            plate_radius: scale(base.plate_radius),
        }
    }
}

/// 一行摆好之后的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedRow {
    /// 底衬矩形（贴合可见文字）。
    pub plate: RectI,
    /// 第一个可见字在原始字符数组里的下标。
    pub first_visible: usize,
    /// 结束下标（不含）。固定视口布局会把一条字幕拆成多个视觉行。
    pub last_visible: usize,
    /// 第一个可见字的起笔 x。
    pub text_x: i32,
    /// 基线 y。
    pub baseline_y: i32,
}

/// 两行摆完的结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubtitleLayout {
    /// 上面那行（听人说话，冷白）。
    pub listen: Option<PlacedRow>,
    /// 下面那行（对外说话，暖白）。
    pub speak: Option<PlacedRow>,
    /// 固定视口模式下，听人行拆出来的视觉行。
    pub listen_rows: Vec<PlacedRow>,
    /// 固定视口模式下，对外行拆出来的视觉行。
    pub speak_rows: Vec<PlacedRow>,
    /// 这一帧需要的窗口高度（物理像素）。
    pub client_height: i32,
}

/// 摆两行字。
///
/// `client_width` 是窗口宽度（持久化下来的，不随内容变）；高度由内容算出来——
/// 只有一条流水线在跑时只画一行，窗口自动矮一截。行内容超宽时**从左侧滚掉**
/// 最老的字，保证最新的字始终可见（旧版的行为，照搬）。
pub fn layout_rows(
    listen: &RowMetrics,
    speak: &RowMetrics,
    client_width: i32,
    metrics: Metrics,
) -> SubtitleLayout {
    let avail = client_width - 2 * metrics.padding - 2 * metrics.plate_pad_x;

    let mut rows: Vec<(bool, PlacedRow, i32)> = Vec::new();
    let mut content_height = 0;
    for (is_listen, row) in [(true, listen), (false, speak)] {
        if row.is_empty() || avail <= 0 {
            continue;
        }
        let (first_visible, text_width) = visible_tail(&row.advances, avail);
        let plate_w = text_width + 2 * metrics.plate_pad_x;
        let plate_h = row.line_height + 2 * metrics.plate_pad_y;
        let placed = PlacedRow {
            // x/y 先占位，等总高度定了再统一往下挪。
            plate: RectI::new(0, 0, plate_w, plate_h),
            first_visible,
            last_visible: row.advances.len(),
            text_x: 0,
            baseline_y: row.ascent,
        };
        if !rows.is_empty() {
            content_height += metrics.row_gap;
        }
        content_height += plate_h;
        rows.push((is_listen, placed, plate_h));
    }

    // 空帧缩成 1px 全透明窗口；分层窗的 DIB 尺寸不能是 0。
    let client_height = if rows.is_empty() {
        1
    } else {
        2 * metrics.padding + content_height
    };

    let mut layout = SubtitleLayout {
        client_height,
        ..Default::default()
    };
    let mut y = metrics.padding;
    for (is_listen, mut placed, plate_h) in rows {
        placed.plate.x = (client_width - placed.plate.w) / 2;
        placed.plate.y = y;
        placed.text_x = placed.plate.x + metrics.plate_pad_x;
        placed.baseline_y += y + metrics.plate_pad_y;
        if is_listen {
            layout.listen = Some(placed);
        } else {
            layout.speak = Some(placed);
        }
        y += plate_h + metrics.row_gap;
    }
    layout
}

/// 在固定高度的视口里排版字幕。
///
/// 新字符会接在当前视觉行的右侧，宽度不够时才换到下一行。所有视觉行都
/// 保留在布局结果中；超出固定视口的部分只在 Canvas 绘制时被裁剪，因此
/// 换行和字符生命周期不会互相改变。窗口高度由用户拖动决定，不会被内容覆盖。
pub fn layout_rows_in_viewport(
    listen: &RowMetrics,
    speak: &RowMetrics,
    client_width: i32,
    client_height: i32,
    metrics: Metrics,
) -> SubtitleLayout {
    let width = client_width.max(1);
    let height = client_height.max(1);
    let inner_width = (width - 2 * metrics.padding).max(1);
    let avail = (inner_width - 2 * metrics.plate_pad_x).max(1);

    #[derive(Clone, Copy)]
    struct Segment {
        listen: bool,
        first: usize,
        last: usize,
        text_width: i32,
        line_height: i32,
        ascent: i32,
    }

    fn wrap(row: &RowMetrics, avail: i32, listen: bool) -> Vec<Segment> {
        if row.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut first = 0usize;
        while first < row.advances.len() {
            let mut last = first;
            let mut text_width = 0i32;
            while last < row.advances.len() {
                let advance = row.advances[last].max(0);
                if last > first && text_width.saturating_add(advance) > avail {
                    break;
                }
                text_width = text_width.saturating_add(advance);
                last += 1;
            }
            if last == first {
                last += 1;
                text_width = row.advances[first].max(1).min(avail);
            }
            out.push(Segment {
                listen,
                first,
                last,
                text_width,
                line_height: row.line_height.max(1),
                ascent: row.ascent,
            });
            first = last;
        }
        out
    }

    let mut segments = wrap(listen, avail, true);
    segments.extend(wrap(speak, avail, false));
    if segments.is_empty() {
        return SubtitleLayout {
            client_height: height,
            ..Default::default()
        };
    }

    // 视口只负责裁剪，不负责删除视觉行。把完整的行序列排在视口底部，
    // 最旧的行自然会落到客户区上方，由 Canvas 的像素边界裁掉。这样换行
    // 时上一行仍然存在，Renderer 才能在两个布局之间做连续过渡，字符 TTL
    // 也不会被布局层提前模拟掉。
    let total_height = segments
        .iter()
        .map(|s| s.line_height.saturating_add(2 * metrics.plate_pad_y).max(1) as i64)
        .sum::<i64>()
        .saturating_add(
            i64::from(metrics.row_gap.max(0))
                .saturating_mul(segments.len().saturating_sub(1) as i64),
        );
    // i64 计算后再夹回 i32，避免极端窗口/字号把 y 算溢出。
    let mut y = (i64::from(height) - i64::from(metrics.padding) - total_height)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let mut layout = SubtitleLayout {
        client_height: height,
        ..Default::default()
    };
    for segment in segments {
        let plate_h = segment.line_height + 2 * metrics.plate_pad_y;
        let plate_w = (segment.text_width + 2 * metrics.plate_pad_x).min(inner_width);
        let plate = RectI::new(metrics.padding, y, plate_w.max(1), plate_h);
        let placed = PlacedRow {
            plate,
            first_visible: segment.first,
            last_visible: segment.last,
            text_x: plate.x + metrics.plate_pad_x,
            baseline_y: y + metrics.plate_pad_y + segment.ascent,
        };
        if segment.listen {
            layout.listen_rows.push(placed);
        } else {
            layout.speak_rows.push(placed);
        }
        y = y.saturating_add(plate_h).saturating_add(metrics.row_gap);
    }
    layout.listen = layout.listen_rows.last().cloned();
    layout.speak = layout.speak_rows.last().cloned();
    layout
}

/// 从后往前留够 `avail` 宽度，返回 (第一个可见字下标, 可见部分总宽)。
///
/// 至少留一个字：窗口再窄也不该整行消失。
fn visible_tail(advances: &[i32], avail: i32) -> (usize, i32) {
    let mut total: i32 = advances.iter().sum();
    let mut start = 0;
    while start + 1 < advances.len() && total > avail {
        total -= advances.get(start).copied().unwrap_or(0);
        start += 1;
    }
    (start, total)
}

/// 首次运行的默认摆位：屏幕工作区底部居中，抬高 `bottom_margin` 让开任务栏。
///
/// `work` 用工作区（`rcWork`）而不是整块屏幕，这样天然避开任务栏。
pub fn default_placement(work: RectI, width: i32, height: i32, bottom_margin: i32) -> RectI {
    let width = width.min(work.w.max(1));
    let x = work.x + (work.w - width) / 2;
    let y = work.bottom() - height - bottom_margin;
    // 屏幕比窗口还矮时至少保证左上角在工作区里，别把窗口丢到看不见的地方。
    RectI::new(x.max(work.x), y.max(work.y), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 等宽假字体：每个字 30 px，行高 40，ascent 32。
    fn row(n: usize) -> RowMetrics {
        RowMetrics {
            advances: vec![30; n],
            line_height: 40,
            ascent: 32,
        }
    }

    fn m() -> Metrics {
        Metrics::LOGICAL
    }

    #[test]
    fn two_rows_stack_listen_above_speak() {
        let l = layout_rows(&row(4), &row(4), 800, m());
        let listen = l.listen.expect("听人行应当存在");
        let speak = l.speak.expect("对外行应当存在");
        assert!(
            listen.plate.y < speak.plate.y,
            "听人在上、对外在下（拍板 7）：listen.y={} speak.y={}",
            listen.plate.y,
            speak.plate.y
        );
        assert!(listen.plate.bottom() <= speak.plate.y, "两行不许重叠");
        assert_eq!(listen.plate.h, 40 + 2 * m().plate_pad_y);
        // 基线落在底衬内部。
        assert!(listen.baseline_y > listen.plate.y);
        assert!(listen.baseline_y <= listen.plate.bottom());
    }

    #[test]
    fn plates_hug_the_text_and_are_centered() {
        let l = layout_rows(&row(3), &RowMetrics::default(), 800, m());
        let listen = l.listen.expect("有内容就该有行");
        assert_eq!(
            listen.plate.w,
            3 * 30 + 2 * m().plate_pad_x,
            "底衬贴合文字宽度"
        );
        let slack = (800 - listen.plate.w) / 2;
        assert_eq!(listen.plate.x, slack, "水平居中");
        assert_eq!(listen.text_x, listen.plate.x + m().plate_pad_x);
    }

    #[test]
    fn single_row_makes_the_window_shorter() {
        let one = layout_rows(&row(4), &RowMetrics::default(), 800, m());
        let two = layout_rows(&row(4), &row(4), 800, m());
        assert!(
            one.client_height < two.client_height,
            "只有一条流水线时窗口要缩：{} 不小于 {}",
            one.client_height,
            two.client_height
        );
        assert!(one.speak.is_none());
    }

    #[test]
    fn empty_rows_collapse_to_one_transparent_pixel() {
        let l = layout_rows(&RowMetrics::default(), &RowMetrics::default(), 800, m());
        assert!(l.listen.is_none() && l.speak.is_none());
        assert_eq!(l.client_height, 1);
    }

    #[test]
    fn overlong_row_scrolls_off_the_left() {
        // 窗口 300 宽：留白 18*2 + 盒子 14*2 = 64，可用 236 → 最多 7 个 30 px 的字。
        let l = layout_rows(&row(40), &RowMetrics::default(), 300, m());
        let listen = l.listen.expect("超长行也要画");
        assert!(listen.first_visible > 0, "最老的字应当被滚掉");
        let visible = 40 - listen.first_visible;
        assert!(
            (visible as i32) * 30 <= 236,
            "可见部分 {visible} 个字仍然超宽"
        );
        assert_eq!(
            listen.first_visible + visible,
            40,
            "留下的必须是最新的那一段"
        );
        assert!(listen.plate.x >= 0, "底衬不该跑到窗口左边外面");
    }

    #[test]
    fn very_narrow_window_keeps_at_least_one_char() {
        let l = layout_rows(&row(10), &RowMetrics::default(), 70, m());
        match l.listen {
            Some(listen) => assert_eq!(listen.first_visible, 9, "再窄也要留最后一个字"),
            None => panic!("有内容不该整行消失"),
        }
    }

    #[test]
    fn non_positive_width_drops_rows_without_panicking() {
        let l = layout_rows(&row(5), &row(5), 10, m());
        assert!(l.listen.is_none() && l.speak.is_none());
        assert!(l.client_height > 0);
    }

    #[test]
    fn viewport_wraps_without_moving_the_text_origin() {
        let l = layout_rows_in_viewport(&row(20), &RowMetrics::default(), 300, 170, m());
        assert!(l.listen_rows.len() > 1, "固定视口应当把长句拆成多行");
        assert!(l
            .listen_rows
            .iter()
            .all(|r| r.text_x == m().padding + m().plate_pad_x));
        assert_eq!(l.client_height, 170, "窗口高度由视口决定，而不是内容决定");
        for pair in l.listen_rows.windows(2) {
            assert!(pair[0].plate.bottom() + m().row_gap <= pair[1].plate.y);
        }
    }

    #[test]
    fn viewport_keeps_all_visual_rows_for_renderer_clipping() {
        let l = layout_rows_in_viewport(&row(100), &RowMetrics::default(), 300, 170, m());
        let last = l.listen_rows.last().expect("长句至少保留一行");
        assert_eq!(last.last_visible, 100);
        assert_eq!(l.listen_rows.first().unwrap().first_visible, 0);
        assert!(l.listen_rows.len() > 2, "布局必须保留视口外的完整行序列");
        assert!(
            l.listen_rows.first().unwrap().plate.y < 0,
            "最旧行落在视口上方，由画布裁剪而不是布局删除"
        );
    }

    #[test]
    fn two_tracks_wrap_without_overlapping_rows() {
        let l = layout_rows_in_viewport(&row(20), &row(20), 300, 170, m());
        let mut all = l.listen_rows.clone();
        all.extend(l.speak_rows.clone());
        for pair in all.windows(2) {
            assert!(
                pair[0].plate.bottom() + m().row_gap <= pair[1].plate.y,
                "两条轨道的视觉行仍须按顺序留出间距"
            );
        }
    }

    #[test]
    fn metrics_scale_with_dpi_and_reject_nonsense() {
        let at96 = Metrics::for_dpi(96);
        assert_eq!(at96, Metrics::LOGICAL);
        let at144 = Metrics::for_dpi(144); // 150%
        assert_eq!(at144.padding, 27);
        assert_eq!(Metrics::for_dpi(0), Metrics::LOGICAL, "0 DPI 退回 96");
        assert_eq!(
            Metrics::for_dpi(99_999),
            Metrics::LOGICAL,
            "离谱 DPI 退回 96"
        );
        // 缩到很小也不许出现 0 尺寸。
        let tiny = Metrics::for_dpi(72);
        assert!(tiny.row_gap >= 1 && tiny.plate_radius >= 1);
    }

    #[test]
    fn default_placement_centers_near_the_bottom() {
        let work = RectI::new(0, 0, 1920, 1040); // 1080 减掉任务栏
        let r = default_placement(work, 880, 170, 80);
        assert_eq!(r.w, 880);
        assert_eq!(r.center_x(), work.center_x(), "水平居中");
        assert_eq!(r.bottom(), 1040 - 80, "底部留出 80 给任务栏");
        assert!(r.y > work.y);
    }

    #[test]
    fn default_placement_honors_monitor_offset() {
        // 第二块屏在主屏右边，坐标是负的也要能算对。
        let work = RectI::new(-1920, -200, 1600, 900);
        let r = default_placement(work, 880, 170, 80);
        assert_eq!(r.center_x(), work.center_x());
        assert!(r.x >= work.x && r.right() <= work.right());
        assert_eq!(r.bottom(), work.bottom() - 80);
    }

    #[test]
    fn default_placement_clamps_on_tiny_screens() {
        let work = RectI::new(0, 0, 400, 120);
        let r = default_placement(work, 880, 170, 80);
        assert_eq!(r.w, 400, "比屏幕宽就压到屏幕宽");
        assert_eq!(r.x, work.x);
        assert_eq!(r.y, work.y, "屏幕太矮时至少顶到工作区上边");
    }
}
