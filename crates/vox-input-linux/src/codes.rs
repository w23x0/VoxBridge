//! 键名 ↔ evdev 键码（`KEY_*` / `BTN_*`）。
//!
//! 这张表是 `vox-input-win/src/vk.rs` 的 Linux 兄弟：内核只认键名，码值各平台自己管。
//!
//! **不能靠算术推**：evdev 的码值是按物理键盘排布编的，字母不连续
//! （`KEY_A=30` 但 `KEY_Z=44`，中间夹着分号、引号、Shift…），F 键也不连续
//! （`KEY_F10=68`，`KEY_F11=87`）。所以这里是一张写死的表——当初图省事用
//! `KEY_A + offset` 算过一版，被"字母范围连续"那条测试当场抓出来。
//!
//! 另一处跟 Windows 的差别：**修饰键有左右两个码**（`KEY_LEFTCTRL` / `KEY_RIGHTCTRL`），
//! 任一按下都算 Ctrl，所以修饰返回"码组"（组内或、组间与）。

use evdev::KeyCode;
use vox_core::hotkey::Hotkey;

/// 规范键名 → evdev 键码。
pub fn main_code(name: &str) -> Option<u16> {
    let canonical = vox_core::catalog::normalize_key(name)?;
    let code = match canonical.as_str() {
        "A" => KeyCode::KEY_A,
        "B" => KeyCode::KEY_B,
        "C" => KeyCode::KEY_C,
        "D" => KeyCode::KEY_D,
        "E" => KeyCode::KEY_E,
        "F" => KeyCode::KEY_F,
        "G" => KeyCode::KEY_G,
        "H" => KeyCode::KEY_H,
        "I" => KeyCode::KEY_I,
        "J" => KeyCode::KEY_J,
        "K" => KeyCode::KEY_K,
        "L" => KeyCode::KEY_L,
        "M" => KeyCode::KEY_M,
        "N" => KeyCode::KEY_N,
        "O" => KeyCode::KEY_O,
        "P" => KeyCode::KEY_P,
        "Q" => KeyCode::KEY_Q,
        "R" => KeyCode::KEY_R,
        "S" => KeyCode::KEY_S,
        "T" => KeyCode::KEY_T,
        "U" => KeyCode::KEY_U,
        "V" => KeyCode::KEY_V,
        "W" => KeyCode::KEY_W,
        "X" => KeyCode::KEY_X,
        "Y" => KeyCode::KEY_Y,
        "Z" => KeyCode::KEY_Z,
        "1" => KeyCode::KEY_1,
        "2" => KeyCode::KEY_2,
        "3" => KeyCode::KEY_3,
        "4" => KeyCode::KEY_4,
        "5" => KeyCode::KEY_5,
        "6" => KeyCode::KEY_6,
        "7" => KeyCode::KEY_7,
        "8" => KeyCode::KEY_8,
        "9" => KeyCode::KEY_9,
        "0" => KeyCode::KEY_0,
        "F1" => KeyCode::KEY_F1,
        "F2" => KeyCode::KEY_F2,
        "F3" => KeyCode::KEY_F3,
        "F4" => KeyCode::KEY_F4,
        "F5" => KeyCode::KEY_F5,
        "F6" => KeyCode::KEY_F6,
        "F7" => KeyCode::KEY_F7,
        "F8" => KeyCode::KEY_F8,
        "F9" => KeyCode::KEY_F9,
        "F10" => KeyCode::KEY_F10,
        "F11" => KeyCode::KEY_F11,
        "F12" => KeyCode::KEY_F12,
        "Space" => KeyCode::KEY_SPACE,
        // 鼠标侧键：跟 Windows 的 XButton1/2 对应，VRChat 用户常拿它当开关。
        "XButton1" => KeyCode::BTN_SIDE,
        "XButton2" => KeyCode::BTN_EXTRA,
        _ => return None,
    };
    Some(code.0)
}

/// 修饰键分组：**组内是"或"**（左右都算），组间是"与"。
pub fn modifier_groups(hotkey: &Hotkey) -> Vec<Vec<u16>> {
    let mut groups = Vec::with_capacity(3);
    if hotkey.ctrl {
        groups.push(vec![KeyCode::KEY_LEFTCTRL.0, KeyCode::KEY_RIGHTCTRL.0]);
    }
    if hotkey.alt {
        groups.push(vec![KeyCode::KEY_LEFTALT.0, KeyCode::KEY_RIGHTALT.0]);
    }
    if hotkey.shift {
        groups.push(vec![KeyCode::KEY_LEFTSHIFT.0, KeyCode::KEY_RIGHTSHIFT.0]);
    }
    groups
}

/// 这个设备像不像键盘（有字母键）。
pub fn looks_like_keyboard(keys: &evdev::AttributeSetRef<KeyCode>) -> bool {
    keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z)
}

/// 这个设备像不像鼠标（有侧键）。
pub fn looks_like_mouse(keys: &evdev::AttributeSetRef<KeyCode>) -> bool {
    keys.contains(KeyCode::BTN_SIDE) || keys.contains(KeyCode::BTN_EXTRA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// UI 会列出来的键名**全部**都要能解析成码：漏一个，用户选了那个键就是热键失灵。
    #[test]
    fn every_ui_key_name_resolves_to_a_code() {
        for option in vox_core::catalog::key_options() {
            assert!(
                main_code(&option.id).is_some(),
                "UI 提供的键名 {} 在 Linux 上没有对应码",
                option.id
            );
        }
    }

    #[test]
    fn codes_are_distinct() {
        let mut seen = HashSet::new();
        for option in vox_core::catalog::key_options() {
            let code = main_code(&option.id).expect("上面那条已保证能解析");
            assert!(seen.insert(code), "{} 的码跟别的键撞了：{code}", option.id);
        }
    }

    #[test]
    fn letters_and_digits_use_evdev_codes_not_ascii() {
        assert_eq!(main_code("A"), Some(KeyCode::KEY_A.0));
        assert_ne!(main_code("A"), Some(b'A' as u16), "evdev 的 KEY_A 不是 65");
        assert_eq!(main_code("Z"), Some(KeyCode::KEY_Z.0));
        assert_ne!(
            main_code("Z").unwrap() - main_code("A").unwrap(),
            25,
            "字母在 evdev 里不连续，不能靠算术推"
        );
        assert_eq!(main_code("1"), Some(KeyCode::KEY_1.0));
        assert_eq!(main_code("0"), Some(KeyCode::KEY_0.0));
    }

    #[test]
    fn function_keys_are_not_contiguous_either() {
        assert_eq!(main_code("F1"), Some(KeyCode::KEY_F1.0));
        assert_eq!(main_code("F10"), Some(KeyCode::KEY_F10.0));
        assert_eq!(main_code("F11"), Some(KeyCode::KEY_F11.0));
        assert_eq!(main_code("F12"), Some(KeyCode::KEY_F12.0));
        assert_ne!(
            main_code("F12").unwrap() - main_code("F1").unwrap(),
            11,
            "F11/F12 跟前面隔着一大段，不能靠算术推"
        );
        assert_eq!(main_code("f8"), Some(KeyCode::KEY_F8.0));
    }

    #[test]
    fn specials_and_mouse_side_buttons() {
        assert_eq!(main_code("space"), Some(KeyCode::KEY_SPACE.0));
        assert_eq!(main_code("XButton1"), Some(KeyCode::BTN_SIDE.0));
        assert_eq!(main_code("xbutton2"), Some(KeyCode::BTN_EXTRA.0));
    }

    #[test]
    fn rejects_what_the_kernel_rejects() {
        assert_eq!(main_code("Tab"), None, "故意不支持 Tab");
        assert_eq!(main_code("F13"), None);
        assert_eq!(main_code(""), None);
    }

    #[test]
    fn modifier_groups_cover_both_sides() {
        let hotkey = Hotkey {
            ctrl: true,
            alt: false,
            shift: true,
            key: "T".into(),
        };
        let groups = modifier_groups(&hotkey);
        assert_eq!(groups.len(), 2, "Ctrl 与 Shift 两组");
        assert_eq!(
            groups[0],
            vec![KeyCode::KEY_LEFTCTRL.0, KeyCode::KEY_RIGHTCTRL.0]
        );
        assert_eq!(
            groups[1],
            vec![KeyCode::KEY_LEFTSHIFT.0, KeyCode::KEY_RIGHTSHIFT.0]
        );
    }
}
