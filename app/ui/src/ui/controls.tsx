/**
 * 规格第 4 节里那几个控件的 React 实现。样式全在 styles/spec.css，
 * 这里只管行为和无障碍属性，一个内联样式值都不新造
 * （唯一例外是自绘滑块的 --slider-pct，那是要按当前值算的）。
 */

import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";

/* ---- 开关 --------------------------------------------------------- */

/**
 * 规格给了两种等价写法，这里用 button + aria-checked：
 * 读屏播报「开关」而非「复选框」，少两层 DOM。代价是要一行 JS 切属性，
 * 且不能参与原生表单提交 —— 这个界面没有表单提交，无所谓。
 */
export function Toggle({
  checked,
  onChange,
  disabled,
  label,
  id,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  /** 没有可见 label 关联时必须给，否则读屏念不出这是什么开关 */
  label?: string;
  id?: string;
}) {
  return (
    <button
      type="button"
      id={id}
      className="toggle-switch"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

/* ---- 自绘下拉 ----------------------------------------------------- */

export interface Option {
  value: string;
  label: string;
}

/**
 * 不用原生 select（规格 4.2）。
 *
 * 两件容易漏的：
 *  - 点页面空白处要收起：document 上挂 click 关掉，触发器自己的 click 要 stopPropagation。
 *  - 键盘要能用：原生 select 白送的东西，自绘之后得自己接（上下移动、Enter 选中、Esc 收起）。
 */
export function Dropdown({
  value,
  options,
  onChange,
  disabled,
  id,
  label,
  placeholder = "请选择",
}: {
  value: string;
  options: Option[];
  onChange: (value: string) => void;
  disabled?: boolean;
  id?: string;
  label?: string;
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(-1);
  const box = useRef<HTMLDivElement | null>(null);
  const listId = useId();

  const current = options.find((o) => o.value === value);

  // 点空白收起。挂在 document 上，触发器的 click 已经 stopPropagation 了。
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("click", close);
    return () => document.removeEventListener("click", close);
  }, [open]);

  // 展开时把光标放到当前选中项上，键盘从那儿开始走
  useEffect(() => {
    if (open) setCursor(options.findIndex((o) => o.value === value));
  }, [open, options, value]);

  const commit = (v: string) => {
    onChange(v);
    setOpen(false);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (disabled) return;
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(options.length - 1, c + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(0, c - 1));
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const pick = options[cursor];
      if (pick) commit(pick.value);
    }
  };

  return (
    <div className="dropdown" ref={box}>
      <button
        type="button"
        id={id}
        className={open ? "dropdown-selected active" : "dropdown-selected"}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={label}
        onKeyDown={onKey}
        onClick={(e) => {
          // 不让这一下冒到 document 上，否则刚开就被关掉
          e.stopPropagation();
          setOpen((o) => !o);
        }}
      >
        <span className="dropdown-text">{current ? current.label : placeholder}</span>
        {/* 箭头是纯 CSS 三角（skill 4.x），不用图标 */}
        <span className="dropdown-arrow" aria-hidden="true" />
      </button>
      {open ? (
        <div
          className="dropdown-options show"
          role="listbox"
          id={listId}
          aria-label={label}
        >
          {options.map((o, i) => (
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={
                o.value === value
                  ? "dropdown-option selected"
                  : i === cursor
                    ? "dropdown-option cursor"
                    : "dropdown-option"
              }
              onMouseEnter={() => setCursor(i)}
              onClick={() => commit(o.value)}
            >
              {o.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/* ---- 滑块 --------------------------------------------------------- */

/**
 * 规格只给了进度条，没给可拖的滑块，所以用原生 range + spec.css 里那套皮。
 * 原生 range 的键盘、触摸、aria 都是白送的，自绘一遍只会更差。
 */
export function Slider({
  value,
  min,
  max,
  step,
  onChange,
  format,
  disabled,
  id,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  /** 右侧读数。等宽显示，拖动时不跳列。 */
  format: (v: number) => string;
  disabled?: boolean;
  id?: string;
  label?: string;
}) {
  const pct = max > min ? ((value - min) / (max - min)) * 100 : 0;
  return (
    <div className="slider-row">
      <input
        type="range"
        className="slider"
        id={id}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={label}
        aria-valuetext={format(value)}
        style={{ ["--slider-pct" as string]: `${pct}%` }}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span className="slider-readout">{format(value)}</span>
    </div>
  );
}

/* ---- 电平条 ------------------------------------------------------- */

/**
 * 进度条 + 一根门限刻线。数据是后端给的真实 rms，没数据就画空槽写「未运行」，
 * 不做假动画。
 */
export function LevelMeter({
  rms,
  threshold,
  stateText,
  active,
}: {
  rms: number | null;
  /** 门限刻线位置。手动门控没有门限就不传。 */
  threshold?: number;
  stateText: string;
  active?: boolean;
}) {
  // rms 0..0.2 映射到 0..100%，超过 0.2 顶到底
  const pct = (v: number) => Math.min(100, Math.max(0, (v / 0.2) * 100));
  return (
    <div className="row">
      <div
        className="progress-track meter"
        style={{ flex: 1 }}
        role="meter"
        aria-label="音频电平"
        aria-valuemin={0}
        aria-valuemax={0.2}
        aria-valuenow={rms === null ? 0 : Number(rms.toFixed(4))}
        aria-valuetext={rms === null ? stateText : `电平 ${rms.toFixed(3)}，${stateText}`}
      >
        <div
          className="progress-fill"
          style={{ width: `${rms === null ? 0 : pct(rms)}%`, opacity: active ? 1 : 0.55 }}
        />
        {threshold !== undefined && threshold > 0 ? (
          <div className="meter-tick" style={{ left: `${pct(threshold)}%` }} title="门限" />
        ) : null}
      </div>
      <span className="slider-readout">
        {rms === null ? stateText : `${rms.toFixed(3)} · ${stateText}`}
      </span>
    </div>
  );
}

/* ---- 设置行 ------------------------------------------------------- */

/**
 * 规格 4.1 的 settings-item：左边标题+描述，右边控件。
 * 左侧容器 min-width:0 在 CSS 里（.si-text），否则长描述会撑破 flex。
 */
export function SettingsItem({
  title,
  desc,
  control,
  wide,
  htmlFor,
}: {
  title: ReactNode;
  desc?: ReactNode;
  control: ReactNode;
  /** 控件是下拉/输入框这种要占宽的，给它固定宽 */
  wide?: boolean;
  htmlFor?: string;
}) {
  return (
    <div className="settings-item">
      <div className="si-text">
        <div className="si-title">
          {htmlFor ? <label htmlFor={htmlFor}>{title}</label> : title}
        </div>
        {desc ? <div className="si-desc">{desc}</div> : null}
      </div>
      <div className={wide ? "si-control si-control-w" : "si-control"}>{control}</div>
    </div>
  );
}
