/** 运行时快照 / 用量 / 事件的类型镜像。见 types.ts 顶部说明。 */

import type {
  GateKind,
  GateState,
  PipelineName,
  PipelineState,
  Settings,
  Severity,
  Track,
} from "./types";

export interface GateStatus {
  kind: GateKind;
  state: GateState;
  /** 0..1 左右的均方根电平，level 门控才有意义。 */
  rms: number;
  active: boolean;
  ended: boolean;
}

export interface PipelineSnapshot {
  state: PipelineState;
  /** 后端给的中文态标签，直接显示，不要自己再翻一遍。 */
  state_label: string;
  running: boolean;
  gate: GateStatus | null;
  /** 逐轮延迟与队列健康度（后端 `PipelineSnapshotDto` 同形映射）。 */
  latency: LatencySnapshot;
}

export interface DeviceInfo {
  name: string;
  is_default: boolean;
}

export interface AudioApp {
  executable: string;
  display_name: string;
  pid: number;
  active: boolean;
}

export interface DeviceSnapshot {
  inputs: DeviceInfo[];
  outputs: DeviceInfo[];
  apps: AudioApp[];
  /** VB-CABLE 两端都已出现，当前可以作为虚拟麦克风使用。 */
  virtual_cable_installed: boolean;
  virtual_cable_status:
    | "installed"
    | "install_pending_reboot"
    | "uninstall_incomplete"
    | "not_installed";
  virtual_cable_16ch_status: "visible" | "hidden" | "absent";
}

export interface Notice {
  severity: Severity;
  text: string;
  pipeline: PipelineName | null;
}

export interface UsageTotals {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  turns: number;
}

/** ModelUsage：Rust 侧 total 是 #[serde(flatten)]，所以总计字段就摊在顶层。 */
export interface ModelUsage extends UsageTotals {
  daily: UsageTotals;
  /** "YYYY-MM-DD" */
  daily_date: string;
  monthly: UsageTotals;
  /** "YYYY-MM" */
  monthly_month: string;
  /** unix 秒 */
  updated_at: number;
}

/** UsageLedger 是 #[serde(transparent)]，线上就是一个以模型名为键的裸对象。 */
export type UsageLedger = Record<string, ModelUsage>;

/** 单项延迟指标：最近一次 / 中位数 / p95；没采到样时全是 null。 */
export interface LatencyMetric {
  last_ms: number | null;
  p50_ms: number | null;
  p95_ms: number | null;
  samples: number;
}

/** 一条流水线的延迟与队列健康度快照（对应 Rust `LatencySnapshot`）。 */
export interface LatencySnapshot {
  /** TCP/TLS/WebSocket + session.update 发出的冷启动时间。 */
  connect_ms: number | null;
  /** 从开始连接到收到 session.updated 的时间。 */
  session_ready_ms: number | null;
  input_queue: LatencyMetric;
  upload_send: LatencyMetric;
  server_vad: LatencyMetric;
  first_text: LatencyMetric;
  first_audio: LatencyMetric;
  first_playback: LatencyMetric;
  turn_complete: LatencyMetric;
  completed_turns: number;
  input_queue_depth: number;
  input_queue_oldest_ms: number;
  playback_queue_ms: number;
  processed_chunks: number;
  dropped_chunks: number;
}

export interface Snapshot {
  settings: Settings;
  has_api_key: boolean;
  api_keys: Record<"aliyun" | "gemini", boolean>;
  speak: PipelineSnapshot;
  listen: PipelineSnapshot;
  mic_active: boolean;
  headphones_advised: boolean;
  devices: DeviceSnapshot;
  usage: UsageLedger;
  notices: Notice[];
}

export type VoxEvent =
  | { kind: "settings_changed"; settings: Settings }
  | { kind: "pipeline_state"; pipeline: PipelineName; state: PipelineState }
  | { kind: "gate_status"; pipeline: PipelineName; status: GateStatus }
  | {
      kind: "subtitle_delta";
      track: Track;
      text: string;
      done: boolean;
      /** 服务端整句重写了。`true` 时 `text` 是完整当前句，要整行替换而不是追加。 */
      replace: boolean;
    }
  | { kind: "subtitle_cleared"; track: Track }
  | { kind: "source_detected"; track: Track; language: string }
  | { kind: "usage_changed"; usage: UsageLedger }
  | { kind: "mic_active"; active: boolean }
  | { kind: "devices_changed" }
  | { kind: "latency_changed"; pipeline: PipelineName; latency: LatencySnapshot }
  | { kind: "notice"; notice: Notice };

/** update_settings 收的补丁：深度可选，Option 字段允许显式 null 用来清空。 */
export type SettingsPatch = DeepPartialNullable<Settings>;

type DeepPartialNullable<T> = {
  [K in keyof T]?: T[K] extends readonly unknown[]
    ? T[K]
    : T[K] extends Record<string, string>
      ? T[K]
      : T[K] extends object | null
        ? DeepPartialNullable<NonNullable<T[K]>> | (null extends T[K] ? null : never)
        : T[K];
};
