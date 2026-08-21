/** 设置：虚拟麦克风、系统启动行为、快捷键和界面语言。 */

import { useState } from "react";
import { useT } from "../i18n/context";
import { UI_LANG_OPTIONS } from "../i18n/types";
import { useStore } from "../store";
import type { ActivationMode, Hotkey } from "../types";
import type { AudioApp } from "../types.snapshot";
import { Dropdown, SettingsItem, Toggle } from "../ui/controls";
import { HotkeyEditor } from "../ui/hotkey";
import { IconDownload, IconExternal, IconTrash } from "../ui/icons";
import { useToast } from "../ui/toast";

const DEFAULT_LISTEN_HOTKEY: Hotkey = { ctrl: true, alt: false, shift: false, key: "L" };

interface UninstallDialogState {
  blockers: AudioApp[];
  loading: boolean;
}

export function SettingsPage() {
  const { api, snapshot, settings, patch, applyCableChannelStatus } = useStore();
  const toast = useToast();
  const t = useT();
  const [cableBusy, setCableBusy] = useState<
    "install" | "uninstall" | "hide16" | "show16" | null
  >(null);
  const [uninstallDialog, setUninstallDialog] = useState<UninstallDialogState | null>(null);
  const speak = settings.speak;
  const listenHotkey = settings.listen.hotkey;
  const loading = snapshot === null;
  const cableStatus = snapshot?.devices.virtual_cable_status ?? "not_installed";
  const cableInstalled = cableStatus === "installed";
  const cablePending = cableStatus === "install_pending_reboot";
  const cableIncomplete = cableStatus === "uninstall_incomplete";
  const cableBadgeClass = cableInstalled
    ? "badge badge-running"
    : cablePending || cableIncomplete
      ? "badge badge-warn"
      : "badge badge-idle";
  const cableStatusLabel = loading
    ? t("settings.cableStatus.checking")
    : cableStatus === "installed"
      ? t("settings.cableStatus.installed")
      : cableStatus === "install_pending_reboot"
        ? t("settings.cableStatus.installPendingReboot")
        : cableStatus === "uninstall_incomplete"
          ? t("settings.cableStatus.uninstallIncomplete")
          : t("settings.cableStatus.notInstalled");
  const channelStatus = snapshot?.devices.virtual_cable_16ch_status ?? "absent";
  const channelBadgeClass =
    channelStatus === "hidden"
      ? "badge badge-running"
      : channelStatus === "visible"
        ? "badge badge-warn"
        : "badge badge-neutral";
  const manageCable = async (action: "install" | "uninstall", closeBlockers = false) => {
    setCableBusy(action);
    try {
      const result =
        action === "install"
          ? await api.installVirtualCable()
          : await api.uninstallVirtualCable(closeBlockers);
      if (action === "uninstall") setUninstallDialog(null);
      if (result.needs_reboot) {
        toast(
          "warning",
          action === "install"
            ? t("settings.toast.installNeedReboot")
            : t("settings.toast.uninstallNeedReboot"),
        );
      } else if (action === "install" && !result.multichannel_hidden) {
        toast("warning", t("settings.toast.hideMultichannelFailed"));
      } else {
        toast(
          "success",
          action === "install" ? t("settings.toast.installDone") : t("settings.toast.uninstallDone"),
        );
      }
    } catch (error: unknown) {
      toast(
        "danger",
        action === "install"
          ? t("settings.toast.installFailed", { error: String(error) })
          : t("settings.toast.uninstallFailed", { error: String(error) }),
      );
    } finally {
      setCableBusy(null);
    }
  };

  const inspectUninstall = async () => {
    setUninstallDialog({ blockers: [], loading: true });
    try {
      const blockers = await api.virtualCableBlockers();
      setUninstallDialog({ blockers, loading: false });
    } catch (error: unknown) {
      setUninstallDialog(null);
      toast("danger", t("settings.toast.checkBlockersFailed", { error: String(error) }));
    }
  };

  const setChannelVisible = async (visible: boolean) => {
    setCableBusy(visible ? "show16" : "hide16");
    try {
      const result = await api.setVirtualCableMultichannelVisible(visible);
      // 后端回报的禁用状态是权威值，不等异步 devices_changed 事件就翻徽标。
      // （devices_changed 只在 outputs/inputs/apps 真的变时才发；16 声道端点被
      // 禁用后 Core Audio 仍可能报 ACTIVE，列表不变，事件不会来，徽标就卡死。）
      applyCableChannelStatus(result.multichannel_hidden ? "hidden" : "visible");
      if (result.needs_reboot) {
        toast("warning", t("settings.toast.channelNeedReboot"));
      } else {
        toast("success", visible ? t("settings.toast.showChannel") : t("settings.toast.hideChannel"));
      }
    } catch (error: unknown) {
      toast("danger", t("settings.toast.setChannelFailed", { error: String(error) }));
    } finally {
      setCableBusy(null);
    }
  };

  const activationOptions = [
    { value: "toggle", label: t("settings.activationToggle") },
    { value: "hold", label: t("settings.activationHold") },
  ];

  return (
    <>
      <div className="settings-group">
        <SettingsItem
          title={t("settings.virtualCable")}
          control={
            <div className="row row-wrap">
              <span className={cableBadgeClass}>
                <span className={cableInstalled ? "status-dot running" : "status-dot"} />
                {cableStatusLabel}
              </span>
              {cablePending ? null : cableInstalled || cableIncomplete ? (
                <button
                  type="button"
                  className={cableIncomplete ? "btn btn-danger btn-sm" : "btn btn-secondary btn-sm"}
                  disabled={loading || cableBusy !== null}
                  onClick={() => void inspectUninstall()}
                >
                  <IconTrash size={14} />
                  {cableIncomplete ? t("settings.microSwitch.continueUninstall") : t("settings.microSwitch.uninstall")}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  disabled={loading || cableBusy !== null}
                  onClick={() => void manageCable("install")}
                >
                  <IconDownload size={14} />
                  {cableBusy === "install" ? t("settings.microSwitch.installing") : t("settings.microSwitch.install")}
                </button>
              )}
            </div>
          }
        />
        <SettingsItem
          title={t("settings.driveSource")}
          control={
            <div className="row">
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void api.openVirtualCableWebsite()}
              >
                <IconExternal size={14} />
                {t("settings.driveSite")}
              </button>
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void api.openVirtualCableDonation()}
              >
                {t("settings.licenseDonate")}
              </button>
            </div>
          }
        />
        <SettingsItem
          title={t("settings.multichannel")}
          control={
            <div className="row">
              <span className={channelBadgeClass}>
                {channelStatus === "hidden"
                  ? t("settings.channelStatus.hidden")
                  : channelStatus === "visible"
                    ? t("settings.channelStatus.visible")
                    : t("settings.channelStatus.none")}
              </span>
              {cableInstalled && channelStatus !== "absent" ? (
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  disabled={cableBusy !== null}
                  onClick={() => void setChannelVisible(channelStatus === "hidden")}
                >
                  {channelStatus === "hidden"
                    ? cableBusy === "show16"
                      ? t("settings.microSwitch.showing")
                      : t("settings.microSwitch.show")
                    : cableBusy === "hide16"
                      ? t("settings.microSwitch.hiding")
                      : t("settings.microSwitch.hide")}
                </button>
              ) : null}
            </div>
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          title={t("settings.autostart")}
          control={
            <Toggle
              checked={settings.autostart}
              disabled={loading}
              label={t("settings.autostart")}
              onChange={(autostart) => patch({ autostart })}
            />
          }
        />
        <SettingsItem
          title={t("settings.startMinimized")}
          control={
            <Toggle
              checked={settings.start_minimized}
              disabled={loading}
              label={t("settings.startMinimized")}
              onChange={(startMinimized) => patch({ start_minimized: startMinimized })}
            />
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          htmlFor="dd-activation"
          title={t("settings.speakActivation")}
          control={
            <Dropdown
              id="dd-activation"
              label={t("settings.speakActivationAria")}
              value={speak.activation_mode}
              options={activationOptions}
              onChange={(activationMode) =>
                patch({ speak: { activation_mode: activationMode as ActivationMode } })
              }
            />
          }
        />

        <SettingsItem
          title={t("settings.speakHotkey")}
          control={
            <HotkeyEditor
              id="dd-speak-key"
              label={t("settings.speakHotkeyAria")}
              hotkey={speak.hotkey}
              onChange={(hotkey) => patch({ speak: { hotkey } })}
            />
          }
        />

        <SettingsItem
          title={t("settings.listenHotkey")}
          control={
            <Toggle
              checked={listenHotkey !== null}
              label={t("settings.enableListenHotkey")}
              onChange={(enabled) =>
                patch({ listen: { hotkey: enabled ? DEFAULT_LISTEN_HOTKEY : null } })
              }
            />
          }
        />

        {listenHotkey ? (
          <SettingsItem
            title={t("settings.listenHotkeyCombo")}
            control={
              <HotkeyEditor
                id="dd-listen-key"
                label={t("settings.listenHotkeyAria")}
                hotkey={listenHotkey}
                onChange={(hotkey) => patch({ listen: { hotkey } })}
              />
            }
          />
        ) : null}
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          htmlFor="dd-ui-language"
          title={t("uiLanguage.title")}
          control={
            <Dropdown
              id="dd-ui-language"
              label={t("uiLanguage.desc")}
              value={settings.ui_language}
              options={[...UI_LANG_OPTIONS]}
              onChange={(ui_language) => patch({ ui_language })}
            />
          }
        />
      </div>

      {uninstallDialog ? (
        <div className="dialog-backdrop" role="presentation">
          <div
            className="dialog-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="cable-uninstall-title"
          >
            <h2 id="cable-uninstall-title">{t("settings.cableDialog.title")}</h2>
            {uninstallDialog.loading ? (
              <p>{t("settings.cableDialog.loading")}</p>
            ) : uninstallDialog.blockers.length > 0 ? (
              <>
                <p className="hint-danger">
                  {t("settings.cableDialog.closeApps")}
                </p>
                <div className="dialog-app-list">
                  {uninstallDialog.blockers.map((app) => (
                    <div className="dialog-app" key={app.pid}>
                      <span>{app.display_name}</span>
                      <span className="mono">PID {app.pid}</span>
                    </div>
                  ))}
                </div>
              </>
            ) : (
              <p>
                {cableIncomplete
                  ? t("settings.cableDialog.restartAudio")
                  : t("settings.cableDialog.noBlockers")}
              </p>
            )}
            <div className="dialog-actions">
              <button
                type="button"
                className="btn btn-secondary"
                disabled={cableBusy !== null}
                onClick={() => setUninstallDialog(null)}
              >
                {t("settings.cableDialog.cancel")}
              </button>
              {!uninstallDialog.loading ? (
                <button
                  type="button"
                  className="btn btn-danger"
                  disabled={cableBusy !== null}
                  onClick={() =>
                    void manageCable("uninstall", uninstallDialog.blockers.length > 0)
                  }
                >
                  {cableBusy === "uninstall"
                    ? t("settings.cableDialog.uninstalling")
                    : uninstallDialog.blockers.length > 0
                      ? t("settings.cableDialog.uninstallWithClose")
                      : cableIncomplete
                        ? t("settings.cableDialog.resetAndUninstall")
                        : t("settings.cableDialog.uninstall")}
                </button>
              ) : null}
            </div>
          </div>
        </div>
      ) : null}

    </>
  );
}