/**
 * 热键的显示与编辑，用规格里现成的两件拼出来：
 *   - 修饰键 Ctrl / Alt / Shift → chip（等宽小标签，active 是绿描边+绿字）
 *   - 主键 → 自绘下拉
 * 没有另造键帽样式。
 */

import { KEY_OPTIONS, keyLabel } from "../catalog";
import type { Hotkey } from "../types";
import { Dropdown } from "./controls";

const MODIFIERS = [
  { key: "ctrl", label: "Ctrl" },
  { key: "alt", label: "Alt" },
  { key: "shift", label: "Shift" },
] as const;

/** 只读显示一个组合。用 chip 的静态形态，不可点。 */
export function HotkeyCombo({ hotkey }: { hotkey: Hotkey | null }) {
  if (!hotkey) return <span className="hint">未设置</span>;
  const parts = [
    ...MODIFIERS.filter((m) => hotkey[m.key]).map((m) => m.label),
    keyLabel(hotkey.key),
  ];
  return (
    <span className="row" style={{ gap: 4 }}>
      {parts.map((p, i) => (
        <span key={`${i}-${p}`} className="row" style={{ gap: 4 }}>
          {i > 0 ? <span className="hint">+</span> : null}
          <span className="chip" style={{ cursor: "default" }}>
            {p}
          </span>
        </span>
      ))}
    </span>
  );
}

/** 可编辑：三个修饰键 chip + 主键下拉。撞键时描红提示在调用方给。 */
export function HotkeyEditor({
  id,
  hotkey,
  onChange,
  disabled,
  label,
}: {
  id?: string;
  hotkey: Hotkey;
  onChange: (next: Hotkey) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <span className="row" style={{ gap: 6 }}>
      {MODIFIERS.map((m) => {
        const on = hotkey[m.key];
        return (
          <button
            key={m.key}
            type="button"
            className={on ? "chip selected" : "chip"}
            aria-pressed={on}
            disabled={disabled}
            onClick={() => onChange({ ...hotkey, [m.key]: !on })}
          >
            {m.label}
          </button>
        );
      })}
      <span style={{ width: 104 }}>
        <Dropdown
          id={id}
          label={label ? `${label}的主键` : "主键"}
          value={hotkey.key}
          options={KEY_OPTIONS}
          disabled={disabled}
          onChange={(key) => onChange({ ...hotkey, key })}
        />
      </span>
    </span>
  );
}
