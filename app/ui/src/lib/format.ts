/** 纯格式化工具，和组件解耦。 */

import type { TParams } from "../i18n/types";

const NUM_CN = new Intl.NumberFormat("zh-CN");

/** 千分位整数。 */
export function fmtNum(n: number): string {
  return NUM_CN.format(Math.round(n));
}

const pad2 = (n: number): string => String(n).padStart(2, "0");

/** 日期键，格式对齐后端 usage.rs 里 Stamp::date_key 的 `YYYY-MM-DD`。 */
export function dateKey(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** 月份键 `YYYY-MM`（同上，usage.rs 的 month_key）。 */
export function monthKey(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}`;
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
