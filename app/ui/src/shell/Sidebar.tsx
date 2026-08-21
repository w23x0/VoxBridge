/**
 * 浮空侧栏（64px）。上半区页面导航（有激活态 + 左侧 4×20 竖条），
 * 一条 divider 之后是下半区工具项（无激活态）—— 主题切换在这里，不在右上角。
 */

import { useCallback, useState, type MouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { useT } from "../i18n/context";
import { ABOUT_NAV, NAV, PRIMARY_NAV } from "../nav";
import { IconMoon, IconSun } from "../ui/icons";
import { currentTheme, toggleTheme } from "../ui/theme";
import type { Theme } from "../ui/theme";

export function Sidebar({
  page,
  onNavigate,
}: {
  page: string;
  onNavigate: (id: string) => void;
}) {
  const t = useT();
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

  const pageButton = (id: string, label: string, icon: ReactNode) => {
    const active = page === id;
    return (
      <button
        key={id}
        type="button"
        className={active ? "nav-item active" : "nav-item"}
        data-page={id}
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
      <aside className="sidebar" data-tauri-drag-region>
        <nav className="sidebar-nav" aria-label={t("sidebar.navLabel")}>
          {PRIMARY_NAV.map((item) => {
            const Icon = item.icon;
            return pageButton(item.id, item.label(t), <Icon />);
          })}
        </nav>

        {/* 固定三项：关于 / 设置是页面，主题是动作。暂不放语言入口。 */}
        <div className="sidebar-bottom">
          <div className="nav-divider" aria-hidden="true" />
          {pageButton(ABOUT_NAV.id, ABOUT_NAV.label(t), <ABOUT_NAV.icon />)}
          {(() => {
            const settings = NAV.find((item) => item.id === "settings");
            if (!settings) return null;
            const Icon = settings.icon;
            return pageButton(settings.id, settings.label(t), <Icon />);
          })()}
          <button
            type="button"
            className="nav-item"
            aria-label={theme === "dark" ? t("sidebar.toLight") : t("sidebar.toDark")}
            onClick={() => setTheme(toggleTheme())}
            {...hoverTip(t("sidebar.toggleTheme"))}
          >
            {theme === "dark" ? <IconSun /> : <IconMoon />}
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
