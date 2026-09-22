/**
 * Agent 控制面（S1）：总开关、监听端口、四个授权位、字幕订阅去抖。
 *
 * **位为事实**：这一屏的状态只说观察值（`snapshot.control`，Rust `mcp::Status`）——
 * 后端没在监听就直说没在监听和为什么，**不许**拿 `settings.control.enabled` 自己推
 * "应该在跑"。设置是"要什么"，`control` 那一格是"起了没有"，两者不同时按"正在起停"处理
 * （见 [`controlView`]）。
 *
 * 开关改完**立刻**生效：后端收到 `SettingsChanged` 就按新开关起/停（`app/src-tauri/src/mcp.rs`
 * 的 `ControlPlane`），不用重启应用；这里只负责把设置写回去、再把观察值读回来。
 */

import { useEffect, useRef, useState } from "react";

import {
  CONTROL_PORT_MIN,
  TRANSCRIPT_NOTIFY_MS_RANGE,
} from "../defaults";
import { useT } from "../i18n/context";
import type { TParams } from "../i18n/types";
import { useStore } from "../store";
import type { ControlSettings } from "../types";
import type { Snapshot } from "../types.snapshot";
import { SettingsItem, Toggle } from "../ui/controls";

/** 状态那一格要说的四种话（外加"还不知道"与"正在按新设置起停"）。 */
type ControlView =
  | { kind: "loading" }
  | { kind: "applying" }
  | { kind: "off" }
  | { kind: "running"; port: number | null }
  | { kind: "failed"; error: string | null };

/**
 * 观察值 + 设置 → 该显示哪一句。
 *
 * - 快照还没到 = 还不知道（不闪一句假话）；
 * - 设置与后端**按下去的那一档**不一致 = 正在起停（这两句都可能说错，所以都不说）；
 * - 后端说开关关着 = 没在监听；
 * - 后端说在监听 = 运行中（端口取**实际绑上**的那个：`port` 是 0 时只有它知道）；
 * - 开关开着却没在监听 = 起不来（带上后端给的原因）。
 */
function controlView(snapshot: Snapshot | null, wanted: ControlSettings): ControlView {
  if (!snapshot) return { kind: "loading" };
  const status = snapshot.control;
  if (status.enabled !== wanted.enabled || status.port !== wanted.port) {
    return { kind: "applying" };
  }
  if (!status.enabled) return { kind: "off" };
  if (status.running) return { kind: "running", port: status.bound_port };
  return { kind: "failed", error: status.error };
}

/** 端口归一化：与芯的 `Settings::normalize` 同一条——保留段（<1024）归 0，0 = 系统分配。 */
function normalizePort(value: number): number {
  if (!Number.isFinite(value) || value < CONTROL_PORT_MIN) return 0;
  return Math.min(Math.round(value), 65535);
}

/** 去抖间隔归一化：夹进芯的 `TRANSCRIPT_NOTIFY_MS_RANGE`。 */
function normalizeNotify(value: number): number {
  if (!Number.isFinite(value)) return TRANSCRIPT_NOTIFY_MS_RANGE.min;
  return Math.min(
    TRANSCRIPT_NOTIFY_MS_RANGE.max,
    Math.max(TRANSCRIPT_NOTIFY_MS_RANGE.min, Math.round(value)),
  );
}

/**
 * 数字设置项：**输入期间只改本地草稿，失焦 / 回车才写回设置**。
 *
 * 每敲一个数字就写回的话，中间态会被芯的 `normalize` 夹一遍（`4` → `0`），
 * 光标底下的字当场被改掉，`47123` 这种数根本敲不进去。
 *
 * 提交时把草稿也归一化一遍：否则"本地归一化结果 = 原值"的那一次（例如 0 → 0）
 * 不会触发重渲染，屏幕上会留下一个与设置不一致的数字。
 */
function NumberSetting({
  id,
  label,
  value,
  min,
  max,
  normalize,
  onCommit,
}: {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  normalize: (value: number) => number;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(() => String(value));
  const editing = useRef(false);

  // 外面改了值（另一条路写设置、后端归一化）→ 没在编辑就跟着走。
  useEffect(() => {
    if (!editing.current) setDraft(String(value));
  }, [value]);

  function commit(): void {
    editing.current = false;
    const next = normalize(Number(draft));
    setDraft(String(next));
    onCommit(next);
  }

  return (
    <input
      id={id}
      className="form-input mono"
      type="number"
      min={min}
      max={max}
      value={draft}
      aria-label={label}
      onFocus={() => {
        editing.current = true;
      }}
      onChange={(e) => setDraft(e.currentTarget.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
    />
  );
}

/** 翻译函数（`useT()` 的返回值）。只依赖这一个签名，好让纯函数也能查文案。 */
type Translate = (key: string, params?: TParams) => string;

/** 状态那一句（文案在 `i18n/{zh,en}.ts` 的 `agent.status*`）。 */
function statusText(t: Translate, view: ControlView): string {
  switch (view.kind) {
    case "loading":
      return t("agent.statusLoading");
    case "applying":
      return t("agent.statusApplying");
    case "off":
      return t("agent.statusOff");
    case "running":
      return t("agent.statusRunning", { port: view.port ?? "—" });
    case "failed":
      return t("agent.statusFailed", { error: view.error ?? t("agent.statusUnknown") });
  }
}

export function AgentControlPage() {
  const { snapshot, settings, patch } = useStore();
  const t = useT();
  const control = settings.control;
  const view = controlView(snapshot, control);

  const statusClass =
    view.kind === "running"
      ? "badge badge-running"
      : view.kind === "failed"
        ? "badge badge-danger"
        : "badge badge-neutral";

  /** 授权位：改一位写一位。总开关关着时它们照旧存着，只是不生效（Grants 读 `enabled && allow_*`）。 */
  const allow = (
    key: "allow_microphone" | "allow_system_audio" | "allow_audible_output" | "allow_config_write",
    title: string,
    desc: string,
  ) => (
    <SettingsItem
      title={title}
      desc={desc}
      control={
        <Toggle
          checked={control[key]}
          label={title}
          onChange={(value) => patch({ control: { [key]: value } })}
        />
      }
    />
  );

  return (
    <>
      <div className="panel">
        <div className="panel-top">
          <div className="panel-title">{t("agent.title")}</div>
          {/* 状态说的是**事实**（`snapshot.control`）：没在监听就说没在监听，起不来就带上原因。 */}
          <span className={statusClass} data-control-status={view.kind}>
            <span className={view.kind === "running" ? "status-dot running" : "status-dot"} />
            {statusText(t, view)}
          </span>
        </div>
        <div className="panel-body">
          <SettingsItem
            title={t("agent.enabled")}
            desc={t("agent.enabledDesc")}
            control={
              <Toggle
                checked={control.enabled}
                label={t("agent.enabled")}
                onChange={(enabled) => patch({ control: { enabled } })}
              />
            }
          />
          <SettingsItem
            wide
            htmlFor="agent-port"
            title={t("agent.port")}
            desc={t("agent.portDesc")}
            control={
              <NumberSetting
                id="agent-port"
                label={t("agent.port")}
                value={control.port}
                min={0}
                max={65535}
                normalize={normalizePort}
                onCommit={(port) => patch({ control: { port } })}
              />
            }
          />
          {/* 凭据文件：路径是固定的（与起没起无关），但只有真起了才有人会去读它——
              没起时摆出来只会让人以为那儿有一份能用的凭据。 */}
          {view.kind === "running" && snapshot ? (
            <SettingsItem
              wide
              htmlFor="agent-credentials"
              title={t("agent.credentials")}
              desc={t("agent.credentialsDesc")}
              control={
                <input
                  id="agent-credentials"
                  className="form-input mono"
                  type="text"
                  value={snapshot.control.state_file}
                  aria-label={t("agent.credentials")}
                  readOnly
                />
              }
            />
          ) : null}
        </div>
      </div>

      <div className="settings-group">
        {allow("allow_microphone", t("agent.allowMicrophone"), t("agent.allowMicrophoneDesc"))}
        {allow("allow_system_audio", t("agent.allowSystemAudio"), t("agent.allowSystemAudioDesc"))}
        {allow(
          "allow_audible_output",
          t("agent.allowAudibleOutput"),
          t("agent.allowAudibleOutputDesc"),
        )}
        {allow("allow_config_write", t("agent.allowConfigWrite"), t("agent.allowConfigWriteDesc"))}
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          htmlFor="agent-notify"
          title={t("agent.notify")}
          desc={t("agent.notifyDesc")}
          control={
            <NumberSetting
              id="agent-notify"
              label={t("agent.notify")}
              value={control.transcript_notify_ms}
              min={TRANSCRIPT_NOTIFY_MS_RANGE.min}
              max={TRANSCRIPT_NOTIFY_MS_RANGE.max}
              normalize={normalizeNotify}
              onCommit={(transcript_notify_ms) => patch({ control: { transcript_notify_ms } })}
            />
          }
        />
      </div>
    </>
  );
}
