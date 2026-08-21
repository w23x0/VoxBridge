import { useEffect, useState } from "react";
import * as catalog from "../catalog";
import { useLang, useT } from "../i18n/context";
import { useStore } from "../store";
import type { ModelProvider } from "../types";
import { useToast } from "../ui/toast";

const PROVIDERS: ModelProvider[] = ["aliyun", "gemini", "gpt"];

type RowState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "checked"; current: string; latest: string }
  | { kind: "applying" }
  | { kind: "applied"; last: string };

export function AboutPage() {
  const t = useT();
  const { uiLang } = useLang();
  const { api } = useStore();
  const toast = useToast();
  const [version, setVersion] = useState("0.1.0");
  const [rows, setRows] = useState<Record<string, RowState>>({});

  useEffect(() => {
    void import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then(setVersion)
      .catch(() => undefined);
  }, []);

  // 目录被覆盖后本组件订阅重渲染，让下拉/列表那行的展示名跟上去。
  useEffect(() => {
    return catalog.subscribeCatalog(() => setRows((r) => ({ ...r })));
  }, []);

  const providerName = (provider: ModelProvider) =>
    catalog.providerLabel(provider, uiLang);

  const check = async (provider: ModelProvider) => {
    setRows((r) => ({ ...r, [provider]: { kind: "checking" } }));
    try {
      const res = await api.checkCatalogUpdate(provider);
      setRows((r) => ({
        ...r,
        [provider]: { kind: "checked", current: res.current, latest: res.latest },
      }));
    } catch (error: unknown) {
      toast("danger", t("about.catalogUpdateFailed", { error: String(error) }));
      setRows((r) => ({ ...r, [provider]: { kind: "idle" } }));
    }
  };

  const apply = async (provider: ModelProvider) => {
    setRows((r) => ({ ...r, [provider]: { kind: "applying" } }));
    try {
      const res = await api.applyCatalogUpdate(provider);
      // 把刚落盘的目录灌进内存（不用等重启），并让相关组件重渲染。
      await catalog.reloadCatalog();
      toast("success", t("about.catalogApplied", { at: res.verified }));
      setRows((r) => ({ ...r, [provider]: { kind: "applied", last: res.verified } }));
    } catch (error: unknown) {
      toast("danger", t("about.catalogUpdateFailed", { error: String(error) }));
      setRows((r) => ({ ...r, [provider]: { kind: "idle" } }));
    }
  };

  return (
    <div className="panel">
      <div className="panel-top">
        <div className="panel-title">VoxBridge</div>
        <span className="badge badge-running">v{version}</span>
      </div>
      <div className="panel-body">
        <div className="sub-row">
          <span>{t("about.product")}</span>
          <span className="num num-muted">{t("about.productValue")}</span>
        </div>

        <div className="sub-card" style={{ marginTop: 14 }}>
          <div className="sub-card-head">{t("about.catalogSection")}</div>
          {PROVIDERS.map((provider) => {
            const row = rows[provider] ?? { kind: "idle" as const };
            return (
              <div key={provider} className="row" style={{ gap: 10, padding: "6px 0" }}>
                <span style={{ minWidth: 120 }}>{providerName(provider)}</span>
                <span className="hint mono" style={{ flex: 1, minWidth: 0 }}>
                  {row.kind === "checked"
                    ? row.current === row.latest
                      ? t("about.catalogUpToDate", { at: row.current })
                      : t("about.catalogCurrent", { at: row.current })
                    : row.kind === "applied"
                      ? t("about.catalogApplied", { at: row.last })
                      : row.kind === "checking"
                        ? t("about.catalogChecking")
                        : row.kind === "applying"
                          ? t("about.catalogApplying")
                          : t("about.catalogNoOverride", { at: "builtin" })}
                </span>
                {row.kind === "checked" && row.latest !== row.current ? (
                  <>
                    <span className="badge badge-warn">
                      {t("about.catalogHasUpdate", { at: row.latest })}
                    </span>
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      data-focus-item
                      onClick={() => void apply(provider)}
                    >
                      {t("about.catalogApply")}
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    disabled={row.kind === "checking" || row.kind === "applying"}
                    data-focus-item
                    onClick={() => void check(provider)}
                  >
                    {t("about.checkUpdate")}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}