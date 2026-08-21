import { useCallback, useEffect, useState } from "react";

const LIMIT = 10;

function read(key: string): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter((value): value is string => typeof value === "string" && value.length > 0)
      : [];
  } catch {
    return [];
  }
}

/** 在本机保存一小段最近选择历史；密钥和业务数据绝不会写进这里。 */
export function useRecentValues(key: string, current?: string | null) {
  const [values, setValues] = useState<string[]>(() => read(key));

  const remember = useCallback(
    (value: string) => {
      if (!value) return;
      setValues((before) => {
        const next = [value, ...before.filter((item) => item !== value)].slice(0, LIMIT);
        try {
          localStorage.setItem(key, JSON.stringify(next));
        } catch {
          // WebView 禁用本地存储时，只保留本次运行内的排序。
        }
        return next;
      });
    },
    [key],
  );

  useEffect(() => {
    if (current) remember(current);
  }, [current, remember]);

  return [values, remember] as const;
}

/** 最近使用的值排前面，其余选项保持维护表中的稳定顺序。 */
export function recentFirst<T extends { value: string }>(
  options: readonly T[],
  recent: readonly string[],
): T[] {
  const rank = new Map(recent.map((value, index) => [value, index]));
  return options
    .map((option, index) => ({ option, index }))
    .sort((left, right) => {
      const leftRank = rank.get(left.option.value) ?? Number.MAX_SAFE_INTEGER;
      const rightRank = rank.get(right.option.value) ?? Number.MAX_SAFE_INTEGER;
      return leftRank - rightRank || left.index - right.index;
    })
    .map(({ option }) => option);
}
