/**
 * 服务商目录的前端适配层。
 *
 * 模型、语言、音色和 API 元数据只维护在仓库根目录 `catalog/*.json`
 * （多语 label 存为 {zh,en,ja}）。Rust 构建脚本读取同一数据生成常量；
 * 这里直接导入，避免双份清单漂移。
 *
 * 由于 label 依赖当前界面语言，模块顶部不再定死字符串，而是暴露
 * `l10n(label, uiLang)` 与按 uiLang 取 label 的函数；组件在体内调用。
 */

import aliyun from "../../../catalog/aliyun.json";
import gemini from "../../../catalog/gemini.json";
import gpt from "../../../catalog/gpt.json";
import type { ModelProvider } from "./types";
import type { UiLang } from "./i18n/types";

/** catalog JSON 里 label/name/description 的多语对象。键是 zh / en / ja。 */
export interface CatalogLabel {
  zh: string;
  en: string;
  ja: string;
}

/** 按当前界面语言取一个 label；未知语言回退 zh。 */
export function l10n(label: CatalogLabel, uiLang: UiLang): string {
  return uiLang === "en" ? label.en : uiLang === "ja-JP" ? label.ja : label.zh;
}

export interface LabeledOption {
  value: string;
  label: string;
}

export interface ModelInfo {
  name: string;
  label: CatalogLabel;
}

export interface ProviderCapabilities {
  languages?: string;
  voice_selection: boolean;
  voice_clone: boolean;
  source_language: boolean;
  hot_update_language: boolean;
}

export interface ProviderCatalog {
  id: ModelProvider;
  label: CatalogLabel;
  console_url: string;
  api_key_placeholder: string;
  model_name: string;
  model_label: CatalogLabel;
  capabilities: ProviderCapabilities;
}

function toModelProvider(id: string): ModelProvider {
  return id as ModelProvider;
}

const PROVIDERS: ProviderCatalog[] = [
  {
    id: toModelProvider(aliyun.provider.id),
    label: aliyun.provider.label,
    console_url: aliyun.provider.console_url,
    api_key_placeholder: aliyun.api.api_key_placeholder,
    model_name: aliyun.model.id,
    model_label: aliyun.model.label,
    capabilities: aliyun.capabilities,
  },
  {
    id: toModelProvider(gemini.provider.id),
    label: gemini.provider.label,
    console_url: gemini.provider.console_url,
    api_key_placeholder: gemini.api.api_key_placeholder,
    model_name: gemini.model.id,
    model_label: gemini.model.label,
    capabilities: gemini.capabilities,
  },
  {
    id: toModelProvider(gpt.provider.id),
    label: gpt.provider.label,
    console_url: gpt.provider.console_url,
    api_key_placeholder: gpt.api.api_key_placeholder,
    model_name: gpt.model.id,
    model_label: gpt.model.label,
    capabilities: gpt.capabilities,
  },
];

export function providerCatalog(provider: ModelProvider): ProviderCatalog | undefined {
  return PROVIDERS.find((candidate) => candidate.id === provider);
}

export function providerIds(): ModelProvider[] {
  return PROVIDERS.map((provider) => provider.id);
}

/** 语言列表（label 已按界面语言解析）。 */
export function providerOptions(uiLang: UiLang): LabeledOption[] {
  return PROVIDERS.map((row) => ({ value: row.id, label: l10n(row.label, uiLang) }));
}

export const LANGUAGE_CODES: { code: string; label: CatalogLabel; audio_output: boolean }[] =
  aliyun.languages.map((language) => ({
    code: language.code,
    label: language.label,
    audio_output: language.audio_output,
  }));

/** 语言下拉选项（label 已按界面语言解析）。 */
export function languageOptions(uiLang: UiLang): LabeledOption[] {
  return LANGUAGE_CODES.map((language) => ({
    value: language.code,
    label: l10n(language.label, uiLang),
  }));
}

export const AUDIO_OUTPUT_LANGUAGES = new Set(
  aliyun.languages.filter((language) => language.audio_output).map((language) => language.code),
);

export const DEFAULT_TARGET_LANGUAGE = aliyun.defaults.target_language;
/** 听人说话固定翻成中文，界面上写死、不给选。 */
export const LISTEN_TARGET_LANGUAGE = aliyun.defaults.listen_target_language;

/** 对方语言下拉：「自动识别」 + 全部语言。「自动识别」来自前端字典 t()。 */
export function sourceLanguageOptions(
  uiLang: UiLang,
  autoDetect: string,
): LabeledOption[] {
  return [{ value: "", label: autoDetect }, ...languageOptions(uiLang)];
}

export const DEFAULT_MODEL_NAME = aliyun.model.id;
const MODELS: ModelInfo[] = PROVIDERS.map((provider) => ({
  name: provider.model_name,
  label: provider.model_label,
}));

export function defaultModelForProvider(provider: ModelProvider): string {
  return providerCatalog(provider)?.model_name ?? DEFAULT_MODEL_NAME;
}

export function providerApiKeyPlaceholder(provider: ModelProvider): string {
  return providerCatalog(provider)?.api_key_placeholder ?? "";
}

export function providerConsoleUrl(provider: ModelProvider): string {
  return providerCatalog(provider)?.console_url ?? "";
}

export function supportsVoiceSelection(provider: ModelProvider): boolean {
  return providerCatalog(provider)?.capabilities.voice_selection ?? false;
}

export function supportsVoiceClone(provider: ModelProvider): boolean {
  return providerCatalog(provider)?.capabilities.voice_clone ?? false;
}

export function supportsSourceLanguage(provider: ModelProvider): boolean {
  return providerCatalog(provider)?.capabilities.source_language ?? false;
}

export function supportsHotUpdateLanguage(provider: ModelProvider): boolean {
  return providerCatalog(provider)?.capabilities.hot_update_language ?? false;
}

export function providerLabel(provider: ModelProvider, uiLang: UiLang): string {
  const row = providerCatalog(provider);
  return row ? l10n(row.label, uiLang) : provider;
}

export function findModel(name: string): ModelInfo | undefined {
  return MODELS.find((model) => model.name === name);
}

export function modelLabel(name: string, uiLang: UiLang): string {
  const model = findModel(name);
  return model ? l10n(model.label, uiLang) : name;
}

export function languageLabel(code: string, uiLang: UiLang): string {
  const language = LANGUAGE_CODES.find((candidate) => candidate.code === code);
  return language ? l10n(language.label, uiLang) : code;
}

export function supportsAudioOutput(language: string, provider: ModelProvider = "aliyun"): boolean {
  const row = providerCatalog(provider);
  if (row?.capabilities.voice_selection) {
    return AUDIO_OUTPUT_LANGUAGES.has(language);
  }
  return LANGUAGE_CODES.some((candidate) => candidate.code === language);
}

export const VOICE_CATALOG = aliyun.voices;
export const DEFAULT_VOICE = aliyun.defaults.voice;
export const LEGACY_REMOVED_VOICE_IDS = new Set(aliyun.legacy_removed_voice_ids);

// --- 按键 ------------------------------------------------------------------

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("");
const DIGITS = "0123456789".split("");

/** 可选按键：26 字母 + 10 数字 + F1–F12 + 空格 + 两个鼠标侧键。故意不含 Tab。 */
export const KEY_OPTIONS: LabeledOption[] = [
  ...LETTERS.map((key) => ({ value: key, label: key })),
  ...DIGITS.map((key) => ({ value: key, label: key })),
  ...Array.from({ length: 12 }, (_, index) => ({
    value: `F${index + 1}`,
    label: `F${index + 1}`,
  })),
  { value: "Space", label: "空格" },
  { value: "XButton1", label: "鼠标侧键1" },
  { value: "XButton2", label: "鼠标侧键2" },
];

export function keyLabel(key: string): string {
  return KEY_OPTIONS.find((option) => option.value === key)?.label ?? key;
}
