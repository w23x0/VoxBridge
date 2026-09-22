//! 静态目录数据：语言、模型、音色、按键。
//!
//! 服务商元数据来自仓库根目录 `catalog/*.json`。构建脚本会校验 Aliyun 的
//! 语言/音色表以及 Gemini 的模型/端点，再生成下面 include 的常量。前端直接
//! 导入同一份 JSON，避免两边各抄一份后悄悄漂移。

use serde::{Deserialize, Serialize};

use crate::capability::{Capability, CapabilitySet};
use crate::settings::ModelProvider;

// --- 模型 ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ModelInfo {
    pub name: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProviderCapabilities {
    pub voice_selection: bool,
    pub voice_clone: bool,
    pub source_language: bool,
    pub hot_update_language: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub console_url: &'static str,
    pub default_model: &'static str,
    pub model_label: &'static str,
    pub capabilities: ProviderCapabilities,
}

include!(concat!(env!("OUT_DIR"), "/aliyun_catalog.rs"));

pub fn normalize_model_for(provider: crate::settings::ModelProvider, name: &str) -> &'static str {
    let info = provider_info(provider);
    if MODELS.iter().any(|model| model.name == info.default_model) {
        normalize_model(name)
    } else {
        info.default_model
    }
}

pub fn provider_info(provider: crate::settings::ModelProvider) -> &'static ProviderInfo {
    PROVIDER_INFOS
        .iter()
        .find(|info| info.id == provider.as_id())
        .unwrap_or_else(|| panic!("catalog is missing provider {}", provider.as_id()))
}

pub fn provider_by_id(id: &str) -> Option<crate::settings::ModelProvider> {
    crate::settings::ModelProvider::ALL
        .into_iter()
        .find(|provider| provider.as_id() == id)
}

/// provider 会什么。名字与能力位表一致（`crate::capability::Capability`）。
///
/// **只有 `catalog/*.json` 的 `capabilities` 里那 4 个布尔有值**；待补的 4 位
/// （`usage_reporting` / `speech_activity` / `turn_end` / `source_transcript`）只占名、恒 `false`
/// ——未拍板的方向不落数据文件，否则等于提前把没定的东西变成契约。
pub fn provider_capabilities(provider: ModelProvider) -> CapabilitySet {
    let caps = provider_info(provider).capabilities;
    let mut set = CapabilitySet::empty();
    for (on, bit) in [
        (caps.voice_selection, Capability::VoiceSelection),
        (caps.voice_clone, Capability::VoiceClone),
        (caps.source_language, Capability::SourceLanguage),
        (caps.hot_update_language, Capability::HotUpdateLanguage),
    ] {
        if on {
            set = set.union(CapabilitySet::single(bit));
        }
    }
    set
}

/// 点名查询：这家 provider 有没有这一位。取代原来的 4 个 `supports_*` 自由函数。
pub fn supports(provider: ModelProvider, bit: Capability) -> bool {
    provider_capabilities(provider).contains(bit)
}

// --- 激活方式 --------------------------------------------------------------

/// 对外说话的激活方式。`Toggle` 按一下开、再按一下关（默认）；
/// `Hold` 按住说话，松开即停。音量阀门只在 `Toggle` 下生效。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivationMode {
    #[default]
    Toggle,
    Hold,
}

impl ActivationMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Toggle => "开关",
            Self::Hold => "按住说话",
        }
    }
}

// --- 查询 ------------------------------------------------------------------

pub fn supports_audio_output(language: &str) -> bool {
    AUDIO_OUTPUT_LANGUAGES.contains(&language)
}

/// 按名字找模型。只有 [`normalize_model`] 用，不对外暴露。
fn find_model(name: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.name == name)
}

/// 把名字收敛成已知模型，不认识就退回默认。
pub fn normalize_model(name: &str) -> &'static str {
    find_model(name).map_or(DEFAULT_MODEL_NAME, |m| m.name)
}

pub fn language_label(code: &str) -> Option<&'static str> {
    LANGUAGE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, l)| *l)
}

pub fn normalize_language(code: &str) -> &'static str {
    LANGUAGE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map_or(DEFAULT_TARGET_LANGUAGE, |(c, _)| *c)
}

/// 音色 id 的显示名。只有 [`ordered_voices`] 用，不对外暴露。
fn voice_label(id: &str) -> Option<&'static str> {
    VOICE_LABELS.iter().find(|(v, _)| *v == id).map(|(_, l)| *l)
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceOption {
    pub id: String,
    pub label: String,
    pub recommended: bool,
}

/// 音色列表。官方默认音色排第一，其余保持维护表顺序。
///
/// `current` 若不在列表里会被钉到最前，保证以前存过的自定义音色不会从菜单里消失。
pub fn ordered_voices(_language: &str, current: Option<&str>) -> Vec<VoiceOption> {
    let mut ids: Vec<&str> = Vec::with_capacity(VOICE_LABELS.len() + 1);
    for (id, _) in VOICE_LABELS {
        ids.push(id);
    }
    if let Some(cur) = current {
        if !cur.is_empty() && !ids.contains(&cur) {
            ids.insert(0, cur);
        }
    }
    ids.into_iter()
        .map(|id| VoiceOption {
            id: id.to_string(),
            label: voice_label(id).unwrap_or(id).to_string(),
            recommended: id == DEFAULT_VOICE,
        })
        .collect()
}

pub fn voice_available(_language: &str, voice: &str) -> bool {
    !voice.is_empty() && !LEGACY_REMOVED_VOICE_IDS.contains(&voice)
}

pub fn default_voice_for_language(_language: &str) -> String {
    DEFAULT_VOICE.to_string()
}

// --- 按键 ------------------------------------------------------------------

/// 合法键名返回规范写法（如 "v" → "V"），非法返回 `None`。
pub fn normalize_key(name: &str) -> Option<String> {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return None;
    }
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(upper);
        }
        return None;
    }
    match upper.as_str() {
        "SPACE" => Some("Space".to_string()),
        "XBUTTON1" => Some("XButton1".to_string()),
        "XBUTTON2" => Some("XButton2".to_string()),
        _ => upper
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=12).contains(n))
            .map(|n| format!("F{n}")),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyOption {
    pub id: String,
    pub label: String,
}

/// 非字母数字键的显示名。UI 的按键枚举（[`key_options`]）和显示当前绑定的
/// [`crate::hotkey::Hotkey::label`] 必须说同一个词，所以这张小表只留这一份。
pub(crate) const SPECIAL_KEY_LABELS: &[(&str, &str)] = &[
    ("Space", "空格"),
    ("XButton1", "鼠标侧键1"),
    ("XButton2", "鼠标侧键2"),
];

/// 供 UI 枚举的按键选项。
pub fn key_options() -> Vec<KeyOption> {
    let mut out = Vec::new();
    let mut push = |id: &str, label: &str| {
        out.push(KeyOption {
            id: id.to_string(),
            label: label.to_string(),
        })
    };
    for c in 'A'..='Z' {
        push(&c.to_string(), &c.to_string());
    }
    for c in '0'..='9' {
        push(&c.to_string(), &c.to_string());
    }
    for n in 1..=12 {
        push(&format!("F{n}"), &format!("F{n}"));
    }
    for (id, label) in SPECIAL_KEY_LABELS {
        push(id, label);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_lookup_is_id_driven_and_covers_third_provider() {
        assert_eq!(
            provider_by_id("gpt"),
            Some(crate::settings::ModelProvider::Gpt)
        );
        for provider in crate::settings::ModelProvider::ALL {
            assert_eq!(
                provider_info(provider).id,
                provider.as_id(),
                "catalog id must match the settings provider id"
            );
            assert_eq!(provider_by_id(provider.as_id()), Some(provider));
        }
        assert_eq!(provider_by_id("missing"), None);
    }

    #[test]
    fn gpt_catalog_points_to_the_realtime_translation_model() {
        let info = provider_info(crate::settings::ModelProvider::Gpt);
        assert_eq!(info.default_model, "gpt-realtime-translate");
        assert_eq!(info.model_label, "GPT Realtime Translate");
        assert_eq!(info.default_model, GPT_MODEL_NAME);
        assert_eq!(info.model_label, GPT_MODEL_LABEL);
        assert_eq!(GPT_INPUT_SAMPLE_RATE, 24_000);
        assert_eq!(GPT_OUTPUT_SAMPLE_RATE, 24_000);
        assert!(!info.capabilities.voice_selection);
        assert!(!info.capabilities.voice_clone);
        assert!(!info.capabilities.source_language);
        assert!(info.capabilities.hot_update_language);
    }

    #[test]
    fn model_normalization_falls_back() {
        assert_eq!(normalize_model("nope"), DEFAULT_MODEL_NAME);
        assert_eq!(normalize_model(DEFAULT_MODEL_NAME), DEFAULT_MODEL_NAME);
        assert_eq!(MODELS.len(), 1, "产品只暴露一个专用实时翻译模型");
    }

    #[test]
    fn language_normalization_falls_back_to_japanese() {
        assert_eq!(normalize_language("xx"), "ja");
        assert_eq!(normalize_language("ko"), "ko");
        assert!(supports_audio_output("ja"));
        assert!(!supports_audio_output("xx"));
    }

    #[test]
    fn default_voice_comes_first_and_custom_is_pinned() {
        let zh = ordered_voices("zh", None);
        assert_eq!(zh[0].id, DEFAULT_VOICE);
        assert!(zh[0].recommended);
        assert_eq!(zh.len(), VOICE_LABELS.len(), "不该有重复项");

        let with_custom = ordered_voices("ja", Some("MyClone"));
        assert_eq!(with_custom[0].id, "MyClone");
        assert!(!with_custom[0].recommended);

        assert_eq!(default_voice_for_language("ja"), DEFAULT_VOICE);
        assert_eq!(default_voice_for_language("th"), DEFAULT_VOICE);
        assert!(voice_available("zh", "Tina"));
        assert!(!voice_available("zh", "Zhixia"), "旧版无效音色要迁走");
        assert!(
            voice_available("zh", "my-cloned-voice"),
            "自定义复刻音色要保留"
        );
        assert!(!voice_available("zh", ""));
    }

    #[test]
    fn key_names_normalize_like_the_old_mapping() {
        // 键名是内核唯一的键标识（键码映射在平台 crate 里），所以这里只验规范化。
        assert_eq!(normalize_key("v").as_deref(), Some("V"));
        assert_eq!(normalize_key("7").as_deref(), Some("7"));
        assert_eq!(normalize_key("F1").as_deref(), Some("F1"));
        assert_eq!(normalize_key("f12").as_deref(), Some("F12"));
        assert_eq!(normalize_key("space").as_deref(), Some("Space"));
        assert_eq!(normalize_key("XButton1").as_deref(), Some("XButton1"));
        assert_eq!(normalize_key("xbutton2").as_deref(), Some("XButton2"));
        assert_eq!(normalize_key("Tab"), None, "故意不支持 Tab");
        assert_eq!(normalize_key("F13"), None);
        assert_eq!(normalize_key(""), None);
        assert_eq!(key_options().len(), 26 + 10 + 12 + 3);
    }
}
