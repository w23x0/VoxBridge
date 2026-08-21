//! 画一整帧纯字幕：逐行底衬 + 逐字 alpha 的文字。
//!
//! 跟窗口无关——输入是内容和几何，输出是一块预乘 BGRA 的 `Canvas`，所以快照
//! 工具可以不开窗口就复用完全相同的像素路径。

use vox_core::ports::{SubtitleFrame, SubtitleLine};
use vox_core::settings::SubtitleSettings;
use vox_core::subtitle::{RenderedChar, Track};

use crate::canvas::{Canvas, Mask};
use crate::color::{alpha_to_u8, parse_hex_rgb_or, Rgb};
use crate::layout::{layout_rows, layout_rows_in_viewport, Metrics, RowMetrics};
use crate::text::FontRaster;

/// 字幕底衬的颜色（近黑）。alpha 由设置里的 `background_alpha` 给。
const PLATE_COLOR: Rgb = Rgb::new(0, 0, 0);
const LAYOUT_ANIMATION_MS: u64 = 220;
const INITIAL_CHAR_ID: u64 = 1;

/// 一帧需要的全部输入。
pub struct FrameInput<'a> {
    pub frame: &'a SubtitleFrame,
    pub settings: &'a SubtitleSettings,
    /// 客户区宽度（物理像素）。
    pub client_width: i32,
    /// 客户区高度（物理像素）。大于 0 时使用固定视口布局，0 仅供离线快照使用
    /// “按内容高度”的兼容模式。
    pub client_height: i32,
    pub dpi: u32,
}

/// 画完一帧之后窗口需要采用的高度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameOutput {
    pub client_height: i32,
}

#[derive(Clone, Copy)]
struct RowStyle {
    color: Rgb,
    plate_alpha: u8,
    radius: f32,
}

#[derive(Debug, Clone)]
struct CharIdentity {
    previous: Vec<char>,
    ids: Vec<u64>,
}

impl Default for CharIdentity {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            ids: Vec::new(),
        }
    }
}

impl CharIdentity {
    /// Match the current frame to the previous ordered character sequence. LCS keeps
    /// identities through append and TTL prefix pruning without putting lifecycle data
    /// into `RenderedChar` or making the subtitle frame protocol renderer-specific.
    fn update(&mut self, chars: &[RenderedChar], next_id: &mut u64) -> Vec<u64> {
        let current: Vec<char> = chars.iter().map(|c| c.ch).collect();
        if current.is_empty() {
            self.previous.clear();
            self.ids.clear();
            return Vec::new();
        }

        let old_len = self.previous.len();
        let new_len = current.len();
        let mut lcs = vec![vec![0u16; new_len + 1]; old_len + 1];
        for i in (0..old_len).rev() {
            for j in (0..new_len).rev() {
                lcs[i][j] = if self.previous[i] == current[j] {
                    lcs[i + 1][j + 1].saturating_add(1)
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }

        let mut matched = vec![None; new_len];
        let (mut i, mut j) = (0usize, 0usize);
        while i < old_len && j < new_len {
            if self.previous[i] == current[j] {
                matched[j] = self.ids.get(i).copied();
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }

        let mut ids = Vec::with_capacity(new_len);
        for id in matched {
            let id = match id {
                Some(id) => id,
                None => {
                    let fresh = (*next_id).max(INITIAL_CHAR_ID);
                    *next_id = fresh.saturating_add(1);
                    fresh
                }
            };
            ids.push(id);
        }
        self.previous = current;
        self.ids = ids.clone();
        ids
    }
}

#[derive(Debug, Clone)]
struct StableRow {
    key: u64,
    placed: crate::layout::PlacedRow,
    char_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
struct RowTransition {
    started: std::time::Instant,
    from: Vec<(u64, f32)>,
    target: Vec<StableRow>,
}

impl RowTransition {
    fn progress(&self, now: std::time::Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_millis() as f32;
        (elapsed / LAYOUT_ANIMATION_MS as f32).clamp(0.0, 1.0)
    }

    fn y_at(&self, key: u64, now: std::time::Instant) -> Option<f32> {
        let target = self.target.iter().find(|row| row.key == key)?;
        let start = self
            .from
            .iter()
            .find(|(old_key, _)| *old_key == key)
            .map(|(_, y)| *y)
            .unwrap_or(target.placed.plate.y as f32);
        Some(interpolate_y(
            start,
            target.placed.plate.y as f32,
            self.progress(now),
        ))
    }
}

/// Ease-out cubic interpolation used for every layout transition.
fn interpolate_y(start: f32, end: f32, progress: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    start + (end - start) * eased
}

fn layout_changed(previous: &[StableRow], target: &[StableRow]) -> bool {
    previous.len() != target.len()
        || previous
            .iter()
            .zip(target.iter())
            .any(|(old, new)| old.key != new.key || old.placed.plate.y != new.placed.plate.y)
}

/// 一帧一帧往 `Canvas` 上画。持有字体，所以只能在窗口线程上用。
pub struct Renderer {
    font: FontRaster,
    /// 记住建字体时的参数，`restyle` 只在真的变了的时候重建。
    font_family: String,
    font_size: u32,
    dpi: u32,
    listen_ids: CharIdentity,
    speak_ids: CharIdentity,
    next_char_id: u64,
    previous_rows: Vec<StableRow>,
    transition: Option<RowTransition>,
}

impl Renderer {
    pub fn new(settings: &SubtitleSettings, dpi: u32) -> vox_core::ports::PortResult<Self> {
        Ok(Self {
            font: FontRaster::new(&settings.font_family, settings.font_size, dpi)?,
            font_family: settings.font_family.clone(),
            font_size: settings.font_size,
            dpi,
            listen_ids: CharIdentity::default(),
            speak_ids: CharIdentity::default(),
            next_char_id: INITIAL_CHAR_ID,
            previous_rows: Vec::new(),
            transition: None,
        })
    }

    /// 字体或 DPI 变了才换字体；没变就保留现有字形缓存。
    pub fn sync_font(
        &mut self,
        settings: &SubtitleSettings,
        dpi: u32,
    ) -> vox_core::ports::PortResult<()> {
        if self.font_family == settings.font_family
            && self.font_size == settings.font_size
            && self.dpi == dpi
        {
            return Ok(());
        }
        self.font = FontRaster::new(&settings.font_family, settings.font_size, dpi)?;
        self.font_family = settings.font_family.clone();
        self.font_size = settings.font_size;
        self.dpi = dpi;
        // A font/DPI change invalidates every measured boundary. Let the next frame
        // establish a fresh layout rather than carrying pixel positions across metrics.
        self.previous_rows.clear();
        self.transition = None;
        Ok(())
    }

    pub fn is_animating(&self) -> bool {
        self.transition.is_some()
    }

    /// 只算高度不画。
    pub fn measure(&mut self, frame: &SubtitleFrame, client_width: i32, dpi: u32) -> i32 {
        let (listen, speak) = self.measure_rows(frame);
        layout_rows(&listen, &speak, client_width, Metrics::for_dpi(dpi)).client_height
    }

    /// 把一帧画到 `canvas` 上。`resize` 本身会清空画布，不再重复清屏。
    pub fn draw(&mut self, canvas: &mut Canvas, input: &FrameInput<'_>) -> FrameOutput {
        let metrics = Metrics::for_dpi(input.dpi);
        let (listen_m, speak_m) = self.measure_rows(input.frame);
        let mut sub = if input.client_height > 0 {
            layout_rows_in_viewport(
                &listen_m,
                &speak_m,
                input.client_width,
                input.client_height,
                metrics,
            )
        } else {
            // 快照/离线调用可以传 0，保留旧的“按内容高度”语义；真实窗口
            // 始终传入正高度，使用固定视口。
            layout_rows(&listen_m, &speak_m, input.client_width, metrics)
        };
        if input.client_height <= 0 {
            if let Some(row) = sub.listen.clone() {
                sub.listen_rows.push(row);
            }
            if let Some(row) = sub.speak.clone() {
                sub.speak_rows.push(row);
            }
        }

        let listen_ids = self.listen_ids.update(
            find_line(input.frame, Track::Listen)
                .map(|l| l.chars.as_slice())
                .unwrap_or(&[]),
            &mut self.next_char_id,
        );
        let speak_ids = self.speak_ids.update(
            find_line(input.frame, Track::Speak)
                .map(|l| l.chars.as_slice())
                .unwrap_or(&[]),
            &mut self.next_char_id,
        );
        let target_rows = self.collect_rows(&sub, &listen_ids, &speak_ids, input.frame);
        let positions = if input.client_height <= 0 {
            // Offline snapshots use content-sized layout and are rendered as a
            // single settled frame. Do not carry a real-window transition between
            // independent scenes.
            let positions = target_rows
                .iter()
                .map(|row| (row.key, row.placed.plate.y as f32))
                .collect();
            self.previous_rows = target_rows;
            self.transition = None;
            positions
        } else {
            let now = std::time::Instant::now();
            let current_positions = self.current_positions(now);
            self.update_transition(
                target_rows,
                current_positions,
                metrics,
                input.client_height.max(1),
            );
            self.current_positions(now)
        };

        canvas.resize(input.client_width, sub.client_height);

        let listen_color = parse_hex_rgb_or(&input.settings.listen_color, Rgb::WHITE);
        let speak_color = parse_hex_rgb_or(&input.settings.speak_color, Rgb::WHITE);
        let keep_offscreen_rows = self.transition.is_some();
        for (track, color) in [(Track::Listen, listen_color), (Track::Speak, speak_color)] {
            let placed_rows = match track {
                Track::Listen => &sub.listen_rows,
                Track::Speak => &sub.speak_rows,
            };
            let Some(line) = find_line(input.frame, track) else {
                continue;
            };
            let style = RowStyle {
                color,
                plate_alpha: input.settings.background_alpha,
                radius: metrics.plate_radius as f32,
            };
            for placed in placed_rows {
                // During a transition the old and new layouts are both retained.
                // Once settled, fully offscreen visual rows are skipped here; their
                // characters remain in the frame and can still expire by TTL.
                if input.client_height > 0
                    && !keep_offscreen_rows
                    && (placed.plate.bottom() <= 0 || placed.plate.y >= sub.client_height)
                {
                    continue;
                }
                let key = self.row_key(
                    track,
                    placed,
                    &match track {
                        Track::Listen => listen_ids.as_slice(),
                        Track::Speak => speak_ids.as_slice(),
                    },
                );
                let y = positions
                    .iter()
                    .find(|(row_key, _)| *row_key == key)
                    .map(|(_, y)| *y)
                    .unwrap_or(placed.plate.y as f32);
                self.draw_row(canvas, placed, line, style, y);
            }
        }

        FrameOutput {
            client_height: sub.client_height,
        }
    }

    fn collect_rows(
        &self,
        layout: &crate::layout::SubtitleLayout,
        listen_ids: &[u64],
        speak_ids: &[u64],
        frame: &SubtitleFrame,
    ) -> Vec<StableRow> {
        let mut rows = Vec::with_capacity(layout.listen_rows.len() + layout.speak_rows.len());
        for (track, placed_rows, ids) in [
            (Track::Listen, &layout.listen_rows, listen_ids),
            (Track::Speak, &layout.speak_rows, speak_ids),
        ] {
            let Some(line) = find_line(frame, track) else {
                continue;
            };
            for placed in placed_rows {
                let ids_for_row = ids
                    .get(placed.first_visible..placed.last_visible.min(ids.len()))
                    .unwrap_or(&[])
                    .to_vec();
                rows.push(StableRow {
                    key: self.row_key(track, placed, ids),
                    placed: placed.clone(),
                    char_ids: ids_for_row,
                });
            }
            // `line` is intentionally looked up above: a layout row without a source
            // line cannot be drawn and should not participate in animation state.
            let _ = line;
        }
        rows
    }

    fn row_key(&self, track: Track, placed: &crate::layout::PlacedRow, ids: &[u64]) -> u64 {
        let first = ids
            .get(placed.first_visible)
            .copied()
            .unwrap_or_else(|| placed.first_visible as u64 + INITIAL_CHAR_ID);
        let track_bit = match track {
            Track::Listen => 0u64,
            Track::Speak => 1u64 << 63,
        };
        track_bit | first
    }

    fn current_positions(&self, now: std::time::Instant) -> Vec<(u64, f32)> {
        let Some(transition) = &self.transition else {
            return self
                .previous_rows
                .iter()
                .map(|row| (row.key, row.placed.plate.y as f32))
                .collect();
        };
        transition
            .target
            .iter()
            .filter_map(|row| transition.y_at(row.key, now).map(|y| (row.key, y)))
            .collect()
    }

    fn update_transition(
        &mut self,
        target: Vec<StableRow>,
        current_positions: Vec<(u64, f32)>,
        metrics: Metrics,
        viewport_height: i32,
    ) {
        if target.is_empty() {
            self.previous_rows.clear();
            self.transition = None;
            return;
        }

        // The first visible frame is a baseline, not a scroll event. Subsequent
        // boundary changes are the ones that need interpolation.
        if self.previous_rows.is_empty() {
            self.previous_rows = target;
            self.transition = None;
            return;
        }

        let changed = layout_changed(&self.previous_rows, &target);
        if !changed {
            self.previous_rows = target;
            if self
                .transition
                .as_ref()
                .is_some_and(|transition| transition.progress(std::time::Instant::now()) >= 1.0)
            {
                self.transition = None;
            }
            return;
        }

        let row_step = metrics.row_gap.saturating_add(
            target
                .iter()
                .map(|row| row.placed.plate.h)
                .max()
                .unwrap_or(1),
        ) as f32;
        // If a row splits, its first half usually keeps the old key. Do not reuse
        // that same old y for the unmatched second half, or the two target rows
        // would overlap at animation t=0. Prefix-pruned rows can still use overlap
        // matching because no exact row is claiming their previous position.
        let mut claimed_old_keys: Vec<u64> = target
            .iter()
            .filter(|row| self.previous_rows.iter().any(|old| old.key == row.key))
            .map(|row| row.key)
            .collect();
        let mut from = Vec::with_capacity(target.len());
        for row in &target {
            let old_y = current_positions
                .iter()
                .find(|(key, _)| *key == row.key)
                .map(|(_, y)| *y)
                .or_else(|| {
                    let matched = self
                        .previous_rows
                        .iter()
                        .filter(|old| !claimed_old_keys.contains(&old.key))
                        .filter_map(|old| {
                            let overlap = old
                                .char_ids
                                .iter()
                                .filter(|id| row.char_ids.contains(id))
                                .count();
                            (overlap > 0).then_some((overlap, old.key, old.placed.plate.y as f32))
                        })
                        .max_by_key(|(overlap, _, _)| *overlap);
                    if let Some((_, old_key, y)) = matched {
                        claimed_old_keys.push(old_key);
                        Some(y)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    // A row inserted before another track must not start on top of
                    // that track's old row. Enter below the viewport; the canvas
                    // clips it until the eased transition brings it into place.
                    (row.placed.plate.y as f32 + row_step).max(viewport_height as f32 + row_step)
                });
            from.push((row.key, old_y));
        }
        self.transition = Some(RowTransition {
            started: std::time::Instant::now(),
            from,
            target: target.clone(),
        });
        self.previous_rows = target;
    }

    fn measure_rows(&mut self, frame: &SubtitleFrame) -> (RowMetrics, RowMetrics) {
        let fm = self.font.metrics();
        let mut make = |track: Track| {
            let chars = find_line(frame, track)
                .map(|l| l.chars.as_slice())
                .unwrap_or(&[]);
            RowMetrics {
                advances: chars.iter().map(|c| self.font.advance(c.ch)).collect(),
                line_height: fm.line_height,
                ascent: fm.ascent,
            }
        };
        (make(Track::Listen), make(Track::Speak))
    }

    fn draw_row(
        &mut self,
        canvas: &mut Canvas,
        placed: &crate::layout::PlacedRow,
        line: &SubtitleLine,
        style: RowStyle,
        animated_y: f32,
    ) {
        let y = animated_y.round() as i32;
        let delta_y = y.saturating_sub(placed.plate.y);
        let plate = crate::geom::RectI::new(placed.plate.x, y, placed.plate.w, placed.plate.h);
        canvas.fill_round_rect(plate, style.radius, PLATE_COLOR, style.plate_alpha);

        let mut x = placed.text_x;
        let baseline_y = placed.baseline_y.saturating_add(delta_y);
        for rc in line
            .chars
            .iter()
            .skip(placed.first_visible)
            .take(placed.last_visible.saturating_sub(placed.first_visible))
        {
            x += draw_char_with(&mut self.font, canvas, x, baseline_y, rc, style.color);
        }
    }
}

/// 画一个字，返回下一个字的起笔步进。
fn draw_char_with(
    font: &mut FontRaster,
    canvas: &mut Canvas,
    pen_x: i32,
    baseline_y: i32,
    rc: &RenderedChar,
    color: Rgb,
) -> i32 {
    let alpha = alpha_to_u8(rc.alpha);
    let Some(g) = font.glyph(rc.ch) else {
        return 0;
    };
    let advance = g.advance;
    if alpha == 0 || g.w <= 0 || g.h <= 0 {
        return advance;
    }
    canvas.blend_mask(
        (pen_x + g.off_x, baseline_y + g.off_y),
        Mask {
            w: g.w,
            h: g.h,
            cov: &g.cov,
        },
        color,
        alpha,
    );
    advance
}

fn find_line(frame: &SubtitleFrame, track: Track) -> Option<&SubtitleLine> {
    frame.lines.iter().find(|l| l.track == track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn line(track: Track, text: &str) -> SubtitleLine {
        SubtitleLine {
            track,
            chars: text
                .chars()
                .map(|ch| RenderedChar { ch, alpha: 1.0 })
                .collect(),
            color: "#ffffff".into(),
        }
    }

    #[test]
    fn find_line_picks_the_right_track() {
        let frame = SubtitleFrame {
            lines: vec![line(Track::Speak, "a"), line(Track::Listen, "b")],
        };
        assert_eq!(find_line(&frame, Track::Speak).unwrap().chars[0].ch, 'a');
        assert_eq!(find_line(&frame, Track::Listen).unwrap().chars[0].ch, 'b');
    }

    #[test]
    fn character_identity_survives_append_and_ttl_prefix_prune() {
        let mut identity = CharIdentity::default();
        let mut next = INITIAL_CHAR_ID;
        let first = identity.update(
            &"abc"
                .chars()
                .map(|ch| RenderedChar { ch, alpha: 1.0 })
                .collect::<Vec<_>>(),
            &mut next,
        );
        let appended = identity.update(
            &"abcd"
                .chars()
                .map(|ch| RenderedChar { ch, alpha: 1.0 })
                .collect::<Vec<_>>(),
            &mut next,
        );
        assert_eq!(&appended[..3], &first[..]);
        let pruned = identity.update(
            &"bcd"
                .chars()
                .map(|ch| RenderedChar { ch, alpha: 0.5 })
                .collect::<Vec<_>>(),
            &mut next,
        );
        assert_eq!(&pruned[..], &appended[1..]);
        assert_eq!(pruned.len(), 3, "布局变化不能删除仍在 TTL 内的字符");
    }

    #[test]
    fn ease_out_layout_midpoint_stays_between_endpoints() {
        let y = interpolate_y(100.0, 52.0, 0.5);
        assert!(y < 100.0 && y > 52.0, "中间帧必须位于起点和终点之间: {y}");
        assert_eq!(interpolate_y(100.0, 52.0, 0.0), 100.0);
        assert_eq!(interpolate_y(100.0, 52.0, 1.0), 52.0);
    }

    #[test]
    fn row_transition_reaches_exact_target_after_duration() {
        let target = StableRow {
            key: 1,
            placed: crate::layout::PlacedRow {
                plate: crate::geom::RectI::new(0, 52, 100, 40),
                first_visible: 0,
                last_visible: 2,
                text_x: 0,
                baseline_y: 30,
            },
            char_ids: vec![1, 2],
        };
        let transition = RowTransition {
            started: Instant::now() - Duration::from_millis(LAYOUT_ANIMATION_MS),
            from: vec![(1, 100.0)],
            target: vec![target],
        };
        assert_eq!(transition.y_at(1, Instant::now()).unwrap(), 52.0);
    }

    #[test]
    fn new_visual_row_has_a_transition_even_when_visible_row_count_is_constant() {
        let old = vec![
            StableRow {
                key: 1,
                placed: crate::layout::PlacedRow {
                    plate: crate::geom::RectI::new(0, 80, 100, 40),
                    first_visible: 0,
                    last_visible: 2,
                    text_x: 0,
                    baseline_y: 30,
                },
                char_ids: vec![1, 2],
            },
            StableRow {
                key: 3,
                placed: crate::layout::PlacedRow {
                    plate: crate::geom::RectI::new(0, 126, 100, 40),
                    first_visible: 2,
                    last_visible: 4,
                    text_x: 0,
                    baseline_y: 30,
                },
                char_ids: vec![3, 4],
            },
        ];
        let next = vec![
            StableRow {
                key: 5,
                placed: crate::layout::PlacedRow {
                    plate: crate::geom::RectI::new(0, 80, 100, 40),
                    first_visible: 0,
                    last_visible: 2,
                    text_x: 0,
                    baseline_y: 30,
                },
                char_ids: vec![5, 6],
            },
            StableRow {
                key: 7,
                placed: crate::layout::PlacedRow {
                    plate: crate::geom::RectI::new(0, 126, 100, 40),
                    first_visible: 2,
                    last_visible: 4,
                    text_x: 0,
                    baseline_y: 30,
                },
                char_ids: vec![7, 8],
            },
        ];
        let changed = layout_changed(&old, &next);
        assert!(changed, "行数不变但行身份变化也必须触发过渡");
    }

    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn draws_two_rows_with_no_invalid_pixels() {
        let settings = SubtitleSettings::default();
        let mut renderer = Renderer::new(&settings, 96).unwrap();
        let frame = SubtitleFrame {
            lines: vec![
                line(Track::Listen, "听人说话"),
                line(Track::Speak, "对外说话"),
            ],
        };
        let mut canvas = Canvas::new(0, 0);
        let out = renderer.draw(
            &mut canvas,
            &FrameInput {
                frame: &frame,
                settings: &settings,
                client_width: 880,
                client_height: 170,
                dpi: 96,
            },
        );
        assert_eq!(canvas.height(), out.client_height);
        assert!(out.client_height > 1);
        assert!(canvas.find_invalid_pixel().is_none());
    }

    #[test]
    #[ignore = "需要真实 GDI 环境"]
    fn zero_alpha_chars_leave_no_text_pixels() {
        let settings = SubtitleSettings {
            background_alpha: 0,
            ..SubtitleSettings::default()
        };
        let mut renderer = Renderer::new(&settings, 96).unwrap();
        let frame = SubtitleFrame {
            lines: vec![SubtitleLine {
                track: Track::Listen,
                chars: "完全透明"
                    .chars()
                    .map(|ch| RenderedChar { ch, alpha: 0.0 })
                    .collect(),
                color: "#ffffff".into(),
            }],
        };
        let mut canvas = Canvas::new(0, 0);
        renderer.draw(
            &mut canvas,
            &FrameInput {
                frame: &frame,
                settings: &settings,
                client_width: 880,
                client_height: 170,
                dpi: 96,
            },
        );
        assert!(canvas.bytes().iter().all(|&v| v == 0));
    }
}
