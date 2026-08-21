/** 模型服务商：按服务商管理密钥与能力目录。 */

import { useEffect, useRef, useState } from "react";

import * as catalog from "../catalog";
import { useStore } from "../store";
import type { ModelProvider } from "../types";
import { Dropdown } from "../ui/controls";
import { IconExternal, IconSave, IconTrash } from "../ui/icons";
import { useToast } from "../ui/toast";
import { orderedVoices } from "../voices";

export function ProvidersPage() {
  const { api, snapshot, reload } = useStore();
  const toast = useToast();
  const [provider, setProvider] = useState<ModelProvider>("gemini");
  const [hasDraft, setHasDraft] = useState(false);
  const [busy, setBusy] = useState<"save" | "clear" | null>(null);
  const [armed, setArmed] = useState(false);
  const input = useRef<HTMLInputElement | null>(null);
  const busyRef = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const loading = snapshot === null;
  const hasKey = snapshot?.api_keys[provider] ?? false;
  const isGemini = provider === "gemini";

  useEffect(() => {
    if (input.current) input.current.value = "";
    setHasDraft(false);
    setArmed(false);
  }, [provider]);

  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const saveKey = async () => {
    if (busyRef.current) return;
    const key = input.current?.value.trim() ?? "";
    if (!key) return;
    busyRef.current = true;
    setBusy("save");
    if (input.current) input.current.value = "";
    setHasDraft(false);
    try {
      await api.setApiKey(provider, key);
      reload();
      toast("success", `${catalog.providerLabel(provider)} 密钥已保存`);
    } catch (error: unknown) {
      toast("danger", `保存失败：${String(error).replaceAll(key, "[密钥已隐藏]")}`);
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const clearKey = async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy("clear");
    setArmed(false);
    try {
      await api.setApiKey(provider, "");
      reload();
      toast("success", `${catalog.providerLabel(provider)} 密钥已清除`);
    } catch (error: unknown) {
      toast("danger", `清除失败：${String(error)}`);
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  const voices = orderedVoices(null);
  const audioLanguages = catalog.LANGUAGES.filter((language) =>
    catalog.supportsAudioOutput(language.value, "aliyun"),
  );
  const textOnlyLanguages = catalog.LANGUAGES.filter(
    (language) => !catalog.supportsAudioOutput(language.value, "aliyun"),
  );

  return (
    <>
      <div className="card">
        <div className="row" style={{ justifyContent: "space-between", marginBottom: 14 }}>
          <div style={{ width: 260, maxWidth: "70%" }}>
            <Dropdown
              id="dd-provider-config"
              label="要配置的模型服务商"
              value={provider}
              options={catalog.PROVIDERS}
              disabled={busy !== null}
              onChange={(value) => setProvider(value as ModelProvider)}
            />
          </div>
          <span
            className={hasKey ? "badge badge-running" : "badge badge-idle"}
            style={{ cursor: "default" }}
          >
            <span className="status-dot" />
            {loading ? "读取中" : hasKey ? "已配置" : "未配置"}
          </span>
        </div>

        <div className="row" style={{ alignItems: "stretch" }}>
          {!hasDraft ? (
            <span className="hint" style={{ alignSelf: "center" }}>
              {hasKey ? "输入新密钥才能覆盖" : "先粘贴密钥再保存"}
            </span>
          ) : null}
          <input
            id="f-api-key"
            ref={input}
            className="form-input mono"
            type="password"
            placeholder={hasKey ? "输入新密钥以覆盖" : isGemini ? "AIza..." : "sk-..."}
            autoComplete="off"
            spellCheck={false}
            disabled={busy !== null}
            aria-label={`${catalog.providerLabel(provider)} API 密钥`}
            onChange={(event) => setHasDraft(event.currentTarget.value.trim().length > 0)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void saveKey();
            }}
          />
          <button
            type="button"
            className="btn btn-primary"
            disabled={!hasDraft || busy !== null}
            onClick={() => void saveKey()}
          >
            <IconSave size={15} />
            {busy === "save" ? "保存中…" : "保存"}
          </button>
        </div>

        <div className="row-wrap" style={{ marginTop: 12 }}>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => void api.openProviderConsole(provider)}
          >
            <IconExternal size={15} />
            {isGemini ? "AI Studio" : "百炼控制台"}
          </button>
          {hasKey ? (
            armed ? (
              <>
                <button
                  type="button"
                  className="btn btn-danger btn-sm"
                  disabled={busy !== null}
                  onClick={() => void clearKey()}
                >
                  {busy === "clear" ? "清除中…" : "确认清除"}
                </button>
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  disabled={busy !== null}
                  onClick={() => setArmed(false)}
                >
                  取消
                </button>
              </>
            ) : (
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                disabled={busy !== null}
                onClick={() => {
                  setArmed(true);
                  if (timer.current) clearTimeout(timer.current);
                  timer.current = setTimeout(() => setArmed(false), 5000);
                }}
              >
                <IconTrash size={15} />
                清除密钥
              </button>
            )
          ) : null}
        </div>
      </div>

      <div className="panel">
        <div className="panel-top">
          <div className="panel-title">模型能力</div>
        </div>
        <div className="panel-body">
          <div className="sub-card" style={{ marginBottom: 14 }}>
            <div className="sub-card-head">实时翻译模型</div>
            <div className="row-wrap">
              <span className="mono">
                {isGemini ? catalog.GEMINI_MODEL_LABEL : catalog.DEFAULT_MODEL_LABEL}
              </span>
              <span className="hint mono">
                {isGemini ? catalog.GEMINI_MODEL_NAME : catalog.DEFAULT_MODEL_NAME}
              </span>
            </div>
          </div>

          {isGemini ? (
            <div className="input-grid-3">
              <div className="sub-card">
                <div className="sub-card-head">语言</div>
                <div className="si-title">70+</div>
                <div className="hint">实时语音互译</div>
              </div>
              <div className="sub-card">
                <div className="sub-card-head">上行音频</div>
                <div className="mono">PCM16LE · 16 kHz</div>
                <div className="hint">单声道</div>
              </div>
              <div className="sub-card">
                <div className="sub-card-head">下行音频</div>
                <div className="mono">PCM16LE · 24 kHz</div>
                <div className="hint">自动音色 · 单声道</div>
              </div>
            </div>
          ) : (
            <div className="input-grid-2">
              <div className="sub-card">
                <div className="sub-card-head">译音 · {audioLanguages.length} 种</div>
                <div className="row-wrap">
                  {audioLanguages.slice(0, 8).map((language) => (
                    <span className="chip static" key={language.value}>{language.label}</span>
                  ))}
                </div>
                <details className="catalog-details">
                  <summary>查看全部</summary>
                  <div className="row-wrap">
                    {audioLanguages.map((language) => (
                      <span className="chip static" key={language.value}>{language.label}</span>
                    ))}
                  </div>
                </details>
              </div>

              <div className="sub-card">
                <div className="sub-card-head">仅译文 · {textOnlyLanguages.length} 种</div>
                <div className="row-wrap">
                  {textOnlyLanguages.slice(0, 8).map((language) => (
                    <span className="chip static" key={language.value}>{language.label}</span>
                  ))}
                </div>
              </div>

              <div className="sub-card" style={{ gridColumn: "1 / -1" }}>
                <div className="sub-card-head">官方音色 · {voices.length} 个</div>
                <div className="row-wrap">
                  {catalog.VOICE_CATALOG.slice(0, 8).map((voice) => (
                    <span className="chip static" key={voice.id}>{voice.name}</span>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </>
  );
}
