/** 音色选项来自统一的阿里云维护表。最近使用顺序由首页在调用时传入。 */

import { DEFAULT_VOICE, LEGACY_REMOVED_VOICE_IDS, VOICE_CATALOG } from "./catalog";
import type { LabeledOption } from "./catalog";

const LABELS = new Map(
  VOICE_CATALOG.map((voice) => [
    voice.id,
    `${voice.name}（${voice.description}）`,
  ]),
);

export function voiceLabel(id: string): string {
  return LABELS.get(id) ?? id;
}

export interface VoiceOption extends LabeledOption {
  /** 官方默认音色。 */
  recommended: boolean;
}

/**
 * 排序顺序：当前值 → 最近用过的值 → 官方默认 → 官方维护表原顺序。
 * 表外值只用于兼容声音复刻生成的自定义音色 ID。
 */
export function orderedVoices(
  current?: string | null,
  recent: readonly string[] = [],
): VoiceOption[] {
  const ids = [current, ...recent, DEFAULT_VOICE, ...VOICE_CATALOG.map((voice) => voice.id)];
  const seen = new Set<string>();
  const out: VoiceOption[] = [];

  for (const id of ids) {
    if (!id || seen.has(id)) continue;
    if (LEGACY_REMOVED_VOICE_IDS.has(id)) continue;
    seen.add(id);
    out.push({
      value: id,
      label: LABELS.has(id) ? voiceLabel(id) : `${id}（自定义）`,
      recommended: id === DEFAULT_VOICE,
    });
  }
  return out;
}

export function defaultVoiceForLanguage(_language: string): string {
  return DEFAULT_VOICE;
}
