/**
 * i18n 核心类型：语言枚举、字典形状、翻译接口。
 *
 * 架构一句话：语言偏好持久化在 Rust `settings.ui_language`（zh-CN / en），
 * 前端 `LanguageProvider` 监听它，向整棵组件树提供 `useT()`。
 * 翻译函数 `t(key)` 支持 `{var}` 占位符替换。
 */

/** 当前支持的界面语言。值是 settings `ui_language` 的取值。 */
export type UiLang = "zh-CN" | "en";

/** 语言选择下拉的展示项（用各自语言的自称，这样在任意语言界面都能认出来）。 */
export const UI_LANG_OPTIONS: ReadonlyArray<{ value: UiLang; label: string }> = [
  { value: "zh-CN", label: "简体中文" },
  { value: "en", label: "English" },
];

/** 字典叶子：翻译后的字符串。占位符形如 `{name}`。 */
export type MessageValue = string | { [key: string]: MessageValue };

/** 字典：一个语言包的完整结构。key 集合两岸必须一致（en 用 `satisfies` 对齐）。 */
export type Dict = { [key: string]: MessageValue };

/** `t` 的替换参数：key → 变量名 → 值。 */
export type TParams = Record<string, unknown>;

/** flatten 后的字典：点分 key → 文案。 */
export type FlatDict = Record<string, string>;