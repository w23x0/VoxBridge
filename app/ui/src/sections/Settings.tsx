/** 设置：虚拟麦克风、系统启动行为和快捷键。 */

import { useState } from "react";
import { useStore } from "../store";
import type { ActivationMode, Hotkey } from "../types";
import type { AudioApp } from "../types.snapshot";
import { Dropdown, SettingsItem, Toggle } from "../ui/controls";
import { HotkeyEditor } from "../ui/hotkey";
import { IconDownload, IconExternal, IconTrash } from "../ui/icons";
import { useToast } from "../ui/toast";

const DEFAULT_LISTEN_HOTKEY: Hotkey = { ctrl: true, alt: false, shift: false, key: "L" };

const ACTIVATION = [
  { value: "toggle", label: "按键切换" },
  { value: "hold", label: "按住说话" },
];

interface UninstallDialogState {
  blockers: AudioApp[];
  loading: boolean;
}

export function SettingsPage() {
  const { api, snapshot, settings, patch, applyCableChannelStatus } = useStore();
  const toast = useToast();
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
    ? "检测中"
    : cableStatus === "installed"
      ? "已安装"
      : cableStatus === "install_pending_reboot"
        ? "安装待重启"
        : cableStatus === "uninstall_incomplete"
          ? "卸载未完成"
          : "未安装";
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
            ? "安装完成，请重启 Windows"
            : "卸载完成，请重启 Windows",
        );
      } else if (action === "install" && !result.multichannel_hidden) {
        toast("warning", "安装完成，多声道设备隐藏失败");
      } else {
        toast(
          "success",
          action === "install" ? "虚拟麦克风已安装" : "虚拟麦克风已卸载",
        );
      }
    } catch (error: unknown) {
      toast("danger", `${action === "install" ? "安装" : "卸载"}失败：${String(error)}`);
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
      toast("danger", `检测占用失败：${String(error)}`);
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
        toast("warning", "请重启 Windows 使设置生效");
      } else {
        toast(
          "success",
          visible ? "多声道设备已显示" : "多声道设备已隐藏",
        );
      }
    } catch (error: unknown) {
      toast("danger", `修改多声道设备失败：${String(error)}`);
    } finally {
      setCableBusy(null);
    }
  };

  return (
    <>
      <div className="settings-group">
        <SettingsItem
          title="虚拟麦克风"
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
                  {cableIncomplete ? "继续卸载" : "卸载"}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn btn-primary btn-sm"
                  disabled={loading || cableBusy !== null}
                  onClick={() => void manageCable("install")}
                >
                  <IconDownload size={14} />
                  {cableBusy === "install" ? "安装中…" : "安装"}
                </button>
              )}
            </div>
          }
        />
        <SettingsItem
          title="驱动来源"
          control={
            <div className="row">
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void api.openVirtualCableWebsite()}
              >
                <IconExternal size={14} />
                官网
              </button>
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void api.openVirtualCableDonation()}
              >
                授权与捐赠
              </button>
            </div>
          }
        />
        <SettingsItem
          title="多声道设备"
          control={
            <div className="row">
              <span className={channelBadgeClass}>
                {channelStatus === "hidden"
                  ? "已隐藏"
                  : channelStatus === "visible"
                    ? "已显示"
                    : "无需处理"}
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
                      ? "恢复中…"
                      : "显示"
                    : cableBusy === "hide16"
                      ? "隐藏中…"
                      : "隐藏"}
                </button>
              ) : null}
            </div>
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          title="开机自启"
          control={
            <Toggle
              checked={settings.autostart}
              disabled={loading}
              label="开机自启"
              onChange={(autostart) => patch({ autostart })}
            />
          }
        />
        <SettingsItem
          title="启动后最小化"
          control={
            <Toggle
              checked={settings.start_minimized}
              disabled={loading}
              label="启动后最小化"
              onChange={(startMinimized) => patch({ start_minimized: startMinimized })}
            />
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          htmlFor="dd-activation"
          title="对外说话 · 激活方式"
          control={
            <Dropdown
              id="dd-activation"
              label="对外说话的激活方式"
              value={speak.activation_mode}
              options={ACTIVATION}
              onChange={(activationMode) =>
                patch({ speak: { activation_mode: activationMode as ActivationMode } })
              }
            />
          }
        />

        <SettingsItem
          title="对外说话 · 快捷键"
          control={
            <HotkeyEditor
              id="dd-speak-key"
              label="对外说话"
              hotkey={speak.hotkey}
              onChange={(hotkey) => patch({ speak: { hotkey } })}
            />
          }
        />

        <SettingsItem
          title="听人说话 · 快捷键"
          control={
            <Toggle
              checked={listenHotkey !== null}
              label="启用听人说话快捷键"
              onChange={(enabled) =>
                patch({ listen: { hotkey: enabled ? DEFAULT_LISTEN_HOTKEY : null } })
              }
            />
          }
        />

        {listenHotkey ? (
          <SettingsItem
            title="听人说话 · 按键组合"
            control={
              <HotkeyEditor
                id="dd-listen-key"
                label="听人说话"
                hotkey={listenHotkey}
                onChange={(hotkey) => patch({ listen: { hotkey } })}
              />
            }
          />
        ) : null}
      </div>

      {uninstallDialog ? (
        <div className="dialog-backdrop" role="presentation">
          <div
            className="dialog-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="cable-uninstall-title"
          >
            <h2 id="cable-uninstall-title">卸载虚拟麦克风</h2>
            {uninstallDialog.loading ? (
              <p>检测中…</p>
            ) : uninstallDialog.blockers.length > 0 ? (
              <>
                <p className="hint-danger">
                  以下应用将被关闭，未保存内容可能丢失。
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
                  ? "将短暂重启 Windows 音频服务。"
                  : "未检测到占用。"}
              </p>
            )}
            <div className="dialog-actions">
              <button
                type="button"
                className="btn btn-secondary"
                disabled={cableBusy !== null}
                onClick={() => setUninstallDialog(null)}
              >
                取消
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
                    ? "卸载中…"
                    : uninstallDialog.blockers.length > 0
                      ? "关闭应用并卸载"
                      : cableIncomplete
                        ? "重置音频并卸载"
                        : "卸载"}
                </button>
              ) : null}
            </div>
          </div>
        </div>
      ) : null}

    </>
  );
}
