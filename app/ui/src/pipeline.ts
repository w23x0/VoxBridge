/** 管线状态的展示辅助。标签取值和 Rust 侧 PipelineState::label 一致。
 *
 * ⚠ 注意：`state_label` 是由后端 Rust 发来的中文；前端要 i18n 时，改为在
 * 渲染处用 `t('pipeline.state.' + state)` 取界面语言文案，不要直接显示这个
 * 常量。本常量仍保留供旧路径 / mock 使用。
 */

import type { PipelineState } from "./types";

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

