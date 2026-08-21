//! 快捷键：修饰键 + 主键的组合，以及它的规范化与显示。
//!
//! 主键沿用旧版的键名 ↔ VK 映射（见 [`crate::catalog::key_vk`]），额外支持
//! Ctrl / Alt / Shift 修饰。热键监听方（Windows 外壳）拿 VK 码去轮询，本模块
//! 只负责数据形状、校验和人看的写法，跟平台无关。

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::catalog::{key_vk, normalize_key};

/// Windows 修饰键的 VK 码。
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12; // Alt
pub const VK_SHIFT: u16 = 0x10;

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

    /// 主键的 VK 码；主键非法时返回 `None`。
    pub fn key_vk(&self) -> Option<u16> {
        key_vk(&self.key)
    }

    /// 需要同时按住的修饰键 VK 列表。
    pub fn modifier_vks(&self) -> Vec<u16> {
        let mut vks = Vec::with_capacity(3);
        if self.ctrl {
            vks.push(VK_CONTROL);
        }
        if self.alt {
            vks.push(VK_MENU);
        }
        if self.shift {
            vks.push(VK_SHIFT);
        }
        vks
    }

    pub fn is_valid(&self) -> bool {
        self.key_vk().is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_key_has_no_modifiers() {
        let hk = Hotkey::plain("V");
        assert!(hk.is_valid());
        assert_eq!(hk.key_vk(), Some(b'V' as u16));
        assert!(hk.modifier_vks().is_empty());
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
        assert_eq!(hk.modifier_vks(), vec![VK_CONTROL, VK_MENU]);
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
}
