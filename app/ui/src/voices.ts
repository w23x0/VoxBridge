/** 音色选项来自统一的维护表（{zh,en,ja}）。最近使用顺序由调用方在传入。
 *  目录里的音色可被在线更新替换，所以全部经 `catalog` 的函数现取，不缓存。 */

import { l10n, type CatalogLabel } from "./catalog";
import { legacyRemovedVoiceIds, voiceCatalog, defaultVoiceForProvider } from "./catalog";
import type { LabeledOption } from "./catalog";
import type { UiLang } from "./i18n/types";

/** 音色 id → 当前目录的多语展示名「name（description）」。随目录变化即时取。 */
function labelFor(id: string): CatalogLabel | undefined {
  const voice = voiceCatalog().find((candidate) => candidate.id === id);
  if (!voice) return undefined;
  return {
    zh: `${voice.name.zh}（${voice.description.zh}）`,
    en: `${voice.name.en}（${voice.description.en}）`,
    ja: `${voice.name.ja}（${voice.description.ja}）`,
  };
}

/** 音色 id → 当前语言展示名；未知 id 返回原 id。 */
export function voiceLabel(id: string, uiLang: UiLang): string {
  const label = labelFor(id);
  return label ? l10n(label, uiLang) : id;
}

export interface VoiceOption extends LabeledOption {
  /** 官方默认音色。 */
  recommended: boolean;
}

/**
 * 排序顺序：当前值 → 最近用过的值 → 官方默认 → 维护表原顺序。
 * 表外值只用于兼容声音复刻生成的自定义音色 ID，展示名给原 id +
 * `customSuffix`（调用方用 t("catalog.customVoiceSuffix") 传入，已本地化）。
 */
export function orderedVoices(
  uiLang: UiLang,
  current: string | null | undefined,
  recent: readonly string[],
  customSuffix: string,
): VoiceOption[] {
  const catalogVoices = voiceCatalog();
  const removed = legacyRemovedVoiceIds();
  const recommendedVoice = defaultVoiceForProvider();
  const ids = [current, ...recent, recommendedVoice, ...catalogVoices.map((voice) => voice.id)];
  const seen = new Set<string>();
  const out: VoiceOption[] = [];

  for (const id of ids) {
    if (!id || seen.has(id)) continue;
    if (removed.has(id)) continue;
    seen.add(id);
    const label = voiceLabel(id, uiLang);
    out.push({
      value: id,
      // 已知音色用展示名，未知（复刻等）给原 id + customSuffix。
      label: label === id ? `${id}${customSuffix}` : label,
      recommended: id === recommendedVoice,
    });
  }
  return out;
}

/**
 * 音色下拉选项：`orderedVoices` 排好序后，给官方默认音色追加 `defaultVoiceSuffix` 标记。
 * speak/listen 两张卡的下拉都走这里，避免两段逐字相同的 map。
 *
 * `customSuffix` 是复刻音色这类目录外 id 的后缀（调用方用 t("catalog.customVoiceSuffix") 传入）；
 * `defaultVoiceSuffix` 是官方默认音色的后缀（调用方用 t("catalog.defaultVoiceSuffix") 传入）。
 */
export function voiceOptions(
  uiLang: UiLang,
  voice: string | null | undefined,
  recent: readonly string[],
  customSuffix: string,
  defaultVoiceSuffix: string,
): LabeledOption[] {
  return orderedVoices(uiLang, voice, recent, customSuffix).map((option) => ({
    value: option.value,
    label: option.recommended ? `${option.label}${defaultVoiceSuffix}` : option.label,
  }));
}

export function defaultVoiceForLanguage(_language: string): string {
  return defaultVoiceForProvider();
}