/** 侧栏导航表。侧栏只有图标，label 用在 hover tooltip 和页面标题上。 */

import type { ReactElement } from "react";
import type { TParams } from "./i18n/types";

import {
  IconChart,
  IconHome,
  IconInfo,
  IconKey,
  IconSettings,
  IconSubtitles,
} from "./ui/icons";

export type LabelFn = (t: (key: string, params?: TParams) => string) => string;

export interface NavItem {
  id: string;
  /** 侧栏 tooltip + 页面大标题。渲染处用 `useT()` 求值，避免模块无 hook 环境。 */
  label: LabelFn;
  /** 侧栏图标。尺寸由 shell.css 的 .nav-item svg 统一定成 22px，这里不传。 */
  icon: (p: { size?: number }) => ReactElement;
}

export const NAV: NavItem[] = [
  {
    id: "home",
    label: (t) => t("nav.home"),
    icon: IconHome,
  },
  {
    id: "providers",
    label: (t) => t("nav.providers"),
    icon: IconKey,
  },
  {
    id: "subtitle",
    label: (t) => t("nav.subtitle"),
    icon: IconSubtitles,
  },
  {
    id: "settings",
    label: (t) => t("nav.settings"),
    icon: IconSettings,
  },
  {
    id: "usage",
    label: (t) => t("nav.usage"),
    icon: IconChart,
  },
];

/** 设置固定放在侧栏底部；其余业务页面留在上半区。 */
export const PRIMARY_NAV = NAV.filter((item) => item.id !== "settings");

/** 关于也是底部页面，但不属于业务导航。 */
export const ABOUT_NAV: NavItem = {
  id: "about",
  label: (t) => t("nav.about"),
  icon: IconInfo,
};

/** 所有需要挂载到内容区的页面。 */
export const PAGE_NAV = [...NAV, ABOUT_NAV];

export const DEFAULT_PAGE = "home";
