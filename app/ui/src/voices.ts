/** 音色选项来自统一的阿里云维护表（{zh,en,ja}）。最近使用顺序由调用方在传入。 */

import { DEFAULT_VOICE, LEGACY_REMOVED_VOICE_IDS, VOICE_CATALOG, l10n, type CatalogLabel } from "./catalog";
import type { LabeledOption } from "./catalog";
import type { UiLang } from "./i18n/types";

/** 官方音色 id -> 多语展示名「name（description）」。 */
const LABELS = new Map<string, CatalogLabel>(
  VOICE_CATALOG.map((voice) => [
    voice.id,
    {
      zh: `${voice.name.zh}（${voice.description.zh}）`,
      en: `${voice.name.en}（${voice.description.en}）`,
      ja: `${voice.name.ja}（${voice.description.ja}）`,
    },
  ]),
);

/** 音色 id -> 当前语言展示名；未知 id 返回原 id。 */
export function voiceLabel(id: string, uiLang: UiLang): string {
  const label = LABELS.get(id);
  return label ? l10n(label, uiLang) : id;
}

export interface VoiceOption extends LabeledOption {
  /** 官方默认音色。 */
  recommended: boolean;
}

/**
 * 排序顺序：当前值 → 最近用过的值 → 官方默认 → 官方维护表原顺序。
 * 表外值只用于兼容声音复刻生成的自定义音色 ID，展示名给原 id +
 * `customSuffix`（调用方用 t("catalog.customVoiceSuffix") 传入，已本地化）。
 */
export function orderedVoices(
  uiLang: UiLang,
  current: string | null | undefined,
  recent: readonly string[],
  customSuffix: string,
): VoiceOption[] {
  const ids = [current, ...recent, DEFAULT_VOICE, ...VOICE_CATALOG.map((voice) => voice.id)];
  const seen = new Set<string>();
  const out: VoiceOption[] = [];

  for (const id of ids) {
    if (!id || seen.has(id)) continue;
    if (LEGACY_REMOVED_VOICE_IDS.has(id)) continue;
    seen.add(id);
    const label = LABELS.has(id)
      ? voiceLabel(id, uiLang)
      : `${id}${customSuffix}`;
    out.push({ value: id, label, recommended: id === DEFAULT_VOICE });
  }
  return out;
}

export function defaultVoiceForLanguage(_language: string): string {
  return DEFAULT_VOICE;
}