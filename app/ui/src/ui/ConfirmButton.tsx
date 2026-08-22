import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { useT } from "../i18n/context";

/** 两段式确认：点第一下变「确定…」，第二下才真执行，5 秒自动退回。 */
export function ConfirmButton({
  children,
  confirmText,
  onConfirm,
  disabled,
  title,
}: {
  children: ReactNode;
  confirmText: string;
  onConfirm: () => void;
  disabled?: boolean;
  title?: string;
}) {
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const t = useT();
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  if (!armed) {
    return (
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        disabled={disabled}
        title={title}
        data-focus-item
        onClick={() => {
          setArmed(true);
          if (timer.current) clearTimeout(timer.current);
          timer.current = setTimeout(() => setArmed(false), 5000);
        }}
      >
        {children}
      </button>
    );
  }
  const cancelText = t("common.cancel");
  return (
    <span className="row">
      <button
        type="button"
        className="btn btn-danger btn-sm"
        data-focus-item
        onClick={() => {
          setArmed(false);
          onConfirm();
        }}
      >
        {confirmText}
      </button>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        data-focus-item
        onClick={() => setArmed(false)}
      >
        {cancelText}
      </button>
    </span>
  );
}
