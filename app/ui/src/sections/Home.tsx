/**
 * 首页：两个常驻功能的启停、听取目标和运行时状态。
 *
 * `listen.target` 是整块替换语义；更换程序或切换子进程选项时，
 * executable / display_name / include_process_tree 必须一起提交。
 */

import * as catalog from "../catalog";
import { LatencyPanel } from "../components/Latency";
import { useT } from "../i18n/context";
import { recentFirst, useRecentValues } from "../lib/recent";
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

function deviceOptions(
  devices: DeviceInfo[],
  selected: string | null,
  t: ReturnType<typeof useT>,
): Option[] {
  const options: Option[] = [{ value: SYSTEM_DEFAULT, label: t("pipeline.systemDefault") }];
  for (const device of devices) {
    options.push({
      value: device.name,
      label: device.is_default ? `${device.name}${t("pipeline.deviceDefaultSuffix")}` : device.name,
    });
  }
  if (selected && !options.some((option) => option.value === selected)) {
    options.push({ value: selected, label: `${selected}${t("pipeline.deviceUnconnectedSuffix")}` });
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
  const t = useT();
  const pipeLabel = (id: Extract<PipelineName, "speak" | "listen">) =>
    id === "speak" ? t("pipeline.speak") : t("pipeline.listen");
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
    label: voice.recommended ? `${voice.label}${t("catalog.defaultVoiceSuffix")}` : voice.label,
  }));
  const listenVoiceOptions = orderedVoices(listen.voice, recentListenVoices).map((voice) => ({
    value: voice.value,
    label: voice.recommended ? `${voice.label}${t("catalog.defaultVoiceSuffix")}` : voice.label,
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
    label: `${info.name}${info.active ? "" : t("pipeline.appNotActiveSuffix")}`,
  }));
  if (target && !byExe.has(target.executable)) {
    appOptions.unshift({ value: target.executable, label: `${target.display_name}${t("pipeline.appNotRunningSuffix")}` });
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
    if (!snapshot) return t("pipeline.openFailReason");
    const provider = pipeline === "speak" ? speak.provider : listen.provider;
    if (!snapshot.api_keys[provider]) {
      return t("pipeline.blockedNoApiKey", { provider: catalog.providerLabel(provider) });
    }
    if (pipeline === "listen" && !target) return t("pipeline.blockedSelectApp");
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
                  <div className="stat-label">{pipeLabel(card.id)}</div>
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
                      {t("pipeline.provider")}
                    </label>
                    <Dropdown
                      id={`dd-home-${card.id}-provider`}
                      label={t("pipeline.providerAria", { pipe: pipeLabel(card.id) })}
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
                        {t("pipeline.targetLanguage")}
                      </label>
                      <Dropdown
                        id="dd-home-target-language"
                        label={t("pipeline.targetLanguageAria")}
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
                        {t("pipeline.sourceLanguage")}
                      </label>
                      <Dropdown
                        id="dd-home-listen-source"
                        label={t("pipeline.sourceLanguageAria")}
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
                        {t("pipeline.voice")}
                      </label>
                      <div className="pipeline-inline-toggle">
                        <span>
                          {card.id === "speak"
                            ? t("pipeline.showTranslation")
                            : t("pipeline.playTranslation")}
                        </span>
                        <Toggle
                          checked={
                            card.id === "speak"
                              ? speak.show_translation
                              : listen.speak_translation
                          }
                          label={
                            card.id === "speak"
                              ? t("pipeline.showTranslation")
                              : t("pipeline.playTranslation")
                          }
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
                      label={t("pipeline.voiceAria", { pipe: pipeLabel(card.id) })}
                      value={card.id === "speak" ? speak.voice : listen.voice}
                      options={
                        (card.id === "speak" ? speak.provider : listen.provider) === "gemini"
                          ? [{
                              value: card.id === "speak" ? speak.voice : listen.voice,
                              label: t("catalog.geminiAutoVoice"),
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
                          {t("pipeline.outputDevice")}
                        </label>
                        <Dropdown
                          id={`dd-home-${card.id}-output`}
                          label={t("pipeline.outputDeviceAria", { pipe: pipeLabel(card.id) })}
                          value={
                            (card.id === "speak" ? speak.output_device : listen.output_device) ??
                            SYSTEM_DEFAULT
                          }
                          options={deviceOptions(
                            snapshot?.devices.outputs ?? [],
                            card.id === "speak" ? speak.output_device : listen.output_device,
                            t,
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
                    {t("home.textOnlyHint", { lang: catalog.languageLabel(speak.target_language) })}
                  </div>
                ) : null}
              </div>

              {card.id === "speak" ? (
                <div style={{ marginTop: 10 }}>
                  <label className="stat-label" htmlFor="dd-home-input-device">
                    {t("pipeline.inputDevice")}
                  </label>
                  <div className="row" style={{ alignItems: "stretch", marginTop: 6 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <Dropdown
                        id="dd-home-input-device"
                        label={t("pipeline.inputDeviceAria")}
                        value={speak.input_device ?? SYSTEM_DEFAULT}
                        options={deviceOptions(inputs, speak.input_device, t)}
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
                      aria-label={t("pipeline.rescanMic")}
                      onClick={() => void api.refreshDevices()}
                    >
                      <IconRefresh size={15} />
                    </button>
                  </div>
                  {missingInput || inputs.length === 0 ? (
                    <div className="hint hint-warn" style={{ marginTop: 6 }}>
                      {missingInput ? t("pipeline.micNotConnected") : t("pipeline.micNotFound")}
                    </div>
                  ) : null}

                  <div
                    className="row"
                    style={{ justifyContent: "space-between", marginTop: 12 }}
                  >
                    <div className="stat-label">{t("pipeline.monitorTranslation")}</div>
                    <Toggle
                      checked={speak.monitor_translation}
                      disabled={!snapshot}
                      label={t("pipeline.monitorTranslation")}
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
                    {t("pipeline.listenTarget")}
                  </label>
                  <div className="row" style={{ alignItems: "stretch", marginTop: 6 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <Dropdown
                        id="dd-home-listen-target"
                        label={t("pipeline.listenTargetAria")}
                        value={target?.executable ?? ""}
                        options={appOptions}
                        placeholder={t("pipeline.listenTargetPlaceholder")}
                        disabled={!snapshot || appOptions.length === 0}
                        onChange={pickApp}
                      />
                    </div>
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      aria-label={t("pipeline.rescanApps")}
                      onClick={() => void api.refreshDevices()}
                    >
                      <IconRefresh size={15} />
                    </button>
                  </div>
                  {appOptions.length === 0 || !target ? (
                    <div className="hint hint-warn" style={{ marginTop: 6 }}>
                      {appOptions.length === 0 ? t("pipeline.appNoAudioFound") : t("pipeline.appSelectApp")}
                    </div>
                  ) : null}
                  <div className="row" style={{ justifyContent: "space-between", marginTop: 10 }}>
                    <span className="stat-label">{t("pipeline.includeSubprocess")}</span>
                    <Toggle
                      checked={target?.include_process_tree ?? true}
                      disabled={!target}
                      label={t("pipeline.includeSubprocess")}
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
                {running ? t("pipeline.stop") : t("pipeline.start")}
              </button>
            </div>
          );
        })}
      </div>

      <div className="panel">
        <div className="panel-top">
          <div className="panel-title">{t("home.panelTitle")}</div>
          {api.mock ? <span className="badge badge-warn">{t("home.demoBadge")}</span> : null}
        </div>
        <div className="panel-body">
          <div className="stats-row cols-2">
            <LatencyPanel
              label={t("pipeline.speak")}
              snapshot={snapshot?.speak ?? null}
              showTranslation={speak.show_translation}
              speakTranslation
            />
            <LatencyPanel
              label={t("pipeline.listen")}
              snapshot={snapshot?.listen ?? null}
              showTranslation={listen.show_translation}
              speakTranslation={listen.speak_translation}
            />
          </div>

          {snapshot?.headphones_advised ? (
            <div className="hint hint-warn" style={{ marginTop: 12 }}>
              {t("home.headphonesHint")}
            </div>
          ) : null}
        </div>
      </div>
    </>
  );
}
