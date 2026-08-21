/**
 * 假后端。没有 Rust 侧的时候整套界面照样能点通：
 * 电平条会动、字幕会流进来、token 会涨、通知会冒出来。
 *
 * 行为刻意抄 runtime.rs 的规则：没密钥不许启动、听人说话必须先选程序、
 * 运行中换模型只提示重启、切激活方式会清掉 mic_active。
 */

import { DEFAULT_SETTINGS, cloneSettings } from "../defaults";
import type { PipelineName, PipelineState, Settings, Track } from "../types";
import * as catalog from "../catalog";
import type {
  AudioApp,
  GateStatus,
  LatencyMetric,
  LatencySnapshot,
  Notice,
  PipelineSnapshot,
  Snapshot,
  SettingsPatch,
  UsageLedger,
  VoxEvent,
} from "../types.snapshot";
import type { VoxApi } from "../api";
import { STATE_LABEL, isRunning } from "../pipeline";
import { LISTEN_SCRIPT, MOCK_APPS, MOCK_INPUTS, MOCK_OUTPUTS, SPEAK_SCRIPT, mockUsage } from "./data";
import { mergePatch, normalizeSettings } from "./merge";
import { FakeGate, FakeVoice } from "./signal";
import { FakeTyper } from "./typer";

const TICK_MS = 60;

interface Lane {
  state: PipelineState;
  gate: GateStatus | null;
  voice: FakeVoice;
  gateImpl: FakeGate;
  typer: FakeTyper | null;
}

export function createMockApi(): VoxApi {
  let settings = normalizeSettings(cloneSettings(DEFAULT_SETTINGS));
  const apiKeys = Object.fromEntries(
    catalog.providerIds().map((provider) => [provider, false]),
  ) as Record<string, boolean>;
  let virtualCableInstalled = true;
  let virtualCableStatus:
    | "installed"
    | "install_pending_reboot"
    | "uninstall_incomplete"
    | "not_installed" = "installed";
  let virtualCable16ChStatus: "visible" | "hidden" | "absent" = "hidden";
  let cableBlockers = [MOCK_APPS[1]].filter((app): app is AudioApp => app !== undefined);
  let micActive = false;
  let usage: UsageLedger = mockUsage();
  let notices: Notice[] = [];
  const handlers = new Set<(e: VoxEvent) => void>();
  let timer: ReturnType<typeof setInterval> | null = null;
  let lastTick = performance.now();
  /** 假的「按住说话」相位：hold 模式下自动一按一放，好让人看到门控切换。 */
  let holdUntil = 0;
  let holdNextAt = 0;

  const lanes: Record<PipelineName, Lane> = {
    speak: mkLane(SPEAK_SCRIPT, "speak"),
    listen: mkLane(LISTEN_SCRIPT, "listen"),
  };

  function mkLane(script: string[], track: Track): Lane {
    return {
      state: "idle",
      gate: null,
      voice: new FakeVoice(),
      gateImpl: new FakeGate(track === "speak" ? "level" : "level"),
      typer: new FakeTyper(track, script),
    };
  }

  function emit(event: VoxEvent): void {
    for (const h of [...handlers]) h(event);
  }

  function notify(severity: Notice["severity"], text: string, pipeline: PipelineName | null = null): void {
    const notice: Notice = { severity, text, pipeline };
    notices = [...notices, notice].slice(-50);
    emit({ kind: "notice", notice });
  }

  function setState(pipeline: PipelineName, state: PipelineState): void {
    const lane = lanes[pipeline];
    if (lane.state === state) return;
    lane.state = state;
    if (!isRunning(state)) lane.gate = null;
    emit({ kind: "pipeline_state", pipeline, state });
  }

  function snap(pipeline: PipelineName): PipelineSnapshot {
    const lane = lanes[pipeline];
    return {
      state: lane.state,
      state_label: STATE_LABEL[lane.state],
      running: isRunning(lane.state),
      gate: lane.gate,
      latency: latency(pipeline),
    };
  }

  function latency(pipeline: PipelineName): LatencySnapshot {
    const lane = lanes[pipeline];
    const running = isRunning(lane.state);
    // 演示数据：跟随时钟轻微抖动，让数字看起来是活的。真实值来自 Rust 侧的
    // LatencySnapshot，这里只是让假后端把界面喂饱。
    const jitter = (base: number, span: number) => running ? Math.max(0, base + Math.sin(performance.now() / 400 + (pipeline === "speak" ? 0 : 2)) * span) : 0;
    const metric = (base: number): LatencyMetric => {
      const last = jitter(base, base * 0.2);
      return {
        last_ms: running ? Math.round(last) : null,
        p50_ms: running ? Math.round(base + base * 0.06) : null,
        p95_ms: running ? Math.round(base * 1.35) : null,
        samples: running ? 37 : 0,
      };
    };
    const queued = Math.max(0, Math.round(jitter(8, 4)));
    return {
      connect_ms: 312,
      session_ready_ms: 425,
      input_queue: metric(12),
      upload_send: metric(18),
      server_vad: metric(64),
      first_text: metric(380),
      first_audio: metric(520),
      first_playback: metric(545),
      turn_complete: metric(880),
      completed_turns: 4,
      input_queue_depth: queued,
      input_queue_oldest_ms: Math.round(queued * 20),
      playback_queue_ms: Math.max(0, Math.round(jitter(60, 20))),
      processed_chunks: 1186,
      dropped_chunks: 0,
    };
  }

  function bumpUsage(model: string, chars: number): void {
    const inTok = Math.max(1, Math.round(chars * 2.4));
    const outTok = Math.max(1, Math.round(chars * 1.6));
    const prev = usage[model];
    const base = prev ?? {
      input_tokens: 0, output_tokens: 0, total_tokens: 0, turns: 0,
      daily: { input_tokens: 0, output_tokens: 0, total_tokens: 0, turns: 0 },
      daily_date: new Date().toISOString().slice(0, 10),
      monthly: { input_tokens: 0, output_tokens: 0, total_tokens: 0, turns: 0 },
      monthly_month: new Date().toISOString().slice(0, 7),
      updated_at: 0,
    };
    const add = (t: typeof base.daily) => ({
      input_tokens: t.input_tokens + inTok,
      output_tokens: t.output_tokens + outTok,
      total_tokens: t.total_tokens + inTok + outTok,
      turns: t.turns,
    });
    usage = {
      ...usage,
      [model]: {
        ...add(base),
        turns: base.turns,
        daily: add(base.daily),
        monthly: add(base.monthly),
        daily_date: base.daily_date,
        monthly_month: base.monthly_month,
        updated_at: Math.floor(Date.now() / 1000),
      },
    };
    emit({ kind: "usage_changed", usage });
  }

  function tick(): void {
    const now = performance.now();
    const dt = now - lastTick;
    lastTick = now;

    // --- 对外说话：按激活方式驱动门控 ---
    const speak = lanes.speak;
    if (isRunning(speak.state)) {
      if (settings.speak.activation_mode === "hold") {
        if (now > holdNextAt) {
          holdUntil = now + 1800 + Math.random() * 1500;
          holdNextAt = holdUntil + 1200 + Math.random() * 1600;
        }
        const held = now < holdUntil;
        speak.gate = speak.gateImpl.manual(now, held);
        setMic(held);
      } else {
        const rms = speak.voice.step(now, dt);
        speak.gate = speak.gateImpl.level(now, rms, settings.speak.gate_threshold);
        setMic(speak.gate.active);
      }
      emit({ kind: "gate_status", pipeline: "speak", status: speak.gate });
      if (speak.gate.active && speak.typer) {
        const n = speak.typer.step(now, emit);
        if (n > 0 && Math.random() < 0.35) bumpUsage(settings.speak.model_name, n);
      }
    } else if (micActive) {
      setMic(false);
    }

    // --- 听人说话：门是常开的（真实后端阈值 0，无条件放行），电平条照样动 ---
    const listen = lanes.listen;
    if (isRunning(listen.state)) {
      const rms = listen.voice.step(now, dt);
      listen.gate = listen.gateImpl.level(now, rms, 0);
      emit({ kind: "gate_status", pipeline: "listen", status: listen.gate });
      if (listen.gate.active && listen.typer) {
        const n = listen.typer.step(now, emit);
        if (n > 0 && Math.random() < 0.35) bumpUsage(settings.listen.model_name, n);
      }
    }

    // 延迟统计沿每拍推一次（真实后端约 500 ms 节流；这里只是喂活面板）。
    if (isRunning(speak.state)) {
      emit({ kind: "latency_changed", pipeline: "speak", latency: latency("speak") });
    }
    if (isRunning(listen.state)) {
      emit({ kind: "latency_changed", pipeline: "listen", latency: latency("listen") });
    }
  }

  function setMic(active: boolean): void {
    if (micActive === active) return;
    micActive = active;
    emit({ kind: "mic_active", active });
  }

  function ensureTimer(): void {
    const anyLive = (["speak", "listen"] as const).some((p) => isRunning(lanes[p].state));
    if (anyLive && timer === null) {
      lastTick = performance.now();
      timer = setInterval(tick, TICK_MS);
    } else if (!anyLive && timer !== null) {
      clearInterval(timer);
      timer = null;
      setMic(false);
    }
  }

  async function start(pipeline: PipelineName): Promise<void> {
    const provider = settings[pipeline].provider;
    if (!apiKeys[provider]) {
      notify("warning", "请先配置 API 密钥", pipeline);
      return;
    }
    if (pipeline === "listen" && !settings.listen.target) {
      notify("warning", "请先选择监听程序", pipeline);
      return;
    }
    if (isRunning(lanes[pipeline].state)) return;
    setState(pipeline, "starting");
    ensureTimer();
    await wait(420);
    setState(pipeline, "ready");
    await wait(260);
    setState(pipeline, "active");
  }

  async function stop(pipeline: PipelineName): Promise<void> {
    if (!isRunning(lanes[pipeline].state)) return;
    lanes[pipeline].typer?.reset(emit);
    setState(pipeline, "idle");
    ensureTimer();
  }

  function currentSnapshot(): Snapshot {
    return {
      settings: cloneSettings(settings),
      has_api_key: catalog.providerIds().some((provider) => apiKeys[provider]),
      api_keys: { ...apiKeys },
      speak: snap("speak"),
      listen: snap("listen"),
      mic_active: micActive,
      headphones_advised: isRunning(lanes.speak.state) && isRunning(lanes.listen.state),
      devices: {
        inputs: MOCK_INPUTS,
        outputs: MOCK_OUTPUTS,
        apps: MOCK_APPS,
        virtual_cable_installed: virtualCableInstalled,
        virtual_cable_status: virtualCableStatus,
        virtual_cable_16ch_status: virtualCable16ChStatus,
      },
      usage,
      notices: [...notices],
    };
  }

  /**
   * 假后端默认起在「已经在用」的状态：有密钥、选好了监听程序、两条常驻管线在跑。
   * 不然一进来什么都不动，看不出电平条和字幕长什么样。
   * 想看空状态（未填密钥、没选程序）加 ?cold=1。
   */
  function seed(): void {
    if (new URLSearchParams(window.location.search).get("cold") === "1") return;
    for (const provider of catalog.providerIds()) {
      apiKeys[provider] = true;
    }
    const app = MOCK_APPS[0];
    if (app) {
      settings = normalizeSettings({
        ...cloneSettings(settings),
        listen: {
          ...settings.listen,
          target: { executable: app.executable, display_name: app.display_name, include_process_tree: true },
        },
      });
    }
    setState("speak", "active");
    setState("listen", "active");
    ensureTimer();
  }

  seed();

  return {
    mock: true,
    async snapshot() {
      await wait(60);
      return currentSnapshot();
    },
    async updateSettings(patch: SettingsPatch) {
      const before = settings;
      const next = normalizeSettings(mergePatch(cloneSettings(settings), patch));
      if (JSON.stringify(before) === JSON.stringify(next)) return cloneSettings(settings);
      settings = next;
      if (
        (before.speak.provider !== next.speak.provider ||
          before.speak.model_name !== next.speak.model_name ||
          (!catalog.supportsHotUpdateLanguage(next.speak.provider) &&
            before.speak.target_language !== next.speak.target_language)) &&
        isRunning(lanes.speak.state)
      ) {
        notify("info", "模型设置已保存，重启「对外说话」后生效", "speak");
      }
      if (
        (before.listen.provider !== next.listen.provider ||
          before.listen.model_name !== next.listen.model_name) &&
        isRunning(lanes.listen.state)
      ) {
        notify("info", "模型设置已保存，重启「听人说话」后生效", "listen");
      }
      if (before.speak.activation_mode !== next.speak.activation_mode) {
        holdUntil = 0;
        holdNextAt = 0;
        setMic(false);
      }
      emit({ kind: "settings_changed", settings: cloneSettings(settings) });
      return cloneSettings(settings);
    },
    async setApiKey(provider, key) {
      apiKeys[provider] = key.trim().length > 0;
      notify("info", apiKeys[provider] ? "密钥已保存" : "密钥已清空");
    },
    startPipeline: start,
    stopPipeline: stop,
    async togglePipeline(pipeline) {
      if (isRunning(lanes[pipeline].state)) await stop(pipeline);
      else await start(pipeline);
    },
    async resetUsage() {
      usage = {};
      emit({ kind: "usage_changed", usage });
    },
    async resetUsageModel(model: string) {
      const next = { ...usage };
      delete next[model];
      usage = next;
      emit({ kind: "usage_changed", usage });
    },
    async refreshDevices() {
      await wait(320);
      emit({ kind: "devices_changed" });
    },
    async installVirtualCable() {
      await wait(900);
      virtualCableInstalled = true;
      virtualCableStatus = "installed";
      virtualCable16ChStatus = "hidden";
      emit({ kind: "devices_changed" });
      return { needs_reboot: false, multichannel_hidden: true };
    },
    async virtualCableBlockers() {
      await wait(250);
      return [...cableBlockers];
    },
    async uninstallVirtualCable(closeBlockers: boolean) {
      if (cableBlockers.length > 0 && !closeBlockers) {
        throw new Error(`以下应用仍在占用虚拟麦克风：${cableBlockers.map((app) => app.display_name).join("、")}`);
      }
      await wait(700);
      cableBlockers = [];
      virtualCableInstalled = false;
      virtualCableStatus = "not_installed";
      virtualCable16ChStatus = "absent";
      emit({ kind: "devices_changed" });
      return { needs_reboot: false, multichannel_hidden: true };
    },
    async setVirtualCableMultichannelVisible(visible: boolean) {
      await wait(500);
      virtualCable16ChStatus = visible ? "visible" : "hidden";
      emit({ kind: "devices_changed" });
      return { needs_reboot: false, multichannel_hidden: !visible };
    },
    async openVirtualCableWebsite() {
      window.open("https://vb-audio.com/Cable/", "_blank", "noopener");
    },
    async openVirtualCableDonation() {
      window.open("https://vb-audio.com/Services/licensing.htm", "_blank", "noopener");
    },
    async openDashscopeConsole() {
      window.open("https://bailian.console.aliyun.com/?apiKey=1", "_blank", "noopener");
    },
    async openProviderConsole(provider) {
      window.open(catalog.providerConsoleUrl(provider), "_blank", "noopener");
    },
    subscribe(handler) {
      handlers.add(handler);
      return () => {
        handlers.delete(handler);
      };
    },
  };
}

function wait(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** 让 Settings 类型在本文件里被用到（避免 noUnusedLocals 报错）。 */
export type MockSettings = Settings;
