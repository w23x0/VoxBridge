/**
 * 主题切换。照 kirox-ui skill 的 references/shell.md。
 *
 * 两个要点：
 *  1. 切换前必须锁住所有 transition —— 否则几十个元素同时补间会闪。
 *     用注入 <style> 的方式而不是加 class，因为 class 要等下一帧才生效。
 *  2. 用 View Transition 从左下角画圆扩散。不支持或用户要求减少动效时降级。
 *
 * 首帧那次套用在 index.html 的内联脚本里，不在这儿 —— 等模块加载完就晚了，会闪一下。
 */

export type Theme = "light" | "dark";

const KEY = "app-theme";

function read(): Theme | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "light" ? v : null;
  } catch {
    // WebView 里 localStorage 偶尔被禁（隐私模式 / 沙箱），读不到就当没存过
    return null;
  }
}

/** 当前生效的主题。tokens.css 的暗色挂在 [data-theme=dark] 上，没属性就是亮色。 */
export function currentTheme(): Theme {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}

/** 启动时恢复。index.html 已经同步做过一次，这里只兜「属性没写上」的情况。 */
export function restoreTheme(): Theme {
  const saved = read();
  if (saved === "dark") document.documentElement.setAttribute("data-theme", "dark");
  else if (saved === "light") document.documentElement.removeAttribute("data-theme");
  return currentTheme();
}

/** 切到另一个主题，返回切完之后的值。 */
export function toggleTheme(): Theme {
  const root = document.documentElement;
  const toDark = currentTheme() !== "dark";

  // 1. 注入样式锁死过渡
  const lock = document.createElement("style");
  lock.textContent = "*, *::before, *::after { transition-duration: 0s !important; }";
  document.head.appendChild(lock);

  const apply = () => {
    if (toDark) root.setAttribute("data-theme", "dark");
    else root.removeAttribute("data-theme");
    try {
      localStorage.setItem(KEY, toDark ? "dark" : "light");
    } catch {
      // 存不下就只在本次会话里生效，不影响切换本身
    }
  };
  const unlock = () => setTimeout(() => lock.remove(), 100);

  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!document.startViewTransition || reduce) {
    apply();
    unlock();
    return toDark ? "dark" : "light";
  }

  const vt = document.startViewTransition(apply);
  void vt.finished.then(unlock);
  void vt.ready.then(() => {
    // 左下角起点，半径取到最远角
    const x = 0;
    const y = innerHeight;
    const r = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
    root.animate(
      { clipPath: [`circle(0% at ${x}px ${y}px)`, `circle(${r}px at ${x}px ${y}px)`] },
      {
        duration: 500,
        easing: "ease-in-out",
        pseudoElement: "::view-transition-new(root)",
      },
    );
  });

  return toDark ? "dark" : "light";
}
