/**
 * 能力位的两个渲染件。
 *
 * - `CapabilityNote`：**位为假时**在原位给一句说明（R1：不静默；R9：说"这台设备做不到"
 *   及 reason）。位为真渲染 `null`——位为真时用户开关照旧（R5）。
 * - `CapabilityPanel`：整机清单（关于页）。位、状态、reason 全摆出来，一行一句。
 *
 * 两个都不判断"这是哪一档宿主"：档位只显示，门开在哪看位。
 */

import type { CSSProperties } from "react";

import { CAPABILITY_KEY, HOST_CAPABILITIES, capabilityNote, hostBit } from "../capabilities";
import { useT } from "../i18n/context";
import { useStore } from "../store";
import type { HostCapability } from "../types.snapshot";

export function CapabilityNote({
  bit,
  style,
}: {
  bit: HostCapability;
  style?: CSSProperties;
}) {
  const { snapshot } = useStore();
  const t = useT();
  // 快照还没到 = 位还不知道，不是"位为假"：这时不渲染，免得闪一句假话。
  if (!snapshot) return null;
  const text = capabilityNote(t, bit, hostBit(snapshot, bit));
  if (!text) return null;
  return (
    <div className="hint hint-warn" style={style} data-capability-note={bit}>
      {text}
    </div>
  );
}

export function CapabilityPanel() {
  const { snapshot } = useStore();
  const t = useT();
  if (!snapshot) return null;
  const caps = snapshot.capabilities;

  return (
    <div className="sub-card" style={{ marginTop: 14 }}>
      <div className="sub-card-head">
        {t("capabilities.title")}
        <span className="num num-muted" style={{ marginLeft: "auto", fontWeight: 400 }}>
          {t(`capabilities.tierName.${caps.tier}`)}
        </span>
      </div>

      <div className="row row-wrap" style={{ gap: 6 }}>
        {HOST_CAPABILITIES.map((bit) => {
          const status = hostBit(snapshot, bit);
          return (
            <span
              key={bit}
              className={status.enabled ? "chip static selected" : "chip static"}
              data-capability={bit}
              data-enabled={status.enabled ? "true" : "false"}
            >
              {t(CAPABILITY_KEY[bit])}
            </span>
          );
        })}
      </div>

      {HOST_CAPABILITIES.map((bit) => {
        const text = capabilityNote(t, bit, hostBit(snapshot, bit));
        return text ? (
          <div
            key={bit}
            className="hint hint-warn"
            style={{ marginTop: 8 }}
            data-capability-note={bit}
          >
            {text}
          </div>
        ) : null;
      })}
    </div>
  );
}
