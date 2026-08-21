/**
 * 全局状态：一份快照 + 事件归约 + 设置补丁的乐观更新。
 *
 * 规则：
 * - 快照是唯一真相，事件只做增量修补；
 * - 改设置时先乐观改本地（输入框不卡手），再把补丁发给后端，
 *   后端返回的归一化结果覆盖回来（会夹取值域，所以本地可能被修正）；
 * - 字幕、电平、通知是易失状态，不进 settings。
 */

import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { getApi } from "./api";
import type { VoxApi } from "./api";
import { DEFAULT_SETTINGS, cloneSettings } from "./defaults";
import { mergePatch } from "./mock/merge";
import type { PipelineName, Settings, Track } from "./types";
import type { GateStatus, Snapshot, SettingsPatch } from "./types.snapshot";
import { STATE_LABEL } from "./pipeline";

export interface Live {
  /** 两条轨的当前字幕文本（已按 done 断句累积）。 */
  subtitles: Record<Track, string>;
  gates: Partial<Record<PipelineName, GateStatus>>;
  /** 服务端回报的「识别成 X 语言」（自动识别下才有），按轨记，显示成小字。 */
  sourceLanguage: Partial<Record<Track, string>>;
}

export interface Store {
  api: VoxApi;
  snapshot: Snapshot | null;
  settings: Settings;
  live: Live;
  /** 正在发的补丁数，>0 时头部显示「保存中」。 */
  saving: number;
  error: string | null;
  patch(patch: SettingsPatch): void;
  dismissNotice(index: number): void;
  reload(): void;
  /** 后端命令回报的 16 声道端点状态，立刻落本地，不等 devices_changed 事件。 */
  applyCableChannelStatus(status: "visible" | "hidden"): void;
}

const EMPTY_LIVE: Live = {
  subtitles: { speak: "", listen: "" },
  gates: {},
  sourceLanguage: {},
};

const StoreContext = createContext<Store | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const api = useMemo(() => getApi(), []);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [settings, setSettings] = useState<Settings>(() => cloneSettings(DEFAULT_SETTINGS));
  const [live, setLive] = useState<Live>(EMPTY_LIVE);
  const [saving, setSaving] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [reloadTick, setReloadTick] = useState(0);
  /** 本地最新设置的镜像，供补丁合并用，避免闭包拿到旧值。 */
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  useEffect(() => {
    let alive = true;
    api
      .snapshot()
      .then((s) => {
        if (!alive) return;
        setSnapshot(s);
        setSettings(s.settings);
        setError(null);
      })
      .catch((e: unknown) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [api, reloadTick]);

  useEffect(() => {
    return api.subscribe((event) => {
      switch (event.kind) {
        case "settings_changed":
          setSettings(event.settings);
          setSnapshot((s) => (s ? { ...s, settings: event.settings } : s));
          break;
        case "pipeline_state":
          setSnapshot((s) =>
            s
              ? {
                  ...s,
                  [event.pipeline]: {
                    ...s[event.pipeline],
                    state: event.state,
                    state_label: STATE_LABEL[event.state],
                    running: event.state !== "idle" && event.state !== "failed",
                  },
                  headphones_advised: advised(s, event),
                }
              : s,
          );
          break;
        case "gate_status":
          setLive((l) => ({ ...l, gates: { ...l.gates, [event.pipeline]: event.status } }));
          setSnapshot((s) =>
            s ? { ...s, [event.pipeline]: { ...s[event.pipeline], gate: event.status } } : s,
          );
          break;
        case "latency_changed":
          setSnapshot((s) =>
            s ? { ...s, [event.pipeline]: { ...s[event.pipeline], latency: event.latency } } : s,
          );
          break;
        case "subtitle_delta":
          setLive((l) => ({
            ...l,
            // 订正时后端发整句 + replace，必须整行替换；否则追加会叠字。
            subtitles: {
              ...l.subtitles,
              [event.track]: event.replace ? event.text : l.subtitles[event.track] + event.text,
            },
          }));
          break;
        case "subtitle_cleared":
          setLive((l) => ({
            ...l,
            subtitles: { ...l.subtitles, [event.track]: "" },
            sourceLanguage: { ...l.sourceLanguage, [event.track]: undefined },
          }));
          break;
        case "source_detected":
          setLive((l) => ({
            ...l,
            sourceLanguage: { ...l.sourceLanguage, [event.track]: event.language },
          }));
          break;
        case "usage_changed":
          setSnapshot((s) => (s ? { ...s, usage: event.usage } : s));
          break;
        case "mic_active":
          setSnapshot((s) => (s ? { ...s, mic_active: event.active } : s));
          break;
        case "devices_changed":
          void api.snapshot().then((s) => setSnapshot((prev) => (prev ? { ...prev, devices: s.devices } : s)));
          break;
        case "notice":
          setSnapshot((s) => (s ? { ...s, notices: [...s.notices, event.notice].slice(-50) } : s));
          break;
      }
    });
  }, [api]);

  const store: Store = {
    api,
    snapshot,
    settings,
    live,
    saving,
    error,
    patch(p) {
      setSettings((cur) => mergePatch(cloneSettings(cur), p));
      setSaving((n) => n + 1);
      api
        .updateSettings(p)
        .then((next) => {
          setSettings(next);
          setError(null);
        })
        .catch((e: unknown) => {
          setError(String(e));
          setSettings(cloneSettings(settingsRef.current));
        })
        .finally(() => setSaving((n) => n - 1));
    },
    dismissNotice(index) {
      setSnapshot((s) => (s ? { ...s, notices: s.notices.filter((_, i) => i !== index) } : s));
    },
    reload() {
      setReloadTick((n) => n + 1);
    },
    applyCableChannelStatus(status) {
      setSnapshot((s) =>
        s
          ? {
              ...s,
              devices: { ...s.devices, virtual_cable_16ch_status: status },
            }
          : s,
      );
    },
  };

  return <StoreContext.Provider value={store}>{children}</StoreContext.Provider>;
}

function advised(s: Snapshot, event: { pipeline: PipelineName; state: string }): boolean {
  const running = (p: PipelineName) =>
    p === event.pipeline ? event.state !== "idle" && event.state !== "failed" : s[p].running;
  return running("speak") && running("listen");
}

export function useStore(): Store {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore 必须在 StoreProvider 里用");
  return ctx;
}
