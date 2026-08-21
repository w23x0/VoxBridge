/**
 * 类名对账：JSX 里用到的 class，CSS 里必须有定义。
 *
 * 为什么要这条：换 UI 规格时 CSS 全换了，但八个业务页面还在用旧类名 ——
 * 编译能过、a11y 也过（布局断言看的是 flex 位置），可控件长得跟没上样式一样。
 * 这种漏子只有对账能抓住。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const SRC = "src";
const CSS_FILES = [
  "src/styles/tokens.css",
  "src/styles/shell.css",
  "src/styles/components.css",
  "src/styles/subtitle.css",
];

/** 不是样式类、或由 CSS 属性选择器/内联样式管的，不参与对账。 */
const IGNORE = new Set(["show", "hide", "active", "selected", "cursor", "running", "static"]);

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (/\.tsx$/.test(p)) out.push(p);
  }
  return out;
}

// CSS 里定义过的类
const defined = new Set();
for (const f of CSS_FILES) {
  const css = readFileSync(f, "utf8");
  for (const m of css.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)) defined.add(m[1]);
}

// JSX 里用到的类。只取字符串字面量，模板里的 ${} 拼接部分取不到，
// 所以带变量的那几处（stat-icon ${tone} / badge badge-${x}）要在下面单独列。
const used = new Map();
for (const f of walk(SRC)) {
  const src = readFileSync(f, "utf8");
  for (const m of src.matchAll(/className\s*=\s*(?:"([^"]*)"|\{`([^`]*)`\}|\{[^}]*?"([^"]*)"[^}]*?\})/g)) {
    /*
     * 带 ${} 的类名整段丢掉：`toast-${tone}` 这种静态查不出真名，
     * 只留碎片 "toast-" 反而是误报。真实取值列在下面的 DYNAMIC 里。
     * 前后各吃掉相邻的 [\w-]，所以 `stat-icon ${tone}` 里的 stat-icon 会留下。
     */
    const raw = `${m[1] ?? ""} ${m[2] ?? ""} ${m[3] ?? ""}`.replace(
      /[\w-]*\$\{[^}]*\}[\w-]*/g,
      " ",
    );
    for (const cls of raw.split(/[\s${}]+/)) {
      if (!cls || /[^\w-]/.test(cls)) continue;
      if (IGNORE.has(cls)) continue;
      if (!used.has(cls)) used.set(cls, new Set());
      used.get(cls).add(f);
    }
  }
}

// 模板拼出来的类，手工补齐（改了要同步）
const DYNAMIC = [
  "stat-icon", "indigo", "green", "cyan", "amber", "red", "violet", "blue",
  "badge-idle", "badge-running", "badge-danger", "badge-warn",
  "toast-success", "toast-danger", "toast-warning",
];
for (const c of DYNAMIC) if (!used.has(c)) used.set(c, new Set(["(模板拼接)"]));

const missing = [...used].filter(([c]) => !defined.has(c));
const unused = [...defined].filter(
  (c) => !used.has(c) && !/^(num|mono|label-xs|t-|no-scrollbar|theme-switching|app-shell|dark|page|sidebar|nav|window|btn|form|toggle|chip|badge|status|dot|dropdown|modal|toast|log|progress|meter|slider|sub|stat|panel|card|field|empty|input|mode|si-|row|col|spacer|hint|subtitle)/.test(c),
);

console.log(`CSS 定义 ${defined.size} 个类，JSX 用到 ${used.size} 个`);
if (missing.length) {
  console.log(`\n✗ 用了但 CSS 里没有（${missing.length} 个）：`);
  for (const [c, files] of missing) {
    console.log(`  .${c}  ←  ${[...files].map((f) => f.replace(/\\/g, "/")).join(", ")}`);
  }
} else {
  console.log("\n✓ 所有用到的类都有 CSS 定义");
}
if (unused.length) console.log(`\n· CSS 里定义了没用到（不算错）：${unused.join(" ")}`);

process.exitCode = missing.length ? 1 : 0;
