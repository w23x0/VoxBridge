/**
 * 假后端用的补丁合并 + 归一化，行为对齐 Rust 侧 update_settings / Settings::normalize。
 * 真后端上线后这套逻辑在 Rust 里跑，这里只服务 dev 预览。
 */

import {
  BACKGROUND_ALPHA_RANGE,
  CHAR_FADE_RANGE,
  CHAR_TTL_RANGE,
  clamp,
  DIM_ALPHA_RANGE,
  FONT_SIZE_RANGE,
  GATE_THRESHOLD_RANGE,
} from "../defaults";
import * as catalog from "../catalog";
import { DEFAULT_VOICE } from "../catalog";
import type { Settings } from "../types";

/** 整块替换而不是递归合并的键：语义上是「这一份就是全部」。 */
const REPLACE_WHOLESALE = new Set(["voice_by_language", "hotkey", "geometry", "target"]);

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 深合并：显式 null 表示清空，缺席的键保持原值。 */
export function mergePatch<T>(base: T, patch: unknown): T {
  if (!isPlainObject(patch)) return base;
  const out = { ...(base as Record<string, unknown>) };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    const current = out[key];
    if (isPlainObject(value) && isPlainObject(current) && !REPLACE_WHOLESALE.has(key)) {
      out[key] = mergePatch(current, value);
    } else {
      out[key] = value;
    }
  }
  return out as T;
}

const HEX = /^#[0-9a-fA-F]{6}$/;

function normalizeColor(value: string, fallback: string): string {
  return HEX.test(value) ? value.toLowerCase() : fallback;
}

function normalizeDevice(value: string | null | undefined): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed.length === 0 ? null : trimmed;
}

/** 夹取值域、清洗空设备名、保证淡出不超过存活时长。 */
export function normalizeSettings(settings: Settings): Settings {
  const s = structuredClone(settings);

  // 界面语言只认白名单；手改配置、拼错一律回退中文（与 Rust Settings::normalize 一致）。
  if (s.ui_language !== "zh-CN" && s.ui_language !== "en") {
    s.ui_language = "zh-CN";
  }

  s.speak.model_name = catalog.defaultModelForProvider(s.speak.provider);
  s.listen.model_name = catalog.defaultModelForProvider(s.listen.provider);
  if (!catalog.supportsVoiceClone(s.speak.provider)) s.speak.voice_clone_frequency = null;
  if (!catalog.supportsSourceLanguage(s.listen.provider)) s.listen.source_language = null;
  if (!catalog.LANGUAGE_CODES.some((language) => language.code === s.speak.target_language)) {
    s.speak.target_language = catalog.DEFAULT_TARGET_LANGUAGE;
  }
  if (!s.speak.voice || catalog.LEGACY_REMOVED_VOICE_IDS.has(s.speak.voice)) {
    s.speak.voice = DEFAULT_VOICE;
  }
  if (!s.listen.voice || catalog.LEGACY_REMOVED_VOICE_IDS.has(s.listen.voice)) {
    s.listen.voice = DEFAULT_VOICE;
  }

  s.speak.gate_threshold = clamp(
    s.speak.gate_threshold,
    GATE_THRESHOLD_RANGE.min,
    GATE_THRESHOLD_RANGE.max,
  );
  s.speak.input_device = normalizeDevice(s.speak.input_device);
  s.speak.output_device = normalizeDevice(s.speak.output_device);
  s.listen.output_device = normalizeDevice(s.listen.output_device);

  s.subtitle.font_size = Math.round(
    clamp(s.subtitle.font_size, FONT_SIZE_RANGE.min, FONT_SIZE_RANGE.max),
  );
  s.subtitle.background_alpha = Math.round(
    clamp(s.subtitle.background_alpha, BACKGROUND_ALPHA_RANGE.min, BACKGROUND_ALPHA_RANGE.max),
  );
  s.subtitle.char_ttl_ms = Math.round(
    clamp(s.subtitle.char_ttl_ms, CHAR_TTL_RANGE.min, CHAR_TTL_RANGE.max),
  );
  s.subtitle.char_fade_ms = Math.round(
    clamp(s.subtitle.char_fade_ms, CHAR_FADE_RANGE.min, CHAR_FADE_RANGE.max),
  );
  if (s.subtitle.char_fade_ms > s.subtitle.char_ttl_ms) {
    s.subtitle.char_fade_ms = s.subtitle.char_ttl_ms;
  }
  // Rust 侧会把 NaN/Infinity 退回默认值，再夹进合法范围。
  if (!Number.isFinite(s.subtitle.dim_alpha)) {
    s.subtitle.dim_alpha = 0.3;
  }
  s.subtitle.dim_alpha = clamp(
    s.subtitle.dim_alpha,
    DIM_ALPHA_RANGE.min,
    DIM_ALPHA_RANGE.max,
  );
  s.subtitle.speak_color = normalizeColor(s.subtitle.speak_color, "#fff4de");
  s.subtitle.listen_color = normalizeColor(s.subtitle.listen_color, "#eef6ff");
  if (s.subtitle.font_family.trim().length === 0) {
    s.subtitle.font_family = "Microsoft YaHei UI";
  }
  if (s.subtitle.geometry) {
    s.subtitle.geometry.width = Math.max(160, Math.round(s.subtitle.geometry.width));
    s.subtitle.geometry.height = Math.max(60, Math.round(s.subtitle.geometry.height));
  }

  return s;
}
