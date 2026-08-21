/**
 * 后端契约的 TypeScript 镜像。
 *
 * 字段名一律照抄 Rust 侧的 serde 序列化结果（snake_case），不许改名、不许加驼峰别名。
 * 对应来源：crates/vox-core/src/{settings,runtime,event,usage,gate,ports,hotkey}.rs
 */

export type PipelineName = "speak" | "listen";

/** Rust: ModelProvider，后续接入服务商时在这里追加。 */
export type ModelProvider = "aliyun" | "gemini";

/** Rust: PipelineState，serde rename_all = "snake_case"。 */
export type PipelineState =
  | "idle"
  | "starting"
  | "ready"
  | "active"
  | "reconnecting"
  | "failed";

/** Rust: Track，serde 小写。 */
export type Track = "speak" | "listen";

/** Rust: ActivationMode，serde 小写。 */
export type ActivationMode = "toggle" | "hold";

/** Rust: Severity，serde 小写。 */
export type Severity = "info" | "warning" | "error";

/** Rust: GateKind，serde 小写。 */
export type GateKind = "manual" | "level";

/** Rust: GateState，serde rename_all = "snake_case"。 */
export type GateState =
  | "empty"
  | "manual"
  | "released"
  | "waiting"
  | "always"
  | "speech"
  | "tail"
  | "tail_end"
  | "silence";

export interface Hotkey {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

export interface ListenTarget {
  executable: string;
  display_name: string;
  include_process_tree: boolean;
}

export interface Geometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SpeakSettings {
  enabled: boolean;
  provider: ModelProvider;
  model_name: string;
  target_language: string;
  voice: string;
  voice_by_language: Record<string, string>;
  voice_clone_frequency: number | null;
  input_device: string | null;
  output_device: string | null;
  show_translation: boolean;
  speak_translation: boolean;
  monitor_translation: boolean;
  activation_mode: ActivationMode;
  hotkey: Hotkey;
  gate_threshold: number;
  denoise: boolean;
}

export interface ListenSettings {
  enabled: boolean;
  provider: ModelProvider;
  model_name: string;
  target: ListenTarget | null;
  output_device: string | null;
  show_translation: boolean;
  speak_translation: boolean;
  voice: string;
  hotkey: Hotkey | null;
  /** 源语言代码；`null` = 服务端自动识别（默认）。 */
  source_language: string | null;
}

export interface SubtitleSettings {
  visible: boolean;
  font_family: string;
  font_size: number;
  speak_color: string;
  listen_color: string;
  background_alpha: number;
  char_ttl_ms: number;
  char_fade_ms: number;
  /** 0 类字（纯噪声/填充词/无意义发音）Lifetime 结束后永久淡到浅灰而非消失。 */
  dim_zeros: boolean;
  /** 0 类字永久淡化的目标 alpha（0..1），越小越淡。 */
  dim_alpha: number;
  geometry: Geometry | null;
}

export interface Settings {
  version: number;
  speak: SpeakSettings;
  listen: ListenSettings;
  subtitle: SubtitleSettings;
  autostart: boolean;
  start_minimized: boolean;
}
