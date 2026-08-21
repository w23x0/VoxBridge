/**
 * 首页：两个常驻功能的启停、听取目标和运行时状态。
 *
 * `listen.target` 是整块替换语义；更换程序或切换子进程选项时，
 * executable / display_name / include_process_tree 必须一起提交。
 */

import * as catalog from "../catalog";
import { LatencyPanel } from "../components/Latency";
import { recentFirst, useRecentValues } from "../lib/recent";
import { PIPELINE_LABEL } from "../pipeline";
import { useStore } from "../store";
import type { ModelProvider, PipelineName } from "../types";
import type { DeviceInfo } from "../types.snapshot";
import type { Option } from "../ui/controls";
import { Dropdown, Toggle } from "../ui/controls";
import {
  IconHeadphones,
  IconMic,
  IconPlay,
  IconRefresh,
  IconStop,
} from "../ui/icons";
import { defaultVoiceForLanguage, orderedVoices } from "../voices";

const SYSTEM_DEFAULT = "@@system-default@@";

function deviceOptions(devices: DeviceInfo[], selected: string | null): Option[] {
  const options: Option[] = [{ value: SYSTEM_DEFAULT, label: "系统默认" }];
  for (const device of devices) {
    options.push({
      value: device.name,
      label: device.is_default ? `${device.name}（系统默认）` : device.name,
    });
  }
  if (selected && !options.some((option) => option.value === selected)) {
    options.push({ value: selected, label: `${selected}（未连接）` });
  }
  return options;
}

const CARDS: {
  id: Extract<PipelineName, "speak" | "listen">;
  tone: "blue" | "green";
  icon: (p: { size?: number }) => React.ReactElement;
}[] = [
  { id: "speak", tone: "blue", icon: IconMic },
  { id: "listen", tone: "green", icon: IconHeadphones },
];

export function HomePage() {
  const { api, snapshot, settings, patch } = useStore();
  const speak = settings.speak;
  const listen = settings.listen;
  const target = settings.listen.target;
  const inputs = snapshot?.devices.inputs ?? [];
  const apps = snapshot?.devices.apps ?? [];
  const missingInput =
    !!speak.input_device && !inputs.some((device) => device.name === speak.input_device);

  const [recentSpeakLanguages, rememberSpeakLanguage] = useRecentValues(
    "voxbridge.recent.speak-languages",
    speak.target_language,
  );
  const [recentListenLanguages, rememberListenLanguage] = useRecentValues(
    "voxbridge.recent.listen-languages",
    listen.source_language,
  );
  const [recentSpeakVoices, rememberSpeakVoice] = useRecentValues(
    "voxbridge.recent.speak-voices",
    speak.voice,
  );
  const [recentListenVoices, rememberListenVoice] = useRecentValues(
    "voxbridge.recent.listen-voices",
    listen.voice,
  );

  const speakLanguageOptions = recentFirst(catalog.LANGUAGES, recentSpeakLanguages);
  const listenLanguageOptions = [
    catalog.SOURCE_LANGUAGE_OPTIONS[0],
    ...recentFirst(catalog.SOURCE_LANGUAGE_OPTIONS.slice(1), recentListenLanguages),
  ].filter((option): option is catalog.LabeledOption => option !== undefined);
  const speakVoiceOptions = orderedVoices(speak.voice, recentSpeakVoices).map((voice) => ({
    value: voice.value,
    label: voice.recommended ? `${voice.label} · 默认` : voice.label,
  }));
  const listenVoiceOptions = orderedVoices(listen.voice, recentListenVoices).map((voice) => ({
    value: voice.value,
    label: voice.recommended ? `${voice.label} · 默认` : voice.label,
  }));

  // 浏览器等程序会开出多个音频会话；按可执行文件名合并，避免下拉重复。
  const byExe = new Map<string, { name: string; active: boolean }>();
  for (const app of apps) {
    const seen = byExe.get(app.executable);
    if (seen) seen.active = seen.active || app.active;
    else byExe.set(app.executable, { name: app.display_name, active: app.active });
  }

  const appOptions: Option[] = [...byExe].map(([exe, info]) => ({
    value: exe,
    label: `${info.name}${info.active ? "" : "（未发声）"}`,
  }));
  if (target && !byExe.has(target.executable)) {
    appOptions.unshift({ value: target.executable, label: `${target.display_name}（未运行）` });
  }

  const pickApp = (exe: string) => {
    const app = apps.find((candidate) => candidate.executable === exe);
    if (!app) return;
    patch({
      listen: {
        target: {
          executable: app.executable,
          display_name: app.display_name,
          include_process_tree: target?.include_process_tree ?? true,
        },
      },
    });
  };

  /** 开不了的原因。null 表示能开。 */
  const blockedBy = (pipeline: PipelineName): string | null => {
    if (!snapshot) return "读取中";
    const provider = pipeline === "speak" ? speak.provider : listen.provider;
    if (!snapshot.api_keys[provider]) return `请先配置 ${catalog.providerLabel(provider)} API 密钥`;
    if (pipeline === "listen" && !target) return "请先选择监听程序";
    return null;
  };

  return (
    <>
      <div className="stats-row cols-2">
        {CARDS.map((card) => {
          const state = snapshot?.[card.id] ?? null;
          const running = state?.running ?? false;
          const failed = state?.state === "failed";
          const blocked = blockedBy(card.id);
          const Icon = card.icon;

          return (
            <div
              className="stat-card"
              key={card.id}
              style={{ flexDirection: "column", alignItems: "stretch" }}
            >
              <div className="row" style={{ gap: 16 }}>
                <div className={`stat-icon ${card.tone}`}>
                  <Icon size={26} />
                </div>
                <div style={{ minWidth: 0, flex: 1 }}>
                  <div className="stat-label">{PIPELINE_LABEL[card.id]}</div>
                  <div className="row" style={{ gap: 6, marginTop: 4 }}>
                    <span
                      className={
                        failed
                          ? "badge badge-danger"
                          : running
                            ? "badge badge-running"
                            : "badge badge-idle"
                      }
                    >
                      <span className={running && !failed ? "status-dot running" : "status-dot"} />
                      {state?.state_label ?? "读取中"}
                    </span>
                  </div>
                </div>
              </div>

              {!running && blocked ? (
                <div className="hint hint-warn" style={{ marginTop: 10 }}>
                  {blocked}
                </div>
              ) : null}

              <div className="pipeline-config">
                <div className="pipeline-config-grid">
                  <div style={{ minWidth: 0 }}>
                    <label
                      className="stat-label"
                      htmlFor={`dd-home-${card.id}-provider`}
                    >
                      模型服务商
                    </label>
                    <Dropdown
                      id={`dd-home-${card.id}-provider`}
                      label={`${PIPELINE_LABEL[card.id]}的模型服务商`}
                      value={card.id === "speak" ? speak.provider : listen.provider}
                      options={catalog.PROVIDERS}
                      onChange={(provider) =>
                        card.id === "speak"
                          ? patch({
                              speak: {
                                provider: provider as ModelProvider,
                                model_name: catalog.defaultModelForProvider(provider as ModelProvider),
                              },
                            })
                          : patch({
                              listen: {
                                provider: provider as ModelProvider,
                                model_name: catalog.defaultModelForProvider(provider as ModelProvider),
                                source_language: provider === "gemini" ? null : listen.source_language,
                              },
                            })
                      }
                    />
                  </div>

                  {card.id === "speak" ? (
                    <div style={{ minWidth: 0 }}>
                      <label className="stat-label" htmlFor="dd-home-target-language">
                        目标语言
                      </label>
                      <Dropdown
                        id="dd-home-target-language"
                        label="对外说话的目标语言"
                        value={speak.target_language}
                        options={speakLanguageOptions}
                        onChange={(targetLanguage) => {
                          rememberSpeakLanguage(targetLanguage);
                          const nextVoice =
                            speak.voice_by_language[targetLanguage] ??
                            defaultVoiceForLanguage(targetLanguage);
                          patch({
                            speak: {
                              target_language: targetLanguage,
                              voice: nextVoice,
                              voice_by_language: {
                                ...speak.voice_by_language,
                                [targetLanguage]: nextVoice,
                              },
                            },
                          });
                        }}
                      />
                    </div>
                  ) : (
                    <div style={{ minWidth: 0 }}>
                      <label className="stat-label" htmlFor="dd-home-listen-source">
                        对方语言
                      </label>
                      <Dropdown
                        id="dd-home-listen-source"
                        label="听人说话时对方说的语言"
                        value={listen.source_language ?? ""}
                        options={
                          listen.provider === "gemini"
                            ? [catalog.SOURCE_LANGUAGE_OPTIONS[0]].filter(
                                (option): option is catalog.LabeledOption => option !== undefined,
                              )
                            : listenLanguageOptions
                        }
                        disabled={listen.provider === "gemini"}
                        onChange={(language) => {
                          rememberListenLanguage(language);
                          patch({
                            listen: { source_language: language === "" ? null : language },
                          });
                        }}
                      />
                    </div>
                  )}

                  <div className="pipeline-config-wide">
                    <div className="pipeline-config-head">
                      <label
                        className="stat-label"
                        htmlFor={card.id === "speak" ? "dd-home-speak-voice" : "dd-home-listen-voice"}
                      >
                        译音音色
                      </label>
                      <div className="pipeline-inline-toggle">
                        <span>{card.id === "speak" ? "显示译文" : "播放译音"}</span>
                        <Toggle
                          checked={
                            card.id === "speak"
                              ? speak.show_translation
                              : listen.speak_translation
                          }
                          label={card.id === "speak" ? "显示译文" : "播放译音"}
                          onChange={(checked) =>
                            card.id === "speak"
                              ? patch({ speak: { show_translation: checked } })
                              : patch({ listen: { speak_translation: checked } })
                          }
                        />
                      </div>
                    </div>
                    <Dropdown
                      id={card.id === "speak" ? "dd-home-speak-voice" : "dd-home-listen-voice"}
                      label={`${PIPELINE_LABEL[card.id]}的译音音色`}
                      value={card.id === "speak" ? speak.voice : listen.voice}
                      options={
                        (card.id === "speak" ? speak.provider : listen.provider) === "gemini"
                          ? [{
                              value: card.id === "speak" ? speak.voice : listen.voice,
                              label: "Gemini 自动音色",
                            }]
                          : card.id === "speak"
                            ? speakVoiceOptions
                            : listenVoiceOptions
                      }
                      disabled={
                        (card.id === "speak" ? speak.provider : listen.provider) === "gemini" ||
                        (card.id === "listen" && !listen.speak_translation)
                      }
                      onChange={(voice) => {
                        if (card.id === "speak") {
                          rememberSpeakVoice(voice);
                          patch({
                            speak: {
                              voice,
                              voice_by_language: {
                                ...speak.voice_by_language,
                                [speak.target_language]: voice,
                              },
                            },
                          });
                        } else {
                          rememberListenVoice(voice);
                          patch({ listen: { voice } });
                        }
                      }}
                    />

                    {card.id === "speak" || listen.speak_translation ? (
                      <div className="pipeline-output-device">
                        <label className="stat-label" htmlFor={`dd-home-${card.id}-output`}>
                          译音输出
                        </label>
                        <Dropdown
                          id={`dd-home-${card.id}-output`}
                          label={`${PIPELINE_LABEL[card.id]}的译音输出设备`}
                          value={
                            (card.id === "speak" ? speak.output_device : listen.output_device) ??
                            SYSTEM_DEFAULT
                          }
                          options={deviceOptions(
                            snapshot?.devices.outputs ?? [],
                            card.id === "speak" ? speak.output_device : listen.output_device,
                          )}
                          disabled={!snapshot}
                          onChange={(outputDevice) =>
                            card.id === "speak"
                              ? patch({
                                  speak: {
                                    output_device:
                                      outputDevice === SYSTEM_DEFAULT ? null : outputDevice,
                                  },
                                })
                              : patch({
                                  listen: {
                                    output_device:
                                      outputDevice === SYSTEM_DEFAULT ? null : outputDevice,
                                  },
                                })
                          }
                        />
                      </div>
                    ) : null}
                  </div>
                </div>

                {card.id === "speak" && !catalog.supportsAudioOutput(speak.target_language, speak.provider) ? (
                  <div className="hint hint-warn" style={{ marginTop: 8 }}>
                    {catalog.languageLabel(speak.target_language)}仅支持译文
                  </div>
                ) : null}
              </div>

              {card.id === "speak" ? (
                <div style={{ marginTop: 10 }}>
                  <label className="stat-label" htmlFor="dd-home-input-device">
                    麦克风
                  </label>
                  <div className="row" style={{ alignItems: "stretch", marginTop: 6 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <Dropdown
                        id="dd-home-input-device"
                        label="麦克风"
                        value={speak.input_device ?? SYSTEM_DEFAULT}
                        options={deviceOptions(inputs, speak.input_device)}
                        disabled={!snapshot}
                        onChange={(inputDevice) =>
                          patch({
                            speak: {
                              input_device:
                                inputDevice === SYSTEM_DEFAULT ? null : inputDevice,
                            },
                          })
                        }
                      />
                    </div>
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      aria-label="重新扫描麦克风设备"
                      onClick={() => void api.refreshDevices()}
                    >
                      <IconRefresh size={15} />
                    </button>
                  </div>
                  {missingInput || inputs.length === 0 ? (
                    <div className="hint hint-warn" style={{ marginTop: 6 }}>
                      {missingInput ? "麦克风未连接" : "未发现麦克风"}
                    </div>
                  ) : null}

                  <div
                    className="row"
                    style={{ justifyContent: "space-between", marginTop: 12 }}
                  >
                    <div className="stat-label">回听译音</div>
                    <Toggle
                      checked={speak.monitor_translation}
                      disabled={!snapshot}
                      label="回听译音"
                      onChange={(enabled) =>
                        patch({ speak: { monitor_translation: enabled } })
                      }
                    />
                  </div>
                </div>
              ) : null}

              {card.id === "listen" ? (
                <div style={{ marginTop: 10 }}>
                  <label className="stat-label" htmlFor="dd-home-listen-target">
                    监听程序
                  </label>
                  <div className="row" style={{ alignItems: "stretch", marginTop: 6 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <Dropdown
                        id="dd-home-listen-target"
                        label="监听程序"
                        value={target?.executable ?? ""}
                        options={appOptions}
                        placeholder="选择一个程序"
                        disabled={!snapshot || appOptions.length === 0}
                        onChange={pickApp}
                      />
                    </div>
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      aria-label="重新扫描监听程序"
                      onClick={() => void api.refreshDevices()}
                    >
                      <IconRefresh size={15} />
                    </button>
                  </div>
                  {appOptions.length === 0 || !target ? (
                    <div className="hint hint-warn" style={{ marginTop: 6 }}>
                      {appOptions.length === 0 ? "未发现音频程序" : "请选择程序"}
                    </div>
                  ) : null}
                  <div className="row" style={{ justifyContent: "space-between", marginTop: 10 }}>
                    <span className="stat-label">包含子进程</span>
                    <Toggle
                      checked={target?.include_process_tree ?? true}
                      disabled={!target}
                      label="包含子进程"
                      onChange={(include) => {
                        if (target) {
                          patch({
                            listen: { target: { ...target, include_process_tree: include } },
                          });
                        }
                      }}
                    />
                  </div>
                </div>
              ) : null}

              <button
                type="button"
                className={running ? "btn btn-secondary btn-sm" : "btn btn-dark btn-sm"}
                style={{ marginTop: "auto", justifyContent: "center" }}
                disabled={!snapshot || (!running && blocked !== null)}
                onClick={() => void api.togglePipeline(card.id)}
              >
                {running ? <IconStop size={14} /> : <IconPlay size={14} />}
                {running ? "停止" : "启动"}
              </button>
            </div>
          );
        })}
      </div>

      <div className="panel">
        <div className="panel-top">
          <div className="panel-title">延迟</div>
          {api.mock ? <span className="badge badge-warn">演示数据</span> : null}
        </div>
        <div className="panel-body">
          <div className="stats-row cols-2">
            <LatencyPanel
              label="对外说话"
              snapshot={snapshot?.speak ?? null}
              showTranslation={speak.show_translation}
              speakTranslation
            />
            <LatencyPanel
              label="听人说话"
              snapshot={snapshot?.listen ?? null}
              showTranslation={listen.show_translation}
              speakTranslation={listen.speak_translation}
            />
          </div>

          {snapshot?.headphones_advised ? (
            <div className="hint hint-warn" style={{ marginTop: 12 }}>
              两条线路同时运行，请使用耳机
            </div>
          ) : null}
        </div>
      </div>
    </>
  );
}
