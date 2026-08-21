/**
 * 用量：只显示「已用」。
 *
 * 产品硬规定，和 crates/vox-core/src/usage.rs 一致：
 * 只累加已用 token —— 不做价格表、不换算成钱、不谈剩余额度或余额。
 */

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import * as catalog from "../catalog";
import { useT } from "../i18n/context";
import { useStore } from "../store";
import type { ModelUsage, UsageLedger, UsageTotals } from "../types.snapshot";
import { fmtAgo, fmtNum } from "../lib/format";
import { IconDownload, IconTokens, IconUpload } from "../ui/icons";
import { useToast } from "../ui/toast";

type Range = "today" | "month" | "total";

const ZERO: UsageTotals = { input_tokens: 0, output_tokens: 0, total_tokens: 0, turns: 0 };

const pad2 = (n: number): string => String(n).padStart(2, "0");

/** 日期键，格式对齐后端 usage.rs 里 Stamp::date_key 的 `YYYY-MM-DD`。 */
const dayKey = (now: Date): string =>
  `${now.getFullYear()}-${pad2(now.getMonth() + 1)}-${pad2(now.getDate())}`;
const monthKey = (now: Date): string => `${now.getFullYear()}-${pad2(now.getMonth() + 1)}`;

/**
 * 取某个模型在选定范围下的桶。
 *
 * 后端只在「有新用量记进来」的时候才滚动 daily / monthly 桶，
 * 所以一个今天没用过的模型，它的 daily 桶还挂着上次那天的日期。
 * 这里按日期键对一遍：对不上就当 0，避免把昨天的数字混进「今日」。
 */
function bucket(usage: ModelUsage, range: Range, day: string, month: string): UsageTotals {
  if (range === "today") return usage.daily_date === day ? usage.daily : ZERO;
  if (range === "month") return usage.monthly_month === month ? usage.monthly : ZERO;
  return usage;
}

function sum(rows: UsageTotals[]): UsageTotals {
  return rows.reduce<UsageTotals>(
    (acc, t) => ({
      input_tokens: acc.input_tokens + t.input_tokens,
      output_tokens: acc.output_tokens + t.output_tokens,
      total_tokens: acc.total_tokens + t.total_tokens,
      turns: acc.turns + t.turns,
    }),
    ZERO,
  );
}

/** 统计卡：52×52 渐变图标 + 小标签 + 30px 等宽大数字（规格 4.1）。 */
function StatCard({
  tone,
  icon,
  label,
  value,
}: {
  /** 渐变图标砖的语义色，七档见 tokens.css 的 --grad-*。 */
  tone: "indigo" | "green" | "cyan" | "amber" | "red" | "violet" | "blue";
  icon: ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="stat-card">
      <div className={`stat-icon ${tone}`}>{icon}</div>
      <div style={{ minWidth: 0 }}>
        <div className="stat-label">{label}</div>
        <div className="stat-value">{value}</div>
      </div>
    </div>
  );
}

/** 两段式确认：点第一下变「确定…」，第二下才真执行，5 秒自动退回。 */
function ConfirmButton({
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

export function UsagePage() {
  const { api, snapshot } = useStore();
  const toast = useToast();
  const t = useT();
  const [range, setRange] = useState<Range>("today");

  const now = new Date();
  const day = dayKey(now);
  const month = monthKey(now);

  const ledger: UsageLedger = snapshot?.usage ?? {};
  const models = Object.entries(ledger);
  const totals = sum(models.map(([, u]) => bucket(u, range, day, month)));

  // 快照没到之前不显示数字，免得把「还没读到」误读成「用了 0」
  const loading = snapshot === null;
  const num = (n: number): string => (loading ? "—" : fmtNum(n));

  const resetBlocked = loading
    ? t("usage.loading")
    : models.length === 0
      ? t("usage.empty")
      : null;

  return (
    <>
      <div className="row" style={{ marginBottom: 16 }}>
        <div className="mode-row" role="group" aria-label={t("usage.rangeAria")}>
          {(
            [
              { v: "today", k: "rangeToday" },
              { v: "month", k: "rangeMonth" },
              { v: "total", k: "rangeTotal" },
            ] as const
          ).map((r) => (
            <button
              key={r.v}
              type="button"
              className={range === r.v ? "mode-btn selected" : "mode-btn"}
              aria-pressed={range === r.v}
              data-focus-item
              onClick={() => setRange(r.v)}
            >
              {t(`usage.${r.k}`)}
            </button>
          ))}
        </div>
        <span className="hint mono">
          {loading ? "—" : t("usage.turnsCount", { n: fmtNum(totals.turns) })}
        </span>
      </div>

      <div className="input-grid-3" style={{ marginBottom: 20 }}>
        <StatCard
          tone="blue"
          icon={<IconTokens size={24} />}
          label="Token"
          value={num(totals.total_tokens)}
        />
        <StatCard
          tone="green"
          icon={<IconUpload size={24} />}
          label={t("usage.inputTokens")}
          value={num(totals.input_tokens)}
        />
        <StatCard
          tone="amber"
          icon={<IconDownload size={24} />}
          label={t("usage.outputTokens")}
          value={num(totals.output_tokens)}
        />
      </div>

      <div className="card">
        <div className="sub-card-head">{t("usage.modelList")}</div>
        {loading ? (
          <div className="empty-state">{t("usage.loading")}</div>
        ) : models.length === 0 ? (
          <div className="empty-state">{t("usage.empty")}</div>
        ) : (
          <div className="col">
            {models.map(([name, usage]) => {
              const row = bucket(usage, range, day, month);
              return (
                <div className="sub-card" key={name}>
                  <div className="sub-card-head">
                    <span style={{ color: "var(--text)", fontSize: 13 }}>
                      {catalog.modelLabel(name)}
                    </span>
                    <span className="chip" style={{ cursor: "default" }}>
                      {catalog.findModel(name)
                        ? t("usage.modelChipRealtime")
                        : t("usage.modelChipLegacy")}
                    </span>
                    <span style={{ marginLeft: "auto" }}>
                      <ConfirmButton
                        confirmText={t("usage.confirmClear")}
                        onConfirm={() => {
                          void api.resetUsageModel(name);
                          toast("success", t("usage.cleared", { name: catalog.modelLabel(name) }));
                        }}
                      >
                        {t("usage.clear")}
                      </ConfirmButton>
                    </span>
                  </div>
                  <div className="sub-row">
                    <span>Token</span>
                    <span className="num num-blue">{fmtNum(row.total_tokens)}</span>
                  </div>
                  <div className="sub-row">
                    <span>{t("usage.inputTokens")}</span>
                    <span className="num num-green">{fmtNum(row.input_tokens)}</span>
                  </div>
                  <div className="sub-row">
                    <span>{t("usage.outputTokens")}</span>
                    <span className="num num-amber">{fmtNum(row.output_tokens)}</span>
                  </div>
                  <div className="sub-row">
                    <span>{t("usage.turns")}</span>
                    <span className="num num-muted">{fmtNum(row.turns)}</span>
                  </div>
                  <div className="sub-row">
                    <span>{t("usage.updatedAt")}</span>
                    <span className="num num-muted">{fmtAgo(usage.updated_at, t)}</span>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="settings-group">
        <div className="settings-item">
          <div className="si-text">
            <div className="si-title">{t("usage.resetAll")}</div>
          </div>
          <div className="si-control">
            <ConfirmButton
              confirmText={t("usage.confirmClear")}
              disabled={resetBlocked !== null}
              title={resetBlocked ?? undefined}
              onConfirm={() => {
                void api.resetUsage();
                toast("success", t("usage.allCleared"));
              }}
            >
              {t("usage.clear")}
            </ConfirmButton>
          </div>
        </div>
      </div>
    </>
  );
}
