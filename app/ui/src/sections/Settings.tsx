/** 设置：系统启动行为、激活模式与快捷键。虚拟麦克风管理区见 CableManager。 */

import { useT } from "../i18n/context";
import { useStore } from "../store";
import type { ActivationMode, Hotkey } from "../types";
import { Dropdown, SettingsItem, Toggle } from "../ui/controls";
import { HotkeyEditor } from "../ui/hotkey";
import { CableManager } from "./CableManager";

const DEFAULT_LISTEN_HOTKEY: Hotkey = { ctrl: true, alt: false, shift: false, key: "L" };

export function SettingsPage() {
  const { snapshot, settings, patch } = useStore();
  const t = useT();
  const speak = settings.speak;
  const listenHotkey = settings.listen.hotkey;
  const loading = snapshot === null;

  const activationOptions = [
    { value: "toggle", label: t("settings.activationToggle") },
    { value: "hold", label: t("settings.activationHold") },
  ];

  return (
    <>
      <CableManager />

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
    </>
  );
}
