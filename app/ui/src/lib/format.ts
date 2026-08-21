/** 纯格式化工具，和组件解耦。 */

import type { TParams } from "../i18n/types";

const NUM_CN = new Intl.NumberFormat("zh-CN");
const NUM_EN = new Intl.NumberFormat("en-US");

/** 千分位整数。locale 决定分组字符（中文同英文逗号，这里仅按界面语言切换）。 */
export function fmtNum(n: number, lang: "zh-CN" | "en" = "zh-CN"): string {
  return (lang === "en" ? NUM_EN : NUM_CN).format(Math.round(n));
}

/**
 * 相对时间，用于「最后更新」。受语言影响，`t` 提供相对时间文案。
 * `format` 的 key 写在 dict 的 `format.*`，未启用纯返回原文。
 */
export function fmtAgo(
  unixSec: number,
  t: (key: string, params?: TParams) => string,
): string {
  if (!unixSec) return t("format.never");
  const diff = Math.max(0, Math.floor(Date.now() / 1000 - unixSec));
  if (diff < 60) return t("format.justNow");
  if (diff < 3600) return t("format.minutesAgo", { n: Math.floor(diff / 60) });
  if (diff < 86400) return t("format.hoursAgo", { n: Math.floor(diff / 3600) });
  return t("format.daysAgo", { n: Math.floor(diff / 86400) });
}