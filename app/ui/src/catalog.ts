/**
 * 服务商目录的前端适配层。
 *
 * 模型、语言、音色和 API 元数据只维护在仓库根目录 `catalog/*.json`。
 * Rust 构建脚本读取同一数据生成常量；这里直接导入，避免双份清单漂移。
 */

import aliyun from "../../../catalog/aliyun.json";
import gemini from "../../../catalog/gemini.json";
import type { ModelProvider } from "./types";

export interface LabeledOption {
  value: string;
  label: string;
}

export const PROVIDERS: LabeledOption[] = [
  { value: aliyun.provider.id, label: aliyun.provider.label },
  { value: gemini.provider.id, label: gemini.provider.label },
];

export const LANGUAGES: LabeledOption[] = aliyun.languages.map((language) => ({
  value: language.code,
  label: language.label,
}));

export const AUDIO_OUTPUT_LANGUAGES = new Set(
  aliyun.languages.filter((language) => language.audio_output).map((language) => language.code),
);

export const DEFAULT_TARGET_LANGUAGE = aliyun.defaults.target_language;
/** 听人说话固定翻成中文，界面上写死、不给选。 */
export const LISTEN_TARGET_LANGUAGE = aliyun.defaults.listen_target_language;

export const SOURCE_LANGUAGE_OPTIONS: LabeledOption[] = [
  { value: "", label: "自动识别" },
  ...LANGUAGES,
];

export interface ModelInfo {
  name: string;
  label: string;
}

export const DEFAULT_MODEL_NAME = aliyun.model.id;
export const DEFAULT_MODEL_LABEL = aliyun.model.label;
export const GEMINI_MODEL_NAME = gemini.model.id;
export const GEMINI_MODEL_LABEL = gemini.model.label;
export const MODEL_TYPE_LABEL = "实时翻译";
export const MODELS: ModelInfo[] = [
  { name: DEFAULT_MODEL_NAME, label: DEFAULT_MODEL_LABEL },
  { name: GEMINI_MODEL_NAME, label: GEMINI_MODEL_LABEL },
];

export function defaultModelForProvider(provider: ModelProvider): string {
  return provider === "gemini" ? GEMINI_MODEL_NAME : DEFAULT_MODEL_NAME;
}

export function providerLabel(provider: ModelProvider): string {
  return PROVIDERS.find((option) => option.value === provider)?.label ?? provider;
}

export function findModel(name: string): ModelInfo | undefined {
  return MODELS.find((model) => model.name === name);
}

export function modelLabel(name: string): string {
  return findModel(name)?.label ?? `${name}（历史模型）`;
}

export function languageLabel(code: string): string {
  return LANGUAGES.find((language) => language.value === code)?.label ?? code;
}

export function supportsAudioOutput(language: string, provider: ModelProvider = "aliyun"): boolean {
  return provider === "gemini"
    ? LANGUAGES.some((candidate) => candidate.value === language)
    : AUDIO_OUTPUT_LANGUAGES.has(language);
}

export const GEMINI_CATALOG = gemini;

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
