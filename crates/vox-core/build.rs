use std::{collections::HashSet, env, fs, path::PathBuf};

use serde::Deserialize;

/// catalog JSON 里 label/name/description 的多语对象。构建期前端只用到展示，
/// 后端仅取 zh 作内部常量（前端自行按 UI 语言从 JSON 取）。
#[derive(Deserialize)]
struct L10n {
    zh: String,
    en: String,
    ja: String,
}

#[derive(Deserialize)]
struct Catalog {
    schema_version: i64,
    verified_at: String,
    expected_counts: ExpectedCounts,
    provider: Provider,
    model: Model,
    api: Api,
    defaults: Defaults,
    legacy_removed_voice_ids: Vec<String>,
    languages: Vec<Language>,
    voices: Vec<Voice>,
}

#[derive(Deserialize)]
struct ExpectedCounts {
    languages: usize,
    audio_output_languages: usize,
    voices: usize,
}

#[derive(Deserialize)]
struct Provider {
    id: String,
    label: L10n,
    console_url: String,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    label: L10n,
    stable_snapshot: String,
}

#[derive(Deserialize)]
struct Api {
    legacy_endpoint: String,
}

#[derive(Deserialize)]
struct Defaults {
    target_language: String,
    listen_target_language: String,
    voice: String,
}

#[derive(Deserialize)]
struct Language {
    code: String,
    label: L10n,
    audio_output: bool,
}

#[derive(Deserialize)]
struct Voice {
    id: String,
    name: L10n,
    description: L10n,
}

#[derive(Deserialize)]
struct GeminiCatalog {
    schema_version: i64,
    verified_at: String,
    provider: Provider,
    model: GeminiModel,
    api: GeminiApi,
}

#[derive(Deserialize)]
struct GeminiModel {
    id: String,
    label: L10n,
}

#[derive(Deserialize)]
struct GeminiApi {
    websocket_endpoint: String,
}

fn lit(value: &str) -> String {
    format!("{value:?}")
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("../../catalog/aliyun.json");
    let gemini_source = manifest.join("../../catalog/gemini.json");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", gemini_source.display());

    let raw = fs::read_to_string(&source).expect("读取 catalog/aliyun.json 失败");
    let catalog: Catalog = serde_json::from_str(&raw).expect("catalog/aliyun.json 格式不合法");
    let gemini_raw = fs::read_to_string(&gemini_source).expect("读取 catalog/gemini.json 失败");
    let gemini: GeminiCatalog =
        serde_json::from_str(&gemini_raw).expect("catalog/gemini.json 格式不合法");

    assert_eq!(
        catalog.provider.id, "aliyun",
        "Aliyun 目录 provider.id 必须是 aliyun"
    );
    assert!(
        catalog.schema_version >= 2,
        "catalog/aliyun.json 的 schema_version 至少应为 2（label 已扩成 {{zh,en,ja}}）"
    );
    assert!(
        gemini.schema_version >= 2,
        "catalog/gemini.json 的 schema_version 至少应为 2"
    );
    for lang in &catalog.languages {
        assert!(!lang.label.zh.is_empty() && !lang.label.en.is_empty() && !lang.label.ja.is_empty(),
            "语言 {} 的多语 label 有空值", lang.code);
    }
    for voice in &catalog.voices {
        assert!(!voice.name.zh.is_empty() && !voice.name.en.is_empty() && !voice.name.ja.is_empty(),
            "音色 {} 的 name 有空值", voice.id);
        assert!(!voice.description.zh.is_empty() && !voice.description.en.is_empty() && !voice.description.ja.is_empty(),
            "音色 {} 的 description 有空值", voice.id);
    }
    assert!(!catalog.provider.label.zh.is_empty(), "provider.label 空值");
    assert!(!catalog.model.label.zh.is_empty(), "model.label 空值");
    assert_eq!(
        gemini.provider.id, "gemini",
        "Gemini 目录 provider.id 必须是 gemini"
    );
    assert_eq!(
        gemini.model.id, "gemini-3.5-live-translate-preview",
        "Gemini Live Translation 模型 ID 发生变化时必须同步协议测试"
    );
    assert!(
        gemini
            .api
            .websocket_endpoint
            .starts_with("wss://generativelanguage.googleapis.com/"),
        "Gemini WebSocket 必须使用官方 generativelanguage 端点"
    );
    assert_eq!(
        catalog.languages.len(),
        catalog.expected_counts.languages,
        "语言表数量与 expected_counts 不一致"
    );
    assert_eq!(
        catalog.languages.iter().filter(|l| l.audio_output).count(),
        catalog.expected_counts.audio_output_languages,
        "语音输出语言数量与 expected_counts 不一致"
    );
    assert_eq!(
        catalog.voices.len(),
        catalog.expected_counts.voices,
        "音色数量与 expected_counts 不一致"
    );
    assert!(
        catalog
            .languages
            .iter()
            .any(|l| l.code == catalog.defaults.target_language),
        "默认目标语言不在语言表"
    );
    assert!(
        catalog
            .languages
            .iter()
            .any(|l| l.code == catalog.defaults.listen_target_language),
        "听取目标语言不在语言表"
    );
    assert!(
        catalog
            .voices
            .iter()
            .any(|v| v.id == catalog.defaults.voice),
        "默认音色不在音色表"
    );

    let mut language_ids = HashSet::new();
    assert!(
        catalog
            .languages
            .iter()
            .all(|l| language_ids.insert(&l.code)),
        "语言代码不应重复"
    );
    let mut voice_ids = HashSet::new();
    assert!(
        catalog.voices.iter().all(|v| voice_ids.insert(&v.id)),
        "音色 ID 不应重复"
    );
    assert!(
        catalog
            .legacy_removed_voice_ids
            .iter()
            .all(|id| !voice_ids.contains(id)),
        "已移除旧音色不能同时出现在官方音色表"
    );

    let mut out = String::new();
    out.push_str(&format!(
        "pub const PROVIDER_ID: &str = {};\npub const PROVIDER_LABEL: &str = {};\npub const PROVIDER_CONSOLE_URL: &str = {};\n",
        lit(&catalog.provider.id),
        lit(&catalog.provider.label.zh),
        lit(&catalog.provider.console_url)
    ));
    out.push_str(&format!(
        "pub const GEMINI_PROVIDER_ID: &str = {};\npub const GEMINI_PROVIDER_LABEL: &str = {};\npub const GEMINI_PROVIDER_CONSOLE_URL: &str = {};\npub const GEMINI_CATALOG_VERIFIED_AT: &str = {};\npub const GEMINI_MODEL_NAME: &str = {};\npub const GEMINI_MODEL_LABEL: &str = {};\npub const GEMINI_API_BASE: &str = {};\n",
        lit(&gemini.provider.id),
        lit(&gemini.provider.label.zh),
        lit(&gemini.provider.console_url),
        lit(&gemini.verified_at),
        lit(&gemini.model.id),
        lit(&gemini.model.label.zh),
        lit(&gemini.api.websocket_endpoint)
    ));
    out.push_str(&format!(
        "pub const CATALOG_VERIFIED_AT: &str = {};\npub const DEFAULT_MODEL_NAME: &str = {};\npub const DEFAULT_MODEL_LABEL: &str = {};\npub const STABLE_MODEL_SNAPSHOT: &str = {};\npub const API_BASE: &str = {};\n",
        lit(&catalog.verified_at),
        lit(&catalog.model.id),
        lit(&catalog.model.label.zh),
        lit(&catalog.model.stable_snapshot),
        lit(&catalog.api.legacy_endpoint)
    ));
    out.push_str(&format!(
        "pub const DEFAULT_TARGET_LANGUAGE: &str = {};\npub const LISTEN_TARGET_LANGUAGE: &str = {};\npub const DEFAULT_VOICE: &str = {};\n",
        lit(&catalog.defaults.target_language),
        lit(&catalog.defaults.listen_target_language),
        lit(&catalog.defaults.voice)
    ));

    out.push_str("pub const MODELS: &[ModelInfo] = &[ModelInfo { name: DEFAULT_MODEL_NAME, label: DEFAULT_MODEL_LABEL }];\n");
    out.push_str("pub const LANGUAGE_LABELS: &[(&str, &str)] = &[\n");
    for language in &catalog.languages {
        out.push_str(&format!(
            "    ({}, {}),\n",
            lit(&language.code),
            lit(&language.label.zh)
        ));
    }
    out.push_str("];\n");

    out.push_str("pub const AUDIO_OUTPUT_LANGUAGES: &[&str] = &[\n");
    for language in catalog.languages.iter().filter(|l| l.audio_output) {
        out.push_str(&format!("    {},\n", lit(&language.code)));
    }
    out.push_str("];\n");

    out.push_str("pub const VOICE_LABELS: &[(&str, &str)] = &[\n");
    for voice in &catalog.voices {
        let label = format!("{}（{}）", voice.name.zh, voice.description.zh);
        out.push_str(&format!("    ({}, {}),\n", lit(&voice.id), lit(&label)));
    }
    out.push_str("];\n");

    out.push_str("pub const LEGACY_REMOVED_VOICE_IDS: &[&str] = &[\n");
    for id in &catalog.legacy_removed_voice_ids {
        out.push_str(&format!("    {},\n", lit(id)));
    }
    out.push_str("];\n");

    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("aliyun_catalog.rs");
    fs::write(output, out).expect("写入生成目录失败");
}
