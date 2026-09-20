//! 快捷键：修饰键 + 主键的组合、规范化与显示，以及**平台无关的边沿状态机**。
//!
//! 内核只认**键名**（`"V"` / `"F8"` / `"Space"` / `"XButton2"`）和这个组合的形状；
//! 键名到**键码**的映射是平台相关的（Windows 是 VK，Linux 是 evdev 的 `KEY_*`），
//! 放在各自的 `vox-input-win` / `vox-input-linux` 里。这样内核里不再出现任何
//! Windows 专有的码值。

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::catalog::normalize_key;
use crate::ports::{HotkeyBindings, HotkeyEvent};

/// 一个快捷键组合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    /// 规范化后的主键名，如 `"V"` / `"F8"` / `"Space"` / `"XButton2"`。
    pub key: String,
}

impl Hotkey {
    /// 无修饰的单键。
    pub fn plain(key: &str) -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
            key: key.to_string(),
        }
    }

    /// 主键是不是合法键名。
    pub fn is_valid(&self) -> bool {
        normalize_key(&self.key).is_some()
    }

    /// 收敛成合法组合：主键非法时退回 `fallback`。
    pub fn normalized(&self, fallback: &Hotkey) -> Hotkey {
        match normalize_key(&self.key) {
            Some(key) => Hotkey {
                ctrl: self.ctrl,
                alt: self.alt,
                shift: self.shift,
                key,
            },
            None => fallback.clone(),
        }
    }

    /// 两个热键是否会同时被按出来（键相同且修饰键集合相同 = 冲突）。
    pub fn conflicts_with(&self, other: &Hotkey) -> bool {
        self.key.eq_ignore_ascii_case(&other.key)
            && self.ctrl == other.ctrl
            && self.alt == other.alt
            && self.shift == other.shift
    }

    /// 人看的写法，如 `Ctrl + Alt + T`。
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(4);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        let key_label = match self.key.as_str() {
            "Space" => "空格",
            "XButton1" => "鼠标侧键1",
            "XButton2" => "鼠标侧键2",
            other => other,
        };
        parts.push(key_label);
        parts.join(" + ")
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl Default for Hotkey {
    fn default() -> Self {
        Self::plain("V")
    }
}

// --- 边沿检测（两个平台共用） ------------------------------------------------

/// 一条绑定解析出来的键码。
///
/// 键码空间是平台相关的（Windows：VK；Linux：evdev `KEY_*`），所以解析在各自的
/// `-win` / `-linux` crate 里做，内核只吃这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingCode {
    /// 主键码。
    pub main: u16,
    /// 修饰键：**每组内是"或"、组间是"与"**。
    ///
    /// Windows 每个修饰只有一个 VK；Linux 的 Ctrl 有左右两个 `KEY_*`，必须"或"起来，
    /// 所以这里不是扁平的码列表。
    pub modifier_groups: Vec<Vec<u16>>,
    /// 按下时发什么。
    pub press: HotkeyEvent,
    /// 松开时发什么（无 release 语义的槽位为 `None`，如 Listen）。
    pub release: Option<HotkeyEvent>,
}

/// 把绑定集解析成键码。
///
/// "有哪些槽位、哪个槽位带 release" 这套规矩留在内核（两平台一致），
/// `main_code` / `modifier_groups` 两张码表由平台提供。
pub fn resolve_bindings(
    bindings: &HotkeyBindings,
    main_code: impl Fn(&str) -> Option<u16>,
    modifier_groups: impl Fn(&Hotkey) -> Vec<Vec<u16>>,
) -> Vec<BindingCode> {
    let mut out = Vec::new();
    if let Some(hotkey) = bindings.speak.as_ref() {
        if let Some(main) = main_code(&hotkey.key) {
            out.push(BindingCode {
                main,
                modifier_groups: modifier_groups(hotkey),
                press: HotkeyEvent::SpeakPressed,
                release: Some(HotkeyEvent::SpeakReleased),
            });
        }
    }
    if let Some(hotkey) = bindings.listen.as_ref() {
        if let Some(main) = main_code(&hotkey.key) {
            out.push(BindingCode {
                main,
                modifier_groups: modifier_groups(hotkey),
                press: HotkeyEvent::ListenPressed,
                release: None,
            });
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// 没按下。
    Up,
    /// 主键已按下（修饰也满足），已发过 Pressed。
    Down,
}

#[derive(Debug, Clone)]
struct Slot {
    code: BindingCode,
    state: SlotState,
}

/// 边沿追踪器：吃"当前哪些码是按下的"，吐 Pressed / Released。
///
/// 纯逻辑、无 FFI、可单测——Windows 用它配 25 ms 轮询，Linux 用它配事件回调
/// （每次按键状态变化调一次 `update`，边沿一样准）。
pub struct EdgeTracker {
    slots: Vec<Slot>,
}

impl EdgeTracker {
    pub fn new(codes: Vec<BindingCode>) -> Self {
        Self {
            slots: codes
                .into_iter()
                .map(|code| Slot {
                    code,
                    state: SlotState::Up,
                })
                .collect(),
        }
    }

    /// 给定一个判断函数 `is_down(code) -> bool`，推进状态机，返回本轮事件。
    pub fn update(&mut self, is_down: impl Fn(u16) -> bool) -> Vec<HotkeyEvent> {
        let mut events = Vec::new();
        for slot in &mut self.slots {
            let main_down = is_down(slot.code.main);
            // 每组修饰里有一个按下就算这组满足。
            let mods_ok = slot
                .code
                .modifier_groups
                .iter()
                .all(|group| group.iter().any(|&code| is_down(code)));
            let combo_active = main_down && mods_ok;

            match slot.state {
                SlotState::Up if combo_active => {
                    slot.state = SlotState::Down;
                    events.push(slot.code.press);
                }
                SlotState::Down if !main_down => {
                    // 释放判断只看主键——修饰先松不触发 release，主键松了才算放手。
                    slot.state = SlotState::Up;
                    if let Some(event) = slot.code.release {
                        events.push(event);
                    }
                }
                _ => {}
            }
        }
        events
    }

    /// 重新绑定：丢弃旧状态，**不**对旧绑定补发 Released。
    ///
    /// 用户改了绑定，语义上新旧是两件事；内核在 rebind 时自己收摊上一个动作，
    /// 不靠输入层补事件。
    pub fn rebind(&mut self, codes: Vec<BindingCode>) {
        *self = Self::new(codes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_key_has_no_modifiers() {
        let hk = Hotkey::plain("V");
        assert!(hk.is_valid());
        assert_eq!(hk.label(), "V");
    }

    #[test]
    fn combo_label_and_modifier_order() {
        let hk = Hotkey {
            ctrl: true,
            alt: true,
            shift: false,
            key: "T".into(),
        };
        assert_eq!(hk.label(), "Ctrl + Alt + T");
    }

    #[test]
    fn mouse_and_space_labels_are_chinese() {
        assert_eq!(Hotkey::plain("Space").label(), "空格");
        assert_eq!(Hotkey::plain("XButton2").label(), "鼠标侧键2");
    }

    #[test]
    fn invalid_key_falls_back() {
        let fallback = Hotkey::plain("V");
        let bad = Hotkey {
            ctrl: true,
            alt: false,
            shift: false,
            key: "Tab".into(),
        };
        assert!(!bad.is_valid());
        assert_eq!(bad.normalized(&fallback), fallback);

        let lower = Hotkey::plain("f8");
        assert_eq!(lower.normalized(&fallback).key, "F8");
    }

    #[test]
    fn conflict_needs_same_key_and_same_modifiers() {
        let a = Hotkey::plain("T");
        let b = Hotkey {
            ctrl: true,
            alt: false,
            shift: false,
            key: "T".into(),
        };
        assert!(!a.conflicts_with(&b));
        assert!(a.conflicts_with(&Hotkey::plain("t")));
    }

    // --- 边沿状态机 -----------------------------------------------------------

    /// 测试用码表：A-Z 就用 ASCII，跟 Windows 侧那张表一致，方便读。
    fn main_code(name: &str) -> Option<u16> {
        let mut chars = name.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        ch.is_ascii_uppercase().then_some(ch as u16)
    }

    fn modifier_groups(hotkey: &Hotkey) -> Vec<Vec<u16>> {
        let mut groups = Vec::new();
        if hotkey.ctrl {
            groups.push(vec![0x11]);
        }
        if hotkey.alt {
            groups.push(vec![0x12]);
        }
        if hotkey.shift {
            groups.push(vec![0x10]);
        }
        groups
    }

    fn tracker(bindings: &HotkeyBindings) -> EdgeTracker {
        EdgeTracker::new(resolve_bindings(bindings, main_code, modifier_groups))
    }

    fn down_set(keys: &[u16]) -> impl Fn(u16) -> bool {
        let set: std::collections::HashSet<u16> = keys.iter().copied().collect();
        move |code| set.contains(&code)
    }

    fn speak_only(key: &str) -> HotkeyBindings {
        HotkeyBindings {
            speak: Some(Hotkey::plain(key)),
            listen: None,
        }
    }

    #[test]
    fn press_and_release_yields_one_each() {
        let mut tracker = tracker(&speak_only("V"));
        let code_v = b'V' as u16;

        assert_eq!(
            tracker.update(down_set(&[code_v])),
            vec![HotkeyEvent::SpeakPressed]
        );
        assert!(tracker.update(down_set(&[code_v])).is_empty(), "按住不重复");
        assert_eq!(
            tracker.update(down_set(&[])),
            vec![HotkeyEvent::SpeakReleased]
        );
        assert!(tracker.update(down_set(&[])).is_empty(), "松开不重复");
    }

    #[test]
    fn auto_repeat_produces_no_extra_events() {
        let mut tracker = tracker(&speak_only("V"));
        let code_v = b'V' as u16;
        tracker.update(down_set(&[code_v]));
        for _ in 0..100 {
            assert!(tracker.update(down_set(&[code_v])).is_empty());
        }
        assert_eq!(
            tracker.update(down_set(&[])),
            vec![HotkeyEvent::SpeakReleased]
        );
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
        let mut tracker = tracker(&bindings);
        let (t, ctrl, alt) = (b'T' as u16, 0x11u16, 0x12u16);

        assert!(tracker.update(down_set(&[t])).is_empty(), "只有主键不触发");
        assert!(
            tracker.update(down_set(&[t, ctrl])).is_empty(),
            "少一个修饰不触发"
        );
        assert_eq!(
            tracker.update(down_set(&[t, ctrl, alt])),
            vec![HotkeyEvent::SpeakPressed]
        );
        assert!(
            tracker.update(down_set(&[t])).is_empty(),
            "修饰先松不算放手"
        );
        assert_eq!(
            tracker.update(down_set(&[])),
            vec![HotkeyEvent::SpeakReleased]
        );
    }

    #[test]
    fn modifier_group_is_or_within_group() {
        // Linux 上 Ctrl 有左右两个码：任意一个按下都算 Ctrl 按下。
        let bindings = HotkeyBindings {
            speak: Some(Hotkey {
                ctrl: true,
                alt: false,
                shift: false,
                key: "D".into(),
            }),
            listen: None,
        };
        let codes = resolve_bindings(&bindings, main_code, |hotkey| {
            if hotkey.ctrl {
                vec![vec![0x11, 0x21]] // 左 Ctrl / 右 Ctrl
            } else {
                Vec::new()
            }
        });
        let mut tracker = EdgeTracker::new(codes);
        let d = b'D' as u16;
        assert_eq!(
            tracker.update(down_set(&[d, 0x21])),
            vec![HotkeyEvent::SpeakPressed],
            "右 Ctrl 也该满足 Ctrl"
        );
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
        let mut tracker = tracker(&bindings);
        assert!(tracker.update(down_set(&[0x11])).is_empty());
        assert!(tracker.update(down_set(&[])).is_empty());
    }

    #[test]
    fn rebind_mid_hold_does_not_emit_phantom_release() {
        let mut tracker = tracker(&speak_only("V"));
        let code_v = b'V' as u16;
        tracker.update(down_set(&[code_v]));
        tracker.rebind(resolve_bindings(
            &speak_only("B"),
            main_code,
            modifier_groups,
        ));
        assert!(
            tracker.update(down_set(&[code_v])).is_empty(),
            "rebind 后不应对旧键补发 Released"
        );
        assert!(tracker.update(down_set(&[])).is_empty());
    }

    #[test]
    fn listen_has_no_release_event() {
        let bindings = HotkeyBindings {
            speak: None,
            listen: Some(Hotkey::plain("L")),
        };
        let mut tracker = tracker(&bindings);
        assert_eq!(
            tracker.update(down_set(&[b'L' as u16])),
            vec![HotkeyEvent::ListenPressed]
        );
        assert!(
            tracker.update(down_set(&[])).is_empty(),
            "Listen 没有松开事件"
        );
    }

    #[test]
    fn unknown_key_name_is_skipped() {
        let codes = resolve_bindings(&speak_only("Tab"), main_code, modifier_groups);
        assert!(codes.is_empty(), "解析不出来的键名应当整条跳过");
    }
}
