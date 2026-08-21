/** 纯格式化工具，和组件解耦。 */

const NUM = new Intl.NumberFormat("zh-CN");

/** 千分位整数。 */
export function fmtNum(n: number): string {
  return NUM.format(Math.round(n));
}

/** 相对时间，用于「最后更新」。 */
export function fmtAgo(unixSec: number): string {
  if (!unixSec) return "从未";
  const diff = Math.max(0, Math.floor(Date.now() / 1000 - unixSec));
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}
