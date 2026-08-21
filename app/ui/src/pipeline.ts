/** 管线状态的展示辅助。标签取值和 Rust 侧 PipelineState::label 一致。
 *
 * ⚠ 注意：`state_label` 是由后端 Rust 发来的中文；前端要 i18n 时，改为在
 * 渲染处用 `t('pipeline.state.' + state)` 取界面语言文案，不要直接显示这个
 * 常量。本常量仍保留供旧路径 / mock 使用。
 */

import type { PipelineName, PipelineState } from "./types";

export const STATE_LABEL: Record<PipelineState, string> = {
  idle: "待机",
  starting: "启动中",
  ready: "已就绪",
  active: "运行中",
  reconnecting: "重连中",
  failed: "错误",
};

/** 只有「待机」和「错误」算没跑。 */
export function isRunning(state: PipelineState): boolean {
  return state !== "idle" && state !== "failed";
}

/** 状态灯的语义色，用来选 CSS 修饰类。 */
export type StateTone = "off" | "pending" | "live" | "bad";

export function stateTone(state: PipelineState): StateTone {
  switch (state) {
    case "idle":
      return "off";
    case "starting":
    case "reconnecting":
      return "pending";
    case "ready":
    case "active":
      return "live";
    case "failed":
      return "bad";
  }
}

export const PIPELINE_LABEL: Record<PipelineName, string> = {
  speak: "对外说话",
  listen: "听人说话",
};
