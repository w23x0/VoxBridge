//! 纯显示悬浮字幕窗的装配：spawn 悬浮窗 + 字幕帧线程。
//!
//! 窗口自带线程和消息泵，主线程只属于 Tauri 事件循环。字幕帧线程按约 30fps
//! 计算淡出；有内容时持续提交以完成换行上移动画，空内容或隐藏时退化到低频轮询。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use vox_core::ports::{SubtitleFrame, SubtitleView};
use vox_overlay_win::Overlay;

use crate::state::AppState;

/// 有内容且淡出可能变化时的帧间隔（约 30fps）。
const FRAME_INTERVAL_ACTIVE: Duration = Duration::from_millis(33);
/// 连续空帧超过此阈值后切入省电模式。
const IDLE_THRESHOLD: u32 = 10;
/// 空内容或悬浮窗隐藏时的轮询间隔。
const FRAME_INTERVAL_IDLE: Duration = Duration::from_millis(200);

/// 帧线程停止旗。进程退出时调 `stop()` 来置位。
static STOP: AtomicBool = AtomicBool::new(false);

/// 字幕帧线程句柄，`stop()` 里 join。
static FRAME_THREAD: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>> =
    parking_lot::Mutex::new(None);

/// 让帧线程收工并等它退出（最多一个空闲轮询周期）。
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    if let Some(h) = FRAME_THREAD.lock().take() {
        let _ = h.join();
    }
}

/// 起悬浮窗 + 字幕帧线程。失败不致命——设置窗照样能用。
pub fn start(state: &Arc<AppState>) {
    let settings = state.runtime.settings();
    let rt = state.runtime.clone();
    let geometry_rt = rt.clone();
    let geometry_callback = std::sync::Arc::new(move |geometry| {
        // 几何回调发生在 Win32 悬浮窗线程；这里只更新 Runtime，持久化和
        // 其他监听器仍沿用设置事件的现有路径，不跨线程直接碰 Tauri 状态。
        geometry_rt.update_settings(|s| s.subtitle.geometry = Some(geometry));
    });

    let overlay = match Overlay::spawn_with_geometry(&settings.subtitle, Some(geometry_callback)) {
        Ok(o) => o,
        Err(e) => {
            state
                .runtime
                .notify(vox_core::event::Notice::warning(format!(
                    "悬浮字幕窗未能启动：{e}"
                )));
            return;
        }
    };

    let _ = state.overlay.set(Arc::clone(&overlay));

    let frame_rt = rt.clone();
    let notify_rt = rt.clone();
    let frame_thread = std::thread::Builder::new()
        .name("vox-subtitle".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                subtitle_loop(frame_rt, overlay)
            }));
            if result.is_err() {
                notify_rt.notify(vox_core::event::Notice::error(
                    "字幕刷新线程异常退出，悬浮窗内容不再更新。重启应用可恢复。".to_string(),
                ));
            }
        })
        .ok();
    if frame_thread.is_none() {
        state.runtime.notify(vox_core::event::Notice::warning(
            "字幕刷新线程未能启动，悬浮窗不会更新内容".to_string(),
        ));
    }
    *FRAME_THREAD.lock() = frame_thread;
}

/// 字幕帧循环。有内容时每帧提交，保证渲染器的平滑上移不会停在半路；
/// 空帧仍按内容变化去重。
fn subtitle_loop(rt: vox_core::Runtime, overlay: Arc<Overlay>) {
    let mut consecutive_empty: u32 = 0;
    let mut prev_visible: Option<bool> = None;
    let mut previous_frame: Option<SubtitleFrame> = None;

    loop {
        if STOP.load(Ordering::Relaxed) || !overlay.is_running() {
            break;
        }

        let visible = rt.subtitle_visible();
        if prev_visible != Some(visible) {
            if visible {
                overlay.show();
            } else {
                overlay.hide();
                // 重新显示时必须强制推一帧，不能沿用隐藏前的去重基线。
                previous_frame = None;
            }
            prev_visible = Some(visible);
        }

        if !visible {
            std::thread::sleep(FRAME_INTERVAL_IDLE);
            continue;
        }

        rt.prune_subtitles();
        let frame = rt.subtitle_frame();
        let has_content = !frame.lines.is_empty();

        if frame_changed(previous_frame.as_ref(), &frame) || has_content {
            overlay.render(frame.clone());
            previous_frame = Some(frame);
        }

        if has_content {
            consecutive_empty = 0;
        } else {
            consecutive_empty = consecutive_empty.saturating_add(1);
        }
        std::thread::sleep(sleep_duration(consecutive_empty));
    }
}

fn frame_changed(previous: Option<&SubtitleFrame>, current: &SubtitleFrame) -> bool {
    previous != Some(current)
}

fn sleep_duration(consecutive_empty: u32) -> Duration {
    if consecutive_empty > IDLE_THRESHOLD {
        FRAME_INTERVAL_IDLE
    } else {
        FRAME_INTERVAL_ACTIVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vox_core::ports::SubtitleLine;
    use vox_core::subtitle::{RenderedChar, Track};

    fn frame(alpha: f32) -> SubtitleFrame {
        SubtitleFrame {
            lines: vec![SubtitleLine {
                track: Track::Listen,
                chars: vec![RenderedChar { ch: '字', alpha }],
                color: "#eef6ff".into(),
            }],
        }
    }

    #[test]
    fn first_frame_is_rendered() {
        assert!(frame_changed(None, &frame(1.0)));
    }

    #[test]
    fn identical_frame_is_skipped() {
        let old = frame(1.0);
        assert!(!frame_changed(Some(&old), &old));
    }

    #[test]
    fn alpha_change_is_rendered() {
        let old = frame(1.0);
        assert!(frame_changed(Some(&old), &frame(0.8)));
    }

    #[test]
    fn sleep_duration_active_when_below_threshold() {
        assert_eq!(sleep_duration(0), FRAME_INTERVAL_ACTIVE);
        assert_eq!(sleep_duration(IDLE_THRESHOLD), FRAME_INTERVAL_ACTIVE);
    }

    #[test]
    fn sleep_duration_idle_when_above_threshold() {
        assert_eq!(sleep_duration(IDLE_THRESHOLD + 1), FRAME_INTERVAL_IDLE);
        assert_eq!(sleep_duration(1000), FRAME_INTERVAL_IDLE);
    }
}
