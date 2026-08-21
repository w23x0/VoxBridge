/**
 * Toast（规格 4.4）。右上角纵向堆叠，3 秒倒计时条，滑入滑出。
 *
 * 规格里点名的坑：插入后必须等一帧再加 .show，否则起始 translateX(120%)
 * 和终态落在同一帧里，过渡不触发、toast 直接闪现。
 * 规格给的是命令式 DOM 写法（appendChild + requestAnimationFrame）；
 * React 里等价做法是先渲染成未 show 的状态，再在 rAF 里翻标志位。
 */

import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { IconAlert, IconCheck, IconInfo } from "./icons";

type Tone = "success" | "danger" | "warning";

interface Item {
  id: number;
  tone: Tone;
  text: string;
  show: boolean;
  hiding: boolean;
}

const ToastContext = createContext<((tone: Tone, text: string) => void) | null>(null);

/** 和 CSS 里 toast-countdown 的 3s 对齐；改一处要改两处。 */
const LIFE_MS = 3000;
/** 和 .toast-item 的 transition 0.4s 对齐，等滑出走完再从 DOM 摘掉。 */
const EXIT_MS = 400;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Item[]>([]);
  const seq = useRef(0);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  useEffect(() => {
    return () => {
      for (const t of timers.current) clearTimeout(t);
    };
  }, []);

  const push = useCallback((tone: Tone, text: string) => {
    seq.current += 1;
    const id = seq.current;
    setItems((list) => [...list, { id, tone, text, show: false, hiding: false }]);

    // 等一帧再加 show，否则过渡不触发（规格点名的坑）
    requestAnimationFrame(() => {
      setItems((list) => list.map((it) => (it.id === id ? { ...it, show: true } : it)));
    });

    const hide = setTimeout(() => {
      setItems((list) => list.map((it) => (it.id === id ? { ...it, hiding: true } : it)));
      const drop = setTimeout(() => {
        setItems((list) => list.filter((it) => it.id !== id));
      }, EXIT_MS);
      timers.current.push(drop);
    }, LIFE_MS);
    timers.current.push(hide);
  }, []);

  return (
    <ToastContext.Provider value={push}>
      {children}
      <div id="toast-container" aria-live="polite" aria-atomic="false">
        {items.map((it) => (
          <div
            key={it.id}
            className={`toast-item toast-${it.tone}${it.hiding ? " hide" : it.show ? " show" : ""}`}
            role="status"
          >
            {it.tone === "success" ? (
              <IconCheck size={16} className="toast-icon" />
            ) : it.tone === "danger" ? (
              <IconAlert size={16} className="toast-icon" />
            ) : (
              <IconInfo size={16} className="toast-icon" />
            )}
            <span className="toast-text">{it.text}</span>
            <span className="toast-bar" />
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): (tone: Tone, text: string) => void {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast 必须在 ToastProvider 里用");
  return ctx;
}
