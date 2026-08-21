/* ==========================================================================
   焦点网格 —— 列模型导航（移植玻璃样式的 GlassUI focusGrid）

   模型：
   - 界面没有「模块/面板」这一层 —— 直接操控控件。
   - 控件按行排、行内向左靠齐，行内第 N 个控件属于第 N 层（列）。
       第 0 层 = 侧边栏（data-focus-zone="sidebar"），↑↓ 沿整条侧栏走
       （Shell 里 stepSidebar，因为侧栏项有「切页」这个特殊动作）
       第 1..N 层 = 内容区
   - ↑↓（行间移动）：把内容区当成一张固定的二维格子表 —— 列槽位。
       保持「列槽位」，落到相邻行的、列槽位与当前相交的项；
       没有相交就落到列槽最接近的。这样列不漂移，speak/listen 天然分列不串。
   - ←→（层间切换）：
       → 在侧栏 = 进入内容区第 0 行第 0 个控件
       → 在内容区 = 同一行右移一格；行内最右 = 无动作
       ← 在内容区最左 = 回侧栏当前激活页图标
       ← 在内容区第 N 列 = 同一行左移一格
       ← 在侧栏 = 无动作（已是最左）
   - 失焦 / 红绿灯上按任意方向 = 视作光标在「当前激活页图标」，方向键直接生效

   接入（代价是要查 DOM，但导航是低频操作，性能无所谓）：
   - 可聚焦项标 data-focus-item（禁用的不算 —— disabled 项不在网格里）
   - 侧栏容器标 data-focus-zone="sidebar"
   ========================================================================== */

/** 可聚焦项必须：在 DOM 里、可见、没 disabled。
 *  offsetParent 为 null 说明祖先链上有 display:none（比如非激活页）。 */
function isFocusable(el: HTMLElement): boolean {
  if (el.hasAttribute('disabled') || el.getAttribute('aria-disabled') === 'true') return false;
  if (el.tabIndex < 0 && !el.hasAttribute('data-focus-item')) return false;
  return el.offsetParent !== null || el.getClientRects().length > 0;
}

/** 聚焦到某项。preventScroll：不加的话浏览器为了露出元素会滚容器，
 *  长列表里连按方向键会产生跳动感 —— 我们自己控制滚动更平滑。
 *
 *  追踪范围（上下）：方向键控制控件时，光标的活动范围 = 窗口中间的追踪带，
 *  不是整个窗口上下（否则光标能跑到屏幕最顶/最底，看不清在哪）。
 *  追踪带 = 原全屏带（顶 60px 起、底 20px 止）的 3/4，上下各向中间收 1/8。
 *  光标一出带就滚到带边缘，始终停在看得见的位置。
 *  注意 .app-content 要有底部留白，否则最后一行物理上滚不进带内。 */
export function focusItem(el: HTMLElement | undefined | null): void {
  if (!el) return;
  el.focus({ preventScroll: true });
  const scroller = el.closest('.app-content') as HTMLElement | null;
  if (!scroller) return;
  const r = el.getBoundingClientRect();
  const TOP_MARGIN = 60;
  const BOTTOM_MARGIN = 20;
  const ZONE_RATIO = 3 / 4;
  const band = Math.max(0, window.innerHeight - TOP_MARGIN - BOTTOM_MARGIN);
  const shrink = (band * (1 - ZONE_RATIO)) / 2;
  const topBound = TOP_MARGIN + shrink;
  const bottomBound = window.innerHeight - BOTTOM_MARGIN - shrink;
  if (r.top < topBound) {
    scroller.scrollTo({ top: scroller.scrollTop + (r.top - topBound), behavior: 'smooth' });
  } else if (r.bottom > bottomBound) {
    scroller.scrollTo({ top: scroller.scrollTop + (r.bottom - bottomBound), behavior: 'smooth' });
  }
}

/* --------------------------------------------------------------------------
   内容区网格（列模型）。核心约定 —— 一张固定的二维格子表，不比较坐标距离：
   - 内容区项 = 所有 [data-focus-item]，排除侧栏里的。
   - 行 = 按 top 分组（容差 ROW_TOLERANCE，同排项横顶齐）。
   - 列 = 把每行的横向宽度均匀切成 COL_COUNT 个「列槽」；
       每行每个项占据「一段连续列槽」[lo,hi] —— 行里项数少（像 1×2 开关）时一项跨多槽。
   - 移动（= 表格光标）：
       ↓↑：保持列槽位 —— 选邻行的、列槽包含当前框的项；无则取列槽最接近的。
       →←：同一行内左右移一格（按该行格子序）。
   这样避免跨行时把 1×2 跨度项的中心点当落点（坐标距离的坑），
   而是按"它占哪几格"来对齐 —— 两张卡各自分列，永不横穿。
   -------------------------------------------------------------------------- */

const ROW_TOLERANCE = 8;
/** 网格列槽数：首页两卡 × 2 列 = 4。 */
const COL_COUNT = 4;

interface Grid {
  rows: HTMLElement[][];
  slot: Map<HTMLElement, { lo: number; hi: number }>;
}

/** 重建网格。每次按键重算（导航低频，不缓存）。 */
function buildGrid(): Grid {
  const items = Array.from(document.querySelectorAll<HTMLElement>('[data-focus-item]'))
    .filter(
      (el) => isFocusable(el) && !el.closest('[data-focus-zone="sidebar"]'),
    )
    .sort((a, b) => {
      const ra = a.getBoundingClientRect();
      const rb = b.getBoundingClientRect();
      return ra.top - rb.top || ra.left - rb.left;
    });

  const rows: HTMLElement[][] = [];
  for (const el of items) {
    const top = el.getBoundingClientRect().top;
    const cur = rows[rows.length - 1];
    if (cur && Math.abs(top - cur[0].getBoundingClientRect().top) <= ROW_TOLERANCE) {
      cur.push(el);
    } else {
      rows.push([el]);
    }
  }

  // 行内按 left 排序，并把每个项映射到「它覆盖的列槽范围 [lo,hi]」。
  const slot = new Map<HTMLElement, { lo: number; hi: number }>();
  rows.forEach((row) => {
    row.sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left);
    const n = row.length;
    row.forEach((el, i) => {
      // 第 i 个均匀占 [i*C/n, (i+1)*C/n - 1]，取整边缘。
      const lo = Math.floor((i * COL_COUNT) / n);
      const hi = Math.ceil(((i + 1) * COL_COUNT) / n) - 1;
      slot.set(el, { lo, hi });
    });
  });
  return { rows, slot };
}

/** ↑↓ 内容区：相邻行走一站，列保持「列槽位」（列记忆）。
 *  记忆 = 上次落下/从中出发的那一列槽（跨每次按键持久），用来看看在跨 1×2
 *   束身上也只认那一格，↑ 才能回到原来的那格（右侧格 → 右侧格）。
 */
let memSlot: number | null = null; // 最近一次稳定停在的列槽位（0..COL_COUNT-1）
let memEl: HTMLElement | null = null; // 记忆所属的焦点控件

export function stepUpDown(dir: 1 | -1 = 1): boolean {
  const g = buildGrid();
  if (g.rows.length === 0) return false;
  const active = document.activeElement as HTMLElement | null;
  if (!active || !g.slot.has(active)) {
    memSlot = null;
    memEl = null;
    const first = g.rows[0][0];
    const last = g.rows[g.rows.length - 1][g.rows[g.rows.length - 1].length - 1];
    focusItem(dir === 1 ? first : last);
    if (dir === 1) memSlot = 0;
    else memSlot = g.slot.get(last)!.hi;
    memEl = dir === 1 ? first : last;
    return true;
  }
  // 焦点换了（点击/ Tab 换到别的单元）→ 丢弃旧记忆，按当前格重新锚定。
  if (memEl !== active) {
    const s = g.slot.get(active)!;
    memSlot = (s.lo + s.hi) >> 1;
    memEl = active;
  }
  const anchor = memSlot!;
  const curRow = g.rows.findIndex((r) => r.includes(active));
  if (curRow < 0) return false;
  const targetRow = g.rows[curRow + dir];
  if (!targetRow) return false;

  // 相邻行里选「列槽包含锚点槽」的项；没有则取列槽与锚点最近的一个。
  const best = pickBestSlotNearest(targetRow, anchor, (el) => g.slot.get(el));
  memSlot = bestSlot(g.slot.get(best)!, anchor);
  memEl = best;
  focusItem(best);
  return true;
}

/** 在行内挑「列槽包含 slot」的项；否则取列槽中点与 slot 最近的那个。 */
function pickBestSlotNearest(
  row: HTMLElement[],
  slot: number,
  getSlot: (el: HTMLElement) => { lo: number; hi: number } | undefined,
): HTMLElement {
  let best = row[0];
  let bestDelta = Infinity;
  const anchor = slot;
  for (const el of row) {
    const s = getSlot(el);
    if (!s) continue;
    if (s.lo <= anchor && anchor <= s.hi) {
      best = el;
      break;
    }
    const mid = (s.lo + s.hi) / 2;
    const d = Math.abs(mid - anchor);
    if (d < bestDelta) {
      bestDelta = d;
      best = el;
    }
  }
  return best;
}

function bestSlot(s: { lo: number; hi: number }, anchor: number): number {
  if (s.lo <= anchor && anchor <= s.hi) return anchor;
  if (s.hi < anchor) return s.hi;
  return s.lo;
}

/** → 内容区：同一行右移一格。已在最右 → false。 */
export function stepColumnRight(): boolean {
  const g = buildGrid();
  const active = document.activeElement as HTMLElement | null;
  if (!active) return false;
  const curRow = g.rows.findIndex((r) => r.includes(active));
  if (curRow < 0) return false;
  const row = g.rows[curRow];
  const col = row.indexOf(active);
  if (col < 0 || col + 1 >= row.length) return false;
  focusItem(row[col + 1]);
  return true;
}

/** ← 内容区：同一行左移一格。已是最左 → false（调用方据此回侧栏）。 */
export function stepColumnLeft(): boolean {
  const g = buildGrid();
  const active = document.activeElement as HTMLElement | null;
  if (!active) return false;
  const curRow = g.rows.findIndex((r) => r.includes(active));
  if (curRow < 0) return false;
  const row = g.rows[curRow];
  const col = row.indexOf(active);
  if (col <= 0) return false;
  focusItem(row[col - 1]);
  return true;
}

/** 进入内容区第一个控件（侧栏按 → 的落点）。 */
export function enterContent(): boolean {
  const g = buildGrid();
  if (g.rows.length === 0) return false;
  focusItem(g.rows[0][0]);
  return true;
}

/** 焦点是否已丢出网格。失格 = 第一下方向键从「当前激活页图标」出发直接导航。
 *  点空白、切别的应用又点回来，浏览器常把焦点清成 body/html；点回窗口空白
 *  还可能落在 SVG <g> / 普通容器上 —— 这些一律视为丢出网格。
 *  Tab 把焦点送到红绿灯也视作丢出网格 —— 方向键和 Tab 互不相干。 */
export function isLostFocus(): boolean {
  const el = document.activeElement as HTMLElement | null;
  if (!el || el === document.body || el === document.documentElement) return true;
  if (el.closest('.window-traffic')) return true;
  return el.closest('[data-focus-item]') === null;
}

/** 有模态框打开时，导航让给模态框内部焦点陷阱。 */
export function isModalOpen(): boolean {
  return (
    document.querySelector('[role="dialog"][aria-modal="true"]') !== null ||
    document.querySelector('dialog[open]') !== null
  );
}

/** 焦点是否在侧栏内。Esc 要「退回 App cli」，需要先知道在不在。 */
export function isInSidebar(): boolean {
  const el = document.activeElement as HTMLElement | null;
  return !!el?.closest('[data-focus-zone="sidebar"]');
}

/** 把焦点送回 App 当前激活的页项（Esc / ← 的落点）。 */
export function focusSidebar() {
  const activeNav = document.querySelector<HTMLElement>(
    '[data-focus-zone="sidebar"] [data-focus-item].active',
  );
  const first = document.querySelector<HTMLElement>(
    '[data-focus-zone="sidebar"] [data-focus-item]',
  );
  focusItem(activeNav ?? first);
}

/* ==========================================================================
   侧栏整条移动（被 App 的 document keydown 调用）。 */
export function stepSidebarBy(current: string, onSelect: (page: string) => void, delta: number): void {
  const items = Array.from(
    document.querySelectorAll<HTMLButtonElement>(
      '[data-focus-zone="sidebar"] .nav-item',
    ),
  );
  if (items.length === 0) return;
  const focused = (document.activeElement as HTMLElement | null)?.closest('.nav-item');
  let i = focused ? items.indexOf(focused as HTMLButtonElement) : -1;
  if (i < 0) i = items.findIndex((el) => el.dataset.page === current);
  if (i < 0) i = 0;
  const next = items[(i + delta + items.length) % items.length];
  next.focus({ preventScroll: true });
  const page = next.dataset.page;
  if (page) onSelect(page);
}

/* ==========================================================================
   文本编辑触发模型。
   规则：
   - 文本输入框聚焦但「没打开」（没输入过首字符、没点进去）时，它只是网格里
     一个普通停靠点 —— 方向键照走。
   - 输入第一个字符 / 点进去之后 = 打开（编辑态），编辑优先：方向键归光标。
   - Enter = 确认并退出编辑；Esc 第一下退编辑，第二下回 App。
   - 判定用「用户动作」而不是「有没有内容」：预填的输入框里有字，但没
     输入过，就不算打开。
   ========================================================================== */

const editingFields = new WeakSet<HTMLElement>();
const EDITING_CLASS = 'nav-editorFocus';

/** 是不是文本输入元素（INPUT / TEXTAREA）。SELECT 不算 —— 那是选择控件。 */
export function isTextField(el: Element | null | undefined): boolean {
  if (!el) return false;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA';
}

/** 是否处于「打开（编辑）」态。 */
export function isEditingText(el: Element | null | undefined): boolean {
  return !!el && editingFields.has(el as HTMLElement);
}

/** 进入 / 退出编辑态。同时切 .nav-editor 视觉分辨。 */
export function setEditingText(el: Element | null | undefined, editing: boolean): void {
  if (!isTextField(el)) return;
  const node = el as HTMLElement;
  if (editing) {
    editingFields.add(node);
    node.classList.add(EDITING_CLASS);
  } else {
    editingFields.delete(node);
    node.classList.remove(EDITING_CLASS);
  }
}

/* 红绿灯焦点环自动消失的定时器。Tab 过去后若一直没操作，环会自己撤掉。 */
let trafficBlurTimer: number | undefined;

/** Tab 只循环窗口红绿灯（关闭/最小化/最大化），不做普通跳转。 */
export function cycleTrafficLights(dir: 1 | -1 = 1): void {
  const btns = Array.from(document.querySelectorAll<HTMLButtonElement>('.window-traffic button'));
  if (btns.length === 0) return;
  const active = document.activeElement as HTMLElement | null;
  const i = active ? btns.indexOf(active as HTMLButtonElement) : -1;
  const next = i < 0 ? (dir === 1 ? 0 : btns.length - 1) : (i + dir + btns.length) % btns.length;
  btns[next].focus({ preventScroll: true });
  window.clearTimeout(trafficBlurTimer);
  trafficBlurTimer = window.setTimeout(() => {
    const el = document.activeElement as HTMLElement | null;
    if (el?.closest('.window-traffic')) el.blur();
  }, 1600);
}

/** 方向键是否已被「打开状态下维护自身键盘」的控件（如展开的下拉）提前消费。
 *  focusGrid 在 document 层判，遇到展开的自绘下拉应放行，让下拉自己处理 ↑↓。 */
export function isDropdownOpenAt(el: Element | null): boolean {
  return !!el?.closest('.dropdown')?.querySelector('.dropdown-options.show');
}