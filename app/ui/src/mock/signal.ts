/**
 * 假电平发生器：造一段像人说话的包络（爆发 → 尾音 → 静音），
 * 让电平条和门控指示灯在 dev 里有真实的呼吸感，而不是一条匀速假进度。
 */

import type { GateKind, GateState } from "../types";
import type { GateStatus } from "../types.snapshot";

interface Phase {
  kind: "speech" | "silence";
  until: number;
}

export class FakeVoice {
  private phase: Phase = { kind: "silence", until: 0 };
  private lastRms = 0;
  /** 说话段内的音节相位，制造起伏。 */
  private t = 0;

  /** 推进一帧，返回 0..1 的 rms。 */
  step(now: number, dtMs: number): number {
    if (now >= this.phase.until) {
      const speaking = this.phase.kind === "silence";
      this.phase = {
        kind: speaking ? "speech" : "silence",
        until: now + (speaking ? 1400 + Math.random() * 2200 : 700 + Math.random() * 1500),
      };
      this.t = 0;
    }
    this.t += dtMs;

    let target: number;
    if (this.phase.kind === "speech") {
      // 三个不同周期的正弦叠出音节感，再压到 0.02..0.11
      const syl =
        0.55 +
        0.28 * Math.sin(this.t / 95) +
        0.12 * Math.sin(this.t / 41 + 1.3) +
        0.08 * Math.sin(this.t / 210 + 0.4);
      target = 0.022 + Math.max(0, syl) * 0.09 + Math.random() * 0.008;
    } else {
      target = 0.0015 + Math.random() * 0.002;
    }
    // 一阶低通，避免数字乱跳
    const a = this.phase.kind === "speech" ? 0.45 : 0.15;
    this.lastRms = this.lastRms + (target - this.lastRms) * a;
    return Math.min(1, Math.max(0, this.lastRms));
  }

  get speaking(): boolean {
    return this.phase.kind === "speech";
  }
}

const TAIL_MS = 600;

/** 电平门控的状态机（对齐 gate.rs 的 level 预设语义）。 */
export class FakeGate {
  private lastAbove = -Infinity;
  private wasActive = false;

  constructor(private readonly kind: GateKind) {}

  /** 手动门（按住说话）：由外部按键状态直接决定。 */
  manual(now: number, held: boolean): GateStatus {
    void now;
    const ended = this.wasActive && !held;
    this.wasActive = held;
    return {
      kind: "manual",
      state: held ? "manual" : ended ? "released" : "empty",
      rms: 0,
      active: held,
      ended,
    };
  }

  level(now: number, rms: number, threshold: number): GateStatus {
    if (threshold <= 0) {
      this.wasActive = true;
      return { kind: this.kind, state: "always", rms, active: true, ended: false };
    }
    const above = rms >= threshold;
    if (above) this.lastAbove = now;
    const sinceAbove = now - this.lastAbove;
    const inTail = !above && sinceAbove < TAIL_MS;
    const active = above || inTail;

    let state: GateState;
    if (above) state = "speech";
    else if (inTail) state = sinceAbove > TAIL_MS * 0.7 ? "tail_end" : "tail";
    else state = this.wasActive ? "silence" : "waiting";

    const ended = this.wasActive && !active;
    this.wasActive = active;
    return { kind: this.kind, state, rms, active, ended };
  }
}
