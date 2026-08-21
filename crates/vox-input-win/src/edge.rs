//! 边沿检测状态机。
//!
//! 纯逻辑、无 FFI、可单测。拿一组"当前按下的 VK 集合"做输入，
//! 输出"哪些绑定发生了 Pressed / Released 转变"。

use vox_core::hotkey::Hotkey;
use vox_core::ports::{HotkeyBindings, HotkeyEvent};

/// 一个绑定槽位的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// 没按下。
    Up,
    /// 主键已按下（修饰也满足），已发过 Pressed。
    Down,
}

/// 一条被监视的绑定。
#[derive(Debug, Clone)]
struct Slot {
    /// 主键 VK。
    main_vk: u16,
    /// 修饰键 VK 列表。
    modifier_vks: Vec<u16>,
    /// 按下时发什么。
    press_event: HotkeyEvent,
    /// 松开时发什么（无 release 语义的槽位为 None，如 Listen）。
    release_event: Option<HotkeyEvent>,
    state: SlotState,
}

/// 边沿追踪器。每次调用 `update` 给出当前各 VK 的按下状态，
/// 返回本轮产生的事件列表。
pub struct EdgeTracker {
    slots: Vec<Slot>,
}

impl EdgeTracker {
    /// 从绑定集构建。绑定无效（VK 解析失败）的槽位直接忽略。
    pub fn from_bindings(bindings: &HotkeyBindings) -> Self {
        let mut slots = Vec::new();
        if let Some(ref hk) = bindings.speak {
            if let Some(slot) = Self::build_slot(
                hk,
                HotkeyEvent::SpeakPressed,
                Some(HotkeyEvent::SpeakReleased),
            ) {
                slots.push(slot);
            }
        }
        if let Some(ref hk) = bindings.listen {
            if let Some(slot) = Self::build_slot(hk, HotkeyEvent::ListenPressed, None) {
                slots.push(slot);
            }
        }
        Self { slots }
    }

    fn build_slot(
        hk: &Hotkey,
        press_event: HotkeyEvent,
        release_event: Option<HotkeyEvent>,
    ) -> Option<Slot> {
        let main_vk = hk.key_vk()?;
        Some(Slot {
            main_vk,
            modifier_vks: hk.modifier_vks(),
            press_event,
            release_event,
            state: SlotState::Up,
        })
    }

    /// 给定一个判断函数 `is_down(vk) -> bool`，推进状态机。
    /// 返回本轮触发的事件。
    pub fn update(&mut self, is_down: impl Fn(u16) -> bool) -> Vec<HotkeyEvent> {
        let mut events = Vec::new();
        for slot in &mut self.slots {
            // 主键当前是否物理按下
            let main_down = is_down(slot.main_vk);
            // 修饰键全部满足
            let mods_ok = slot.modifier_vks.iter().all(|&vk| is_down(vk));

            // 组合满足 = 主键按下 + 所有修饰都按下
            let combo_active = main_down && mods_ok;

            match slot.state {
                SlotState::Up if combo_active => {
                    slot.state = SlotState::Down;
                    events.push(slot.press_event);
                }
                SlotState::Down if !main_down => {
                    // 释放判断只看主键——修饰先松不触发 release，
                    // 主键松了才算真正放手。这跟旧版行为一致。
                    slot.state = SlotState::Up;
                    if let Some(ev) = slot.release_event {
                        events.push(ev);
                    }
                }
                _ => {}
            }
        }
        events
    }

    /// 重新绑定：丢弃旧状态，不对旧绑定补发 Released。
    ///
    /// 设计决策：rebind 时若旧键正处于 Down 状态，我们不补发 Released。
    /// 原因：用户改了绑定，语义上新旧是两件事。内核会在 rebind 时自行
    /// 收摊上一个动作（如果需要的话），不靠输入层补事件。
    pub fn rebind(&mut self, bindings: &HotkeyBindings) {
        *self = Self::from_bindings(bindings);
    }
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use vox_core::hotkey::Hotkey;

    /// 辅助：构造一个简单的 is_down 闭包。
    fn down_set(keys: &[u16]) -> impl Fn(u16) -> bool {
        let set: HashSet<u16> = keys.iter().copied().collect();
        move |vk| set.contains(&vk)
    }

    fn speak_only(key: &str) -> HotkeyBindings {
        HotkeyBindings {
            speak: Some(Hotkey::plain(key)),
            listen: None,
        }
    }

    #[test]
    fn press_and_release_yields_one_each() {
        let mut tracker = EdgeTracker::from_bindings(&speak_only("V"));
        let vk_v = 0x56u16; // 'V'

        // 按下
        let ev = tracker.update(down_set(&[vk_v]));
        assert_eq!(ev, vec![HotkeyEvent::SpeakPressed]);

        // 持续按住——不应重复
        let ev = tracker.update(down_set(&[vk_v]));
        assert!(ev.is_empty());

        // 松开
        let ev = tracker.update(down_set(&[]));
        assert_eq!(ev, vec![HotkeyEvent::SpeakReleased]);

        // 已经松开——不重复
        let ev = tracker.update(down_set(&[]));
        assert!(ev.is_empty());
    }

    #[test]
    fn auto_repeat_produces_no_extra_events() {
        let mut tracker = EdgeTracker::from_bindings(&speak_only("V"));
        let vk_v = 0x56u16;

        // 模拟键盘自动重复：每 poll 都报 down
        tracker.update(down_set(&[vk_v])); // Pressed
        for _ in 0..100 {
            let ev = tracker.update(down_set(&[vk_v]));
            assert!(ev.is_empty(), "auto-repeat 不应产生额外事件");
        }
        let ev = tracker.update(down_set(&[]));
        assert_eq!(ev, vec![HotkeyEvent::SpeakReleased]);
    }

    #[test]
    fn modifier_combo_fires_only_when_all_held() {
        let bindings = HotkeyBindings {
            speak: Some(Hotkey {
                ctrl: true,
                alt: true,
                shift: false,
                key: "T".into(),
            }),
            listen: None,
        };
        let mut tracker = EdgeTracker::from_bindings(&bindings);
        let vk_t = b'T' as u16;
        let vk_ctrl = 0x11u16;
        let vk_alt = 0x12u16;

        // 只按主键——不触发
        let ev = tracker.update(down_set(&[vk_t]));
        assert!(ev.is_empty());

        // 加上一个修饰——仍不触发
        let ev = tracker.update(down_set(&[vk_t, vk_ctrl]));
        assert!(ev.is_empty());

        // 全部按下——触发
        let ev = tracker.update(down_set(&[vk_t, vk_ctrl, vk_alt]));
        assert_eq!(ev, vec![HotkeyEvent::SpeakPressed]);

        // 松掉修饰但主键仍按——不 release（只看主键）
        let ev = tracker.update(down_set(&[vk_t]));
        assert!(ev.is_empty());

        // 松掉主键——release
        let ev = tracker.update(down_set(&[]));
        assert_eq!(ev, vec![HotkeyEvent::SpeakReleased]);
    }

    #[test]
    fn modifier_only_press_emits_nothing() {
        let bindings = HotkeyBindings {
            speak: Some(Hotkey {
                ctrl: true,
                alt: false,
                shift: false,
                key: "D".into(),
            }),
            listen: None,
        };
        let mut tracker = EdgeTracker::from_bindings(&bindings);

        // 只按 Ctrl
        let ev = tracker.update(down_set(&[0x11]));
        assert!(ev.is_empty());
        let ev = tracker.update(down_set(&[]));
        assert!(ev.is_empty());
    }

    #[test]
    fn rebind_mid_hold_does_not_emit_phantom_release() {
        let mut tracker = EdgeTracker::from_bindings(&speak_only("V"));
        let vk_v = 0x56u16;

        // 按下 V
        tracker.update(down_set(&[vk_v]));

        // 用户此时改绑定到 B——旧的 Down 状态被丢弃，不补发 Released
        tracker.rebind(&speak_only("B"));

        // V 仍然按着，但新绑定不关心 V
        let ev = tracker.update(down_set(&[vk_v]));
        assert!(ev.is_empty(), "rebind 后不应对旧键补发 Released");

        // 松开 V 也不应有事件
        let ev = tracker.update(down_set(&[]));
        assert!(ev.is_empty());
    }

    #[test]
    fn listen_has_no_release_event() {
        let bindings = HotkeyBindings {
            speak: None,
            listen: Some(Hotkey::plain("L")),
        };
        let mut tracker = EdgeTracker::from_bindings(&bindings);
        let vk_l = b'L' as u16;

        let ev = tracker.update(down_set(&[vk_l]));
        assert_eq!(ev, vec![HotkeyEvent::ListenPressed]);

        // 松开——Listen 没有 Released 事件
        let ev = tracker.update(down_set(&[]));
        assert!(ev.is_empty());
    }
}
