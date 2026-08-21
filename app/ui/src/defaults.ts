/** 默认值与取值范围。镜像 crates/vox-core/src/settings.rs。 */

import type { Settings } from "./types";
import {
  DEFAULT_MODEL_NAME,
  DEFAULT_TARGET_LANGUAGE,
  DEFAULT_VOICE,
} from "./catalog";

export const SETTINGS_VERSION = 2;

export const GATE_THRESHOLD_RANGE = { min: 0, max: 0.2 } as const;
export const FONT_SIZE_RANGE = { min: 12, max: 96 } as const;
export const CHAR_TTL_RANGE = { min: 500, max: 20000 } as const;
export const CHAR_FADE_RANGE = { min: 0, max: 5000 } as const;
export const BACKGROUND_ALPHA_RANGE = { min: 0, max: 255 } as const;
export const DIM_ALPHA_RANGE = { min: 0.05, max: 1 } as const;

export const DEFAULT_SETTINGS: Settings = {
  version: SETTINGS_VERSION,
  speak: {
    enabled: false,
    provider: "aliyun",
    model_name: DEFAULT_MODEL_NAME,
    target_language: DEFAULT_TARGET_LANGUAGE,
    voice: DEFAULT_VOICE,
    voice_by_language: {},
    voice_clone_frequency: null,
    input_device: null,
    output_device: null,
    show_translation: true,
    speak_translation: true,
    monitor_translation: false,
    activation_mode: "toggle",
    hotkey: { ctrl: false, alt: false, shift: false, key: "V" },
    gate_threshold: 0.012,
    denoise: true,
  },
  listen: {
    enabled: false,
    provider: "aliyun",
    model_name: DEFAULT_MODEL_NAME,
    target: null,
    output_device: null,
    show_translation: true,
    speak_translation: true,
    voice: DEFAULT_VOICE,
    hotkey: null,
    source_language: null,
  },
  subtitle: {
    visible: true,
    font_family: "Microsoft YaHei UI",
    font_size: 30,
    speak_color: "#fff4de",
    listen_color: "#eef6ff",
    background_alpha: 165,
    char_ttl_ms: 2600,
    char_fade_ms: 900,
    dim_zeros: false,
    dim_alpha: 0.3,
    geometry: null,
  },
  autostart: false,
  start_minimized: false,
  ui_language: "zh-CN",
};

export function clamp(value: number, min: number, max: number): number {
  if (Number.isNaN(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/** 深拷贝一份默认设置，避免共享引用被就地改掉。 */
export function cloneSettings(settings: Settings): Settings {
  return structuredClone(settings);
}
