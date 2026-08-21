//! 内核事件 → 前端单通道 + 落盘 + 开机自启同步。
//!
//! 一个监听器挂到 `runtime.add_listener`，对每条事件做分派：
//! - **所有事件**都转发到前端通道 `voxbridge://event`；
//! - 重活（落盘、悬浮窗换样式、注册表）按事件类型选择性做；
//! - 高频事件（`GateStatus`、`SubtitleDelta`）保证快路径：只转发，不做 IO。

use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::Emitter;
use tauri_plugin_autostart::ManagerExt;
use vox_core::event::Event;
use vox_core::ports::SubtitleView;
use vox_core::runtime::Listener;
use vox_core::settings::SubtitleSettings;

use crate::state::AppState;

/// 前端订阅的唯一事件通道，和 `app/ui/src/api.ts` 里的 `EVENT_CHANNEL` 一致。
const EVENT_CHANNEL: &str = "voxbridge://event";

/// 装配入口。由 `lib.rs` 的 `assemble` 调一次。
///
/// 做两件事：
/// 1. 初始同步——对齐一次开机自启注册表。
/// 2. 挂内核事件监听器，后续所有变动由回调驱动。
pub fn wire(state: &Arc<AppState>, app: tauri::AppHandle) {
    // --- 初始同步 ---
    sync_autostart(&app, state.runtime.settings().autostart);

    // --- 挂监听器 ---
    // 闭包只能捕 `Weak<AppState>`，**不能**捕 `Arc<AppState>`。
    //
    // 捕 Arc 会造出一个确定的引用环：
    //   Inner.listeners → 这个闭包 → Arc<AppState> → AppState.runtime
    //     → Arc<Inner> → 回到 Inner.listeners
    // 环上的引用计数永远降不到 0，于是 `AppState`、`Overlay`、`HotkeyListener`、
    // `PipelineEngine` 的 `Drop` 全都不执行——热键线程会一直轮询到进程被内核
    // 回收，期间还能改账本（那时 `persist.flush()` 已经跑完，改动静默丢失）。
    //
    // 换成 Weak 之后环断了，Drop 链恢复正常。闭包里先 upgrade，拿不到就说明
    // `AppState` 已经没了（进程在退出），直接 return。
    let weak = Arc::downgrade(state);
    let handle = app.clone();

    // 记住上一次的字幕样式设置，只在真的变了时才调 restyle。
    // 用 Mutex 包一份 SubtitleSettings 的克隆。
    let prev_subtitle: Arc<Mutex<SubtitleSettings>> =
        Arc::new(Mutex::new(state.runtime.settings().subtitle.clone()));

    let listener: Listener = Arc::new(move |event: &Event| {
        // ── 转发给前端 ──────────────────────────────────────────────────
        // v2 起前后端设置字段名一致，所有事件都能直接走克隆快路径。
        if let Err(e) = handle.emit(EVENT_CHANNEL, event.clone()) {
            tracing::warn!("转发事件到前端失败：{e}");
        }

        // ── 拿住 AppState ───────────────────────────────────────────────
        // upgrade 失败 = AppState 已析构（进程退出中），后面的活都没意义了。
        // 注意：转发给前端放在 upgrade 之前，那一步不需要 AppState。
        let Some(st) = weak.upgrade() else { return };

        // ── 按事件类型分派 ──────────────────────────────────────────────
        match event {
            Event::SettingsChanged { settings } => {
                // 落盘（persist 内部有去抖，直接调即可）。
                st.persist.save_settings(settings);

                // 同步开机自启注册表（只在不一致时才动）。
                sync_autostart(&handle, settings.autostart);

                // 字幕样式或 geometry 变化时同步原生悬浮窗；visible 也会随设置
                // 一并投递，具体显示切换仍由帧线程负责。
                let need_restyle = {
                    let mut prev = prev_subtitle.lock();
                    let changed = subtitle_style_changed(&prev, &settings.subtitle);
                    if changed {
                        *prev = settings.subtitle.clone();
                    }
                    changed
                };
                if need_restyle {
                    if let Some(overlay) = st.overlay.get() {
                        overlay.restyle(&settings.subtitle);
                    }
                }

                // 托盘的"显示字幕"勾选跟着走。设置也可能是前端或热键改的，
                // 不是只有托盘自己那条路径。
                crate::tray::sync(&st);
            }

            Event::UsageChanged { usage } => {
                st.persist.save_usage(usage);
            }

            Event::GateStatus { pipeline, status } => {
                // 极高频（每 200ms × 3 条），只做最小工作：缓存门状态供快照用。
                // 不落盘、不做快照。
                st.remember_gate(*pipeline, *status);
            }

            Event::PipelineState {
                pipeline,
                state: pipe_state,
            } => {
                // 流水线停了→清掉门缓存，否则 UI 残留最后一格电平。
                if !pipe_state.is_running() {
                    st.forget_gate(*pipeline);
                }
                // 托盘勾选跟着流水线实际状态走——热键、前端、崩溃重连都会走这里。
                crate::tray::sync(&st);
            }

            Event::Notice { .. } => {}

            // 以下事件只需转发（上面已经 emit 过了），不做额外工作。
            // SubtitleDelta 也很频繁，快路径到此结束。
            Event::MicActive { .. } => {}
            Event::SubtitleDelta { .. } => {}
            Event::SubtitleCleared { .. } => {}
            Event::SourceDetected { .. } => {}
            Event::LatencyChanged { .. } => {}
            Event::DevicesChanged => {}
        }
    });

    state.runtime.add_listener(listener);
}

// ---------------------------------------------------------------------------
// 开机自启同步
// ---------------------------------------------------------------------------

/// 我们上一次确认过的 autostart 状态。`-1` = 还不知道。
///
/// 存在的意义是省掉注册表读：`SettingsChanged` 可能连续到达，而这个 listener 跑在
/// 发事件的线程上（可能是音频工作线程，输入队列只有几格，攒着就要丢音频）。
/// 让每条 SettingsChanged 都去读一次注册表是不能接受的。
static AUTOSTART_KNOWN: AtomicI8 = AtomicI8::new(-1);

/// 把设置里的 `autostart` 和注册表实际状态对齐。只在不一致时才写注册表。
/// 失败只 warn：有些企业环境用组策略锁了注册表启动项。
fn sync_autostart(app: &tauri::AppHandle, desired: bool) {
    // 快路径：跟我们已知的状态一致就什么都不做，一次注册表 IO 都不发生。
    // 用户从别处改了注册表我们会漏掉，但那是他自己动的，下次启动会重新对齐。
    if AUTOSTART_KNOWN.load(Ordering::Relaxed) == i8::from(desired) {
        return;
    }

    let manager = app.autolaunch();
    let current = match manager.is_enabled() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("查询开机自启状态失败：{e}");
            return;
        }
    };
    if current == desired {
        AUTOSTART_KNOWN.store(i8::from(desired), Ordering::Relaxed);
        return;
    }
    let result = if desired {
        manager.enable()
    } else {
        manager.disable()
    };
    match result {
        Ok(()) => AUTOSTART_KNOWN.store(i8::from(desired), Ordering::Relaxed),
        Err(e) => {
            // 写失败就不缓存——下次还得再试，否则用户勾了开机自启却永远不生效。
            tracing::warn!("同步开机自启失败（期望={desired}, 当前={current}）：{e}");
        }
    }
}

// ---------------------------------------------------------------------------
// 字幕样式比较
// ---------------------------------------------------------------------------

/// 判断字幕的"样式"部分是否变了。
///
/// `visible` 由 overlay.rs 的帧线程负责；geometry 需要送进原生窗口，
/// 这样设置页的“恢复默认”和拖动回写都能立即生效。
/// 这里关心窗口渲染器直接使用的字段：字体、字号、配色、底衬透明度和几何。
/// 字符生命周期由内核字幕模型立即更新，下一帧自然会带来新的 alpha。
fn subtitle_style_changed(prev: &SubtitleSettings, curr: &SubtitleSettings) -> bool {
    prev.font_family != curr.font_family
        || prev.font_size != curr.font_size
        || prev.speak_color != curr.speak_color
        || prev.listen_color != curr.listen_color
        || prev.background_alpha != curr.background_alpha
        || prev.geometry != curr.geometry
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vox_core::event::{Event, Notice, Pipeline, PipelineState};
    use vox_core::gate::GateStatus;
    use vox_core::subtitle::Track;
    use vox_core::usage::UsageLedger;

    // -- 字幕样式比较 --

    #[test]
    fn same_style_returns_false() {
        let a = SubtitleSettings::default();
        let b = a.clone();
        assert!(!subtitle_style_changed(&a, &b));
    }

    #[test]
    fn font_size_change_detected() {
        let a = SubtitleSettings::default();
        let mut b = a.clone();
        b.font_size = 42;
        assert!(subtitle_style_changed(&a, &b));
    }

    #[test]
    fn visible_change_ignored() {
        let a = SubtitleSettings::default();
        let mut b = a.clone();
        b.visible = !a.visible;
        assert!(!subtitle_style_changed(&a, &b), "visible 不算样式");
    }

    #[test]
    fn geometry_change_detected() {
        let a = SubtitleSettings::default();
        let mut b = a.clone();
        b.geometry = Some(vox_core::settings::OverlayGeometry {
            x: 99,
            y: 99,
            width: 800,
            height: 120,
        });
        assert!(subtitle_style_changed(&a, &b), "geometry 变化需要同步窗口");
    }

    #[test]
    fn timing_change_is_handled_by_subtitle_frames() {
        let a = SubtitleSettings::default();
        let mut b = a.clone();
        b.char_ttl_ms += 100;
        b.char_fade_ms += 50;
        assert!(
            !subtitle_style_changed(&a, &b),
            "时序变化不需要重建窗口渲染器"
        );
    }

    // -- 事件序列化契约：确认 JSON 的 kind 标签和字段名与前端 VoxEvent 一致 --

    /// 辅助：序列化一个事件并返回 JSON Value。
    fn to_json(event: &Event) -> serde_json::Value {
        serde_json::to_value(event).expect("Event 序列化不应失败")
    }

    #[test]
    fn pipeline_state_event_shape() {
        let json = to_json(&Event::PipelineState {
            pipeline: Pipeline::Listen,
            state: PipelineState::Reconnecting,
        });
        assert_eq!(json["kind"], "pipeline_state");
        assert_eq!(json["pipeline"], "listen");
        assert_eq!(json["state"], "reconnecting");
    }

    #[test]
    fn gate_status_event_shape() {
        let json = to_json(&Event::GateStatus {
            pipeline: Pipeline::Speak,
            status: GateStatus {
                kind: vox_core::gate::GateKind::Level,
                state: vox_core::gate::GateState::Speech,
                rms: 0.42,
                active: true,
                ended: false,
            },
        });
        assert_eq!(json["kind"], "gate_status");
        assert_eq!(json["pipeline"], "speak");
        // status 子对象
        let status = &json["status"];
        assert!(status["rms"].is_number());
        assert_eq!(status["active"], true);
        assert_eq!(status["ended"], false);
    }

    #[test]
    fn subtitle_delta_event_shape() {
        let json = to_json(&Event::SubtitleDelta {
            track: Track::Listen,
            text: "你好".into(),
            done: true,
            replace: true,
        });
        assert_eq!(json["kind"], "subtitle_delta");
        assert_eq!(json["track"], "listen");
        assert_eq!(json["text"], "你好");
        assert_eq!(json["done"], true);
        assert_eq!(json["replace"], true);
    }

    #[test]
    fn source_detected_event_shape() {
        let json = to_json(&Event::SourceDetected {
            track: Track::Listen,
            language: "ja".into(),
        });
        assert_eq!(json["kind"], "source_detected");
        assert_eq!(json["track"], "listen");
        assert_eq!(json["language"], "ja");
    }

    #[test]
    fn usage_changed_event_shape() {
        let json = to_json(&Event::UsageChanged {
            usage: Box::new(UsageLedger::default()),
        });
        assert_eq!(json["kind"], "usage_changed");
        // usage 是 #[serde(transparent)]，所以 json["usage"] 是个对象（空 map）。
        assert!(json["usage"].is_object());
    }

    #[test]
    fn notice_event_shape() {
        let json = to_json(&Event::Notice {
            notice: Notice::error("出错了").on(Pipeline::Speak),
        });
        assert_eq!(json["kind"], "notice");
        let notice = &json["notice"];
        assert_eq!(notice["severity"], "error");
        assert_eq!(notice["text"], "出错了");
        assert_eq!(notice["pipeline"], "speak");
    }

    #[test]
    fn mic_active_event_shape() {
        let json = to_json(&Event::MicActive { active: true });
        assert_eq!(json["kind"], "mic_active");
        assert_eq!(json["active"], true);
    }

    #[test]
    fn devices_changed_event_shape() {
        let json = to_json(&Event::DevicesChanged);
        assert_eq!(json["kind"], "devices_changed");
    }

    #[test]
    fn subtitle_cleared_event_shape() {
        let json = to_json(&Event::SubtitleCleared {
            track: Track::Speak,
        });
        assert_eq!(json["kind"], "subtitle_cleared");
        assert_eq!(json["track"], "speak");
    }
}
