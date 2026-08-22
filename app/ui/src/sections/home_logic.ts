/**
 * 首页的纯逻辑：设备下拉选项、监听程序去重、选程序、开不了的原因。
 * 从 Home.tsx 抽出来，让 HomePage 只剩外层骨架、PipelineCard 承载单卡。
 *
 * `listen.target` 是整块替换语义；更换程序或切换子进程选项时，
 * executable / display_name / include_process_tree 必须一起提交。
 */

import { useT } from "../i18n/context";
import type { ListenTarget, PipelineName } from "../types";
import type { AudioApp, DeviceInfo, Snapshot } from "../types.snapshot";
import type { Option } from "../ui/controls";

/** 设备下拉里「系统默认」对应的哨兵值；选它等价于把设备写回 null。 */
export const SYSTEM_DEFAULT = "@@system-default@@";

export type HomePipeline = Extract<PipelineName, "speak" | "listen">;

export interface HomeCard {
  id: HomePipeline;
  tone: "blue" | "green";
  icon: (p: { size?: number }) => React.ReactElement;
}

export function deviceOptions(
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

/** 浏览器等程序会开出多个音频会话；按可执行文件名合并，避免下拉重复。 */
export function buildAppOptions(
  apps: AudioApp[],
  target: ListenTarget | null,
  t: ReturnType<typeof useT>,
): Option[] {
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
    appOptions.unshift({
      value: target.executable,
      label: `${target.display_name}${t("pipeline.appNotRunningSuffix")}`,
    });
  }
  return appOptions;
}

/** 选了某个程序：必须整块替换 listen.target（executable/display_name/include_process_tree 一起）。 */
export function pickApp(
  apps: AudioApp[],
  target: ListenTarget | null,
  exe: string,
  patch: (p: { listen: { target: ListenTarget } }) => void,
): void {
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
}

/** 开不了的原因。null 表示能开。 */
export function blockedBy(
  pipeline: PipelineName,
  snapshot: Snapshot | null,
  speakProvider: string,
  listenProvider: string,
  hasKey: (provider: string) => boolean,
  listenMissingTarget: boolean,
  t: ReturnType<typeof useT>,
  providerLabel: (provider: string) => string,
): string | null {
  if (!snapshot) return t("pipeline.openFailReason");
  const provider = pipeline === "speak" ? speakProvider : listenProvider;
  if (!hasKey(provider)) {
    return t("pipeline.blockedNoApiKey", { provider: providerLabel(provider) });
  }
  if (pipeline === "listen" && listenMissingTarget) return t("pipeline.blockedSelectApp");
  return null;
}
