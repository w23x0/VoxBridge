/**
 * 浮空侧栏（64px）。上半区页面导航（有激活态 + 左侧 4×20 竖条），
 * 一条 divider 之后是下半区工具项（无激活态）—— 关于 / 设置 / 主题 / 语言。
 * 语言在设置之外、挂在侧栏底部，对齐 GlassUI 的 utility 段（design-system.md）。
 */

import { useCallback, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { useT } from "../i18n/context";
import { toUiLang, useLang } from "../i18n/context";
import { UI_LANG_CYCLE, UI_LANG_SHORT, type UiLang } from "../i18n/types";
import { ABOUT_NAV, NAV, PRIMARY_NAV } from "../nav";
import { useStore } from "../store";
import { IconMoon, IconSun } from "../ui/icons";
import { currentTheme, toggleTheme } from "../ui/theme";
import type { Theme } from "../ui/theme";

/** ←/→ 触发动作：主题 / 语言按钮在窄侧栏里左右没有东西可跳，
 *  给左右键绑上动作本身（循环切换）。必须 stopPropagation ——
 *  不拦的话 App 的 document 监听会把它当列切换，把焦点跳到右侧内容区。 */
function actionKey(e: KeyboardEvent<HTMLButtonElement>, action: () => void) {
  if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.stopPropagation();
    e.preventDefault();
    action();
  }
}

export function Sidebar({
  page,
  onNavigate,
}: {
  page: string;
  onNavigate: (id: string) => void;
}) {
  const t = useT();
  const { uiLang } = useLang();
  const { settings, patch } = useStore();
  const [theme, setTheme] = useState<Theme>(() => currentTheme());
  const [tip, setTip] = useState<{ label: string; y: number } | null>(null);

  const hoverTip = useCallback(
    (label: string) => ({
      onMouseEnter: (event: MouseEvent<HTMLButtonElement>) => {
        const rect = event.currentTarget.getBoundingClientRect();
        setTip({ label, y: rect.top + rect.height / 2 });
      },
      onMouseLeave: () => setTip(null),
    }),
    [],
  );

  /** 循环切到下一个界面语言（zh-CN → ja-JP → en → 循环），写回 settings。 */
  const cycleLanguage = useCallback(() => {
    const cur = toUiLang(settings.ui_language);
    const i = UI_LANG_CYCLE.indexOf(cur);
    const next: UiLang = UI_LANG_CYCLE[(i + 1) % UI_LANG_CYCLE.length];
    patch({ ui_language: next });
  }, [settings.ui_language, patch]);

  const pageButton = (id: string, label: string, icon: ReactNode) => {
    const active = page === id;
    return (
      <button
        key={id}
        type="button"
        className={active ? "nav-item active" : "nav-item"}
        data-page={id}
        data-focus-item
        aria-current={active ? "page" : undefined}
        aria-label={label}
        onClick={() => onNavigate(id)}
        {...hoverTip(label)}
      >
        <span className="nav-dot" aria-hidden="true" />
        {icon}
      </button>
    );
  };

  return (
    <>
      <aside className="sidebar" data-tauri-drag-region data-focus-zone="sidebar">
        <nav className="sidebar-nav" aria-label={t("sidebar.navLabel")}>
          {PRIMARY_NAV.map((item) => {
            const Icon = item.icon;
            return pageButton(item.id, item.label(t), <Icon />);
          })}
        </nav>

        {/* 固定：关于 / 设置是页面，主题 / 语言是动作。 */}
        <div className="sidebar-bottom">
          <div className="nav-divider" aria-hidden="true" />
          {pageButton(ABOUT_NAV.id, ABOUT_NAV.label(t), <ABOUT_NAV.icon />)}
          {(() => {
            const settingsNav = NAV.find((item) => item.id === "settings");
            if (!settingsNav) return null;
            const Icon = settingsNav.icon;
            return pageButton(settingsNav.id, settingsNav.label(t), <Icon />);
          })()}
          <button
            type="button"
            className="nav-item"
            data-focus-item
            aria-label={theme === "dark" ? t("sidebar.toLight") : t("sidebar.toDark")}
            onClick={() => setTheme(toggleTheme())}
            onKeyDown={(e) => actionKey(e, () => setTheme(toggleTheme()))}
            {...hoverTip(t("sidebar.toggleTheme"))}
          >
            {theme === "dark" ? <IconSun /> : <IconMoon />}
          </button>
          <button
            type="button"
            className="nav-item"
            data-focus-item
            aria-label={t("sidebar.language")}
            onClick={cycleLanguage}
            onKeyDown={(e) => actionKey(e, cycleLanguage)}
            {...hoverTip(t("sidebar.language"))}
          >
            <span className="nav-lang-label" lang={uiLang}>
              {UI_LANG_SHORT[uiLang]}
            </span>
          </button>
        </div>
      </aside>

      {tip
        ? createPortal(
            <div className="nav-tooltip" role="tooltip" style={{ top: tip.y }}>
              {tip.label}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}