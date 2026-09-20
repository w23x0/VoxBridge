//! 键名 ↔ Windows 虚拟键码（VK）。
//!
//! 这张表原来在内核的 `vox-core::catalog::key_vk` 里——那是内核唯一的平台泄漏
//! （见 `docs/PLATFORM_LINUX.md` §6），现在搬到这里：内核只认键名，码表各平台自己管。
//!
//! A-Z / 0-9 的 VK 码正好等于 ASCII，所以字母数字不用查表。

use vox_core::hotkey::Hotkey;

/// Windows 修饰键的 VK 码。
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12; // Alt
pub const VK_SHIFT: u16 = 0x10;

/// 规范键名 → VK 码。A-Z、0-9、F1-F12、Space、鼠标侧键。
/// 刻意不提供 Tab（跟很多游戏冲突）。
pub fn main_code(name: &str) -> Option<u16> {
    let canonical = vox_core::catalog::normalize_key(name)?;
    let bytes = canonical.as_bytes();
    match canonical.as_str() {
        "Space" => Some(0x20),
        "XButton1" => Some(0x05),
        "XButton2" => Some(0x06),
        _ if canonical.len() == 1 => Some(bytes[0] as u16),
        _ => canonical
            .strip_prefix('F')
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|n| (1..=12).contains(n))
            .map(|n| 0x70 + n - 1),
    }
}

/// 修饰键分组。Windows 每个修饰只有一个 VK，所以每组只有一个码。
pub fn modifier_groups(hotkey: &Hotkey) -> Vec<Vec<u16>> {
    let mut groups = Vec::with_capacity(3);
    if hotkey.ctrl {
        groups.push(vec![VK_CONTROL]);
    }
    if hotkey.alt {
        groups.push(vec![VK_MENU]);
    }
    if hotkey.shift {
        groups.push(vec![VK_SHIFT]);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_the_old_vk_codes() {
        assert_eq!(main_code("v"), Some(b'V' as u16));
        assert_eq!(main_code("7"), Some(b'7' as u16));
        assert_eq!(main_code("F1"), Some(0x70));
        assert_eq!(main_code("f12"), Some(0x7B));
        assert_eq!(main_code("space"), Some(0x20));
        assert_eq!(main_code("XButton1"), Some(0x05));
        assert_eq!(main_code("xbutton2"), Some(0x06));
        assert_eq!(main_code("Tab"), None, "故意不支持 Tab");
        assert_eq!(main_code("F13"), None);
        assert_eq!(main_code(""), None);
    }

    #[test]
    fn modifiers_become_single_code_groups() {
        let hotkey = Hotkey {
            ctrl: true,
            alt: true,
            shift: false,
            key: "T".into(),
        };
        assert_eq!(
            modifier_groups(&hotkey),
            vec![vec![VK_CONTROL], vec![VK_MENU]]
        );
    }
}
