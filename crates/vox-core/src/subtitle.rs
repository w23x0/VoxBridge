//! 字幕模型：逐字流入，每个字自己算存活和淡出。
//!
//! 两条流水线各占一行（对外说话暖白、听人说话冷白），同一个悬浮窗上下摆。
//! 模型只管"现在该显示哪些字、每个字多亮"，怎么画是 `vox-overlay-win` 的事。
//!
//! 时间一律用"启动至今的毫秒数"传进来，不在这里读系统时钟——这样测试可以
//! 直接喂时间，也不用管平台。
//!
//! **0 类字**：纯噪声/填充词/无意义发音。转写用一组成对的 [`NOISE_DELIM`]
//! 把 0 类段夹起来；夹在中间的字标记 `is_noise`。开启"0 类字幕"后这些字在
//! Lifetime 结束时不消失，而是一段短淡出后永久留在 `dim_alpha` 的淡灰里。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// 0 类段的定界符：成对出现，夹在中间的就是 0 类段。
pub const NOISE_DELIM: char = '⌀';

/// 0 类字永久淡化的淡出时长，压在 Lifetime 末尾。跟普通字的 fade 独立。
const DIM_FADE_MS: u64 = 300;

/// 哪条流水线的字幕。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    /// 对外说话（暖白）。
    Speak,
    /// 听人说话（冷白）。
    Listen,
}

/// 一个待显示的字符。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitleChar {
    pub ch: char,
    /// 这个字流入的时间（启动至今毫秒）。
    pub born_ms: u64,
    /// 0 类字（纯噪声/填充词/无意义发音）。
    pub is_noise: bool,
}

/// 渲染用的一个字：字形 + 当前不透明度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderedChar {
    pub ch: char,
    /// 0.0..=1.0。
    pub alpha: f32,
}

/// 一行字幕的时间参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitleTiming {
    /// 每个字从流入到开始淡出的总寿命。
    pub char_ttl_ms: u32,
    /// 淡出过程的时长（计入 ttl 内）。
    pub char_fade_ms: u32,
    /// "0 类字幕"开关：开了之后 0 类字永久淡灰保留。
    pub dim_zeros: bool,
    /// 0 类字永久保留时的目标 alpha（0..1）。
    pub dim_alpha: f32,
}

impl Default for SubtitleTiming {
    fn default() -> Self {
        Self {
            char_ttl_ms: 2600,
            char_fade_ms: 900,
            dim_zeros: false,
            dim_alpha: 0.3,
        }
    }
}

/// 单条轨道的字幕流。
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    chars: VecDeque<SubtitleChar>,
    timing: SubtitleTiming,
    /// 当前是否处在未闭合的 0 类段里（``⌀`` 只来了一半，等下半场）。
    noise_open: bool,
    /// 单行最多留多少字，防止对方一直说导致无限涨。
    max_chars: usize,
}

impl SubtitleTrack {
    pub fn new(timing: SubtitleTiming) -> Self {
        Self {
            chars: VecDeque::new(),
            timing,
            noise_open: false,
            max_chars: 400,
        }
    }

    pub fn set_timing(&mut self, timing: SubtitleTiming) {
        self.timing = timing;
    }

    /// 流入一段文字。模型只追加，不做断句——断句是服务端的事。
    ///
    /// ``⌀`` 是 0 类段定界符：成对出现，夹在中间的字（含空格）标记为 0 类。
    /// 增量 push 中段可能半开，用 `noise_open` 保留未闭合状态。
    pub fn push_text(&mut self, text: &str, now_ms: u64) {
        let mut is_noise = self.noise_open;
        for ch in text.chars() {
            if ch == '\r' {
                continue;
            }
            if ch == NOISE_DELIM {
                is_noise = !is_noise;
                continue;
            }
            self.chars.push_back(SubtitleChar {
                ch,
                born_ms: now_ms,
                is_noise,
            });
        }
        self.noise_open = is_noise;
        while self.chars.len() > self.max_chars {
            self.chars.pop_front();
        }
    }

    /// 服务端整句重写了（改译、纠错）时用：**清掉当前这句、换成新的整句**。
    /// 逐字模型是追加的，直接 `push_text` 会把新句叠在旧句尾巴上——订正必须整体换。
    /// 新字符从 `now_ms` 重新计时，整行一起淡出，视觉上是"这一句话被换成了另一句"。
    pub fn replace_text(&mut self, text: &str, now_ms: u64) {
        self.chars.clear();
        self.noise_open = false;
        self.push_text(text, now_ms);
    }

    /// 立刻清空（停流水线、切语言时用）。
    pub fn clear(&mut self) {
        self.chars.clear();
        self.noise_open = false;
    }

    /// 丢掉已经完全透明的字。定时调用，防止无界增长。
    /// 永存的 0 类字不参与过期判定，但跟普通字一样受 `max_chars` 挤出。
    pub fn prune(&mut self, now_ms: u64) {
        let ttl = self.timing.char_ttl_ms as u64;
        let dim_zeros = self.timing.dim_zeros;
        // 永久保留的 0 类字可能夹在普通字前面，不能因为它挡住队首就
        // 停止清理后面的过期字符；队列长度仍由 max_chars 兜底。
        self.chars
            .retain(|c| (dim_zeros && c.is_noise) || now_ms.saturating_sub(c.born_ms) < ttl);
    }

    /// 开关开了的 0 类字：永不主动消失。
    fn persists(&self, c: &SubtitleChar) -> bool {
        self.timing.dim_zeros && c.is_noise
    }

    /// 当前该画的字 + 每个字的不透明度。
    pub fn render(&self, now_ms: u64) -> Vec<RenderedChar> {
        let ttl = self.timing.char_ttl_ms as u64;
        let fade = self.timing.char_fade_ms.min(self.timing.char_ttl_ms) as u64;
        let fade_start = ttl.saturating_sub(fade);
        let dim_fade = DIM_FADE_MS.min(ttl);
        let dim_fade_start = ttl.saturating_sub(dim_fade);
        let dim_alpha = self.timing.dim_alpha.clamp(0.0, 1.0);
        self.chars
            .iter()
            .filter_map(|c| {
                let age = now_ms.saturating_sub(c.born_ms);
                if self.persists(c) {
                    // 0 类字：Lifetime 前后一段短淡出，之后恒定在 dim_alpha。
                    if age < dim_fade_start {
                        return Some(RenderedChar {
                            ch: c.ch,
                            alpha: 1.0,
                        });
                    }
                    let t = ((age - dim_fade_start).min(dim_fade)) as f32 / dim_fade.max(1) as f32;
                    return Some(RenderedChar {
                        ch: c.ch,
                        alpha: (1.0 - t).max(0.0) * (1.0 - dim_alpha) + dim_alpha,
                    });
                }
                if age >= ttl {
                    return None;
                }
                let alpha = if age <= fade_start || fade == 0 {
                    1.0
                } else {
                    1.0 - (age - fade_start) as f32 / fade as f32
                };
                Some(RenderedChar {
                    ch: c.ch,
                    alpha: alpha.clamp(0.0, 1.0),
                })
            })
            .collect()
    }

    /// 当前该画的纯文本（不含透明度），给 UI 的历史面板用。
    pub fn text(&self, now_ms: u64) -> String {
        self.render(now_ms).into_iter().map(|c| c.ch).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

/// 两条轨道合起来的字幕状态。
#[derive(Debug, Clone)]
pub struct Subtitles {
    pub speak: SubtitleTrack,
    pub listen: SubtitleTrack,
}

impl Subtitles {
    pub fn new(timing: SubtitleTiming) -> Self {
        Self {
            speak: SubtitleTrack::new(timing),
            listen: SubtitleTrack::new(timing),
        }
    }

    pub fn track_mut(&mut self, track: Track) -> &mut SubtitleTrack {
        match track {
            Track::Speak => &mut self.speak,
            Track::Listen => &mut self.listen,
        }
    }

    pub fn track(&self, track: Track) -> &SubtitleTrack {
        match track {
            Track::Speak => &self.speak,
            Track::Listen => &self.listen,
        }
    }

    pub fn set_timing(&mut self, timing: SubtitleTiming) {
        self.speak.set_timing(timing);
        self.listen.set_timing(timing);
    }

    pub fn prune(&mut self, now_ms: u64) {
        self.speak.prune(now_ms);
        self.listen.prune(now_ms);
    }

    /// 两行都空 = 悬浮窗可以完全不画。
    pub fn is_empty(&self) -> bool {
        self.speak.is_empty() && self.listen.is_empty()
    }
}

impl Default for Subtitles {
    fn default() -> Self {
        Self::new(SubtitleTiming::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> SubtitleTiming {
        SubtitleTiming {
            char_ttl_ms: 1000,
            char_fade_ms: 200,
            dim_zeros: false,
            dim_alpha: 0.3,
        }
    }

    #[test]
    fn chars_are_fully_opaque_before_fade_starts() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("你好", 0);
        let r = t.render(100);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|c| c.alpha == 1.0));
        assert_eq!(t.text(100), "你好");
    }

    #[test]
    fn alpha_ramps_down_during_fade_window() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("A", 0);
        // fade 从 800ms 开始，1000ms 归零。
        assert_eq!(t.render(800)[0].alpha, 1.0);
        let mid = t.render(900)[0].alpha;
        assert!((mid - 0.5).abs() < 0.01, "半程应该约 0.5，实际 {mid}");
        assert!(t.render(1000).is_empty(), "到 ttl 就不画了");
    }

    #[test]
    fn later_chars_outlive_earlier_ones() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("早", 0);
        t.push_text("晚", 600);
        let r = t.render(1000);
        assert_eq!(r.len(), 1, "先来的字先消失");
        assert_eq!(r[0].ch, '晚');
    }

    #[test]
    fn prune_drops_dead_chars_only() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("早", 0);
        t.push_text("晚", 600);
        t.prune(1000);
        assert_eq!(t.text(1000), "晚");
        assert!(!t.is_empty());
        t.prune(1600);
        assert!(t.is_empty());
    }

    #[test]
    fn prune_removes_expired_chars_after_persistent_noise() {
        let mut timing = timing();
        timing.dim_zeros = true;
        let mut t = SubtitleTrack::new(timing);
        // 0 类字在队首时，后面的普通字也必须能被清掉。
        t.push_text("⌀噪声⌀", 0);
        t.push_text("旧普通字", 0);

        t.prune(10_000);

        let text: String = t.chars.iter().map(|c| c.ch).collect();
        assert_eq!(text, "噪声");
    }

    #[test]
    fn length_is_bounded() {
        let mut t = SubtitleTrack::new(timing());
        for i in 0..2000u64 {
            t.push_text("字", i);
        }
        assert!(t.chars.len() <= t.max_chars, "单行长度必须有上限");
    }

    #[test]
    fn zero_fade_means_hard_cut() {
        let mut t = SubtitleTrack::new(SubtitleTiming {
            char_ttl_ms: 500,
            char_fade_ms: 0,
            dim_zeros: false,
            dim_alpha: 0.3,
        });
        t.push_text("X", 0);
        assert_eq!(t.render(499)[0].alpha, 1.0);
        assert!(t.render(500).is_empty());
    }

    #[test]
    fn two_tracks_are_independent() {
        let mut s = Subtitles::new(timing());
        s.track_mut(Track::Speak).push_text("hello", 0);
        s.track_mut(Track::Listen).push_text("你好", 0);
        assert_eq!(s.track(Track::Speak).text(0), "hello");
        assert_eq!(s.track(Track::Listen).text(0), "你好");
        s.track_mut(Track::Speak).clear();
        assert!(s.track(Track::Speak).is_empty());
        assert!(!s.is_empty(), "只清一条，另一条还在");
        s.track_mut(Track::Listen).clear();
        assert!(s.is_empty());
    }

    #[test]
    fn carriage_returns_are_dropped() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("a\r\nb", 0);
        assert_eq!(t.text(0), "a\nb");
    }

    #[test]
    fn replace_text_swaps_the_whole_line_instead_of_appending() {
        let mut t = SubtitleTrack::new(timing());
        t.push_text("错误句子", 0);
        assert_eq!(t.text(0), "错误句子");
        t.replace_text("订正句子", 100);
        assert_eq!(t.text(100), "订正句子", "整行替换，不能残留旧字");
        // 新字从 100ms 重新计时，整行一起变亮（不是新字亮、旧字半死）。
        assert_eq!(t.render(100).len(), 4);
        assert!(
            t.render(100).iter().all(|c| c.alpha == 1.0),
            "替换后整行同一时刻出生，透明度该一致"
        );
        // 换回去也是整体替换。
        t.replace_text("再改一次", 200);
        assert_eq!(t.text(200), "再改一次");
    }

    #[test]
    fn noise_delim_marks_zero_class_chars() {
        let mut timing = timing();
        timing.dim_zeros = true;
        let mut t = SubtitleTrack::new(timing);
        t.push_text("正常⌀噪声⌀正常后", 0);
        let kinds: Vec<bool> = t.chars.iter().map(|c| c.is_noise).collect();
        // ⌀ 本身不显示，夹在中间的两个字被标记。
        assert_eq!(kinds, [false, false, true, true, false, false, false]);
        assert_eq!(t.text(0), "正常噪声正常后");
    }

    #[test]
    fn noise_state_carries_across_pushes_until_blocked() {
        let mut timing = timing();
        timing.dim_zeros = true;
        let mut t = SubtitleTrack::new(timing);
        t.push_text("前置⌀开始", 0);
        let is_noise: Vec<(char, bool)> = t.chars.iter().map(|c| (c.ch, c.is_noise)).collect();
        assert_eq!(
            is_noise,
            [('前', false), ('置', false), ('开', true), ('始', true)],
            "0 类段还没有拦截结束，后续的 push 依然算 0 类"
        );
    }

    #[test]
    fn noise_persists_after_lifetime_when_dim_zeros_on() {
        let mut timing = timing();
        timing.dim_zeros = true;
        timing.dim_alpha = 0.3;
        let mut t = SubtitleTrack::new(timing);
        t.push_text("消⌀留⌀", 0);
        // ttl = 1000：普通字消失，0 类字稳定停在 dim_alpha。
        let r = t.render(10_000);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ch, '留');
        let gap = r[0].alpha - 0.3;
        assert!(
            gap.abs() < 1e-6,
            "该稳定在 dim_alpha=0.3，实际 {}",
            r[0].alpha
        );
        // prune 也不该把它拿掉。
        t.prune(10_000);
        assert!(!t.is_empty());
        // 关掉开关之后，保留的字回归普通寿命被正常清掉。
        t.set_timing(SubtitleTiming {
            dim_zeros: false,
            ..timing
        });
        t.prune(10_000);
        assert!(t.is_empty(), "关掉选项后保留的字该被清理");
    }

    #[test]
    fn normal_char_still_disappears_when_dim_zeros_on() {
        let mut timing = timing();
        timing.dim_zeros = true;
        let mut t = SubtitleTrack::new(timing);
        t.push_text("普通字", 0);
        assert!(t.render(10_000).is_empty(), "非 0 类字不受永存影响");
    }
}
