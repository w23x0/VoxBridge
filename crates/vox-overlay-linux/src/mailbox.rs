//! 悬浮窗的"最新状态邮箱"：帧线程写、主线程读。
//!
//! 跟 Windows 侧同一个形状（`vox-overlay-win/src/window.rs` 的 `Mailbox`）：**后写覆盖先写**。
//! 字幕一秒来几十帧，排队只会让窗口画一堆已经过期的帧；留最新那一份就够。
//!
//! 差别只有一个：Windows 那边叫醒窗口线程靠 `PostMessage`，这边靠
//! `glib::MainContext::invoke`（见 `window.rs`）。

use vox_core::ports::SubtitleFrame;
use vox_core::settings::SubtitleSettings;

pub struct Mailbox {
    /// 最新一帧；`None` = 还没收到过任何帧。
    pub frame: Option<SubtitleFrame>,
    /// 要不要显示（`SubtitleView::show` / `hide`）。
    pub visible: bool,
    /// 最新外观设置（字体、字号、底衬透明度）。
    pub settings: SubtitleSettings,
}

impl Mailbox {
    pub fn new(settings: SubtitleSettings) -> Self {
        Self {
            frame: None,
            visible: false,
            settings,
        }
    }

    pub fn set_frame(&mut self, frame: SubtitleFrame) {
        self.frame = Some(frame);
    }

    pub fn set_settings(&mut self, settings: &SubtitleSettings) {
        self.settings = settings.clone();
    }
}
