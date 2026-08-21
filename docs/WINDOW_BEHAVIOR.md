# 窗口边缘 · 两项未决问题（真实状态追踪）

> 本文件记录两个互相纠缠、且**尚未在真机确认解决**的窗口问题：
> ① 大圆角没生效  ② 最小高度锁不住 38。
> 口径与 `DECISIONS.md` 一致：**代码与这里打架时以代码为准**；这里只记"当前代码长什么样"和"真机观测到的事实"，**不写结论性"已修复"**，因为两件事都还没经真窗拖拽验证。

---

## 现状一句话

- 用户真机观测（2026-08-21）：**窗口没显示成 12px 大圆角，而是"默认的小弧度"**。
- 推论（待验证）：黑角月牙缝之所以"现在没了"，**很可能不是因为它被修好了，而是因为大圆角根本没有渲染** → 圆角弧外没有大的月牙区可漏黑。**"大圆角没上"才是真正的问题**，黑角是它的次生表现。
- 因此：**当前代码里 `#root` 的 12px 圆角（`tokens.css`）在真机大概率没有真正作用在窗口上**。

---

## 文件与改动现状（工作区当前状态，截止 2026-08-21）

| 文件 | 内容 |
| --- | --- |
| `app/ui/src/styles/tokens.css:142-149` | `#root { height/width:100%; background:var(--bg)(135°渐变); border-radius:var(--window-radius)=12px; overflow:hidden; transform:translateZ(0) }` — **设计上要 12px 大圆角** |
| `app/ui/index.html:23-31` | `html,body { background:var(--bg); overflow:hidden }` — **已铺渐变底（首帧防闪注释）**，圆角外不再透黑 |
| `app/src-tauri/src/winminmax.rs` 全文 | 最底层 `WM_GETMINMAXINFO` 强制 `ptMinTrackSize=min(640,38)×dpi`；**已加 `WM_WINDOWPOSCHANGING` 分支**（`wp.flags!SWP_NOSIZE` 时钳 `cx/cy`） |
| `app/src-tauri/src/lib.rs:148-150` | `enforce_min_size(hwnd.0, 640, 38)` |
| `app/src-tauri/tauri.conf.json:26-27` | `minWidth:640, minHeight:38`, `decorations:false, transparent:true` |

**关键**：另一个会话已经加了两个"假设性修复"（html/body 铺底 + WMPOSCHANGING 兜底）并跑过 `npm run verify` / `cargo build`，但这些**只是代码改动，不等同于问题解决**。

---

## 问题 ①：大圆角在真机没生效 —— 未解决，待实测

### 症状/观测
- 真机窗口显示默认弧度，不是 12px 大圆角。

### 代码意图
- `tokens.css`：#root 圆角 12px + `overflow:hidden`，配合 `transform:translateZ(0)` 让 fixed 子元素也被圆角裁剪。

### 可能原因（未验证，按可疑度排序）
1. **`#root` 的高度/宽度没撑满窗口** 或 `#root` 不是直接 100%，圆角没切到窗口边缘。
2. **`background:var(--bg)` 在 html/body 上被合并到 vision/画布背景**，导致真正的窗口底是 html 的底色而非 #root 的圆角渐变 → 圆角渲染错位。
3. **透明度合成**：`transparent:true` + 圆角，WebView/窗口合成器把圆角外的底层填成默认，结果窗口四角显现的是 OS 的默认弧度（非 0）——但这与"默认弧度"现象不完全吻合，需区分。

### 需要的验证动作（真机，无法 automate）
- 起 `tauri dev`，看正常高度窗口四角：**到底有没有 12px 大圆角？**
- 若没有：在 DevTools 里查 `#root` 的 computed `border-radius` 与 `getBoundingClientRect()` 的宽高是否等于窗口尺寸；是否被 html/body 的 `var(--bg)` 吞掉圆角。
- 截图留证。

---

## 问题 ②：最小高度锁不到 38 —— 待确认

### 现状
- `winminmax.rs` 已有 `WM_GETMINMAXINFO` + `WM_WINDOWPOSCHANGING` 两层钳制，均换算 DPI 后逼 `ptMin`/`WINDOWPOS`。
- 结论：不改动 Rust 的前提下，是否真锁到 38 **必须真机拖**。

### 之前子代理审查结论（无 bug，缺口在"路径"）
- 文件内逻辑无 bug（ORIG 存旧指针、GA_ROOT、DPI 换算、先 orig 后覆盖全对）。
- 真正风险：**被压到 30px 的那次 resize 是走的"哪棵 HWND / 哪条消息"没有确认**——可能是 overlay 悬浮窗（lib.rs:182 自建的另一个 Win32 窗），与主窗钳制无关。

### 需要的验证动作
- 起 `tauri dev`，把主窗拖到最矮：**量边框高度，是否真的停在 38**。
- 若仍压到 30：给 `wndproc` 加日志/断点，确认 `WM_GETMINMAXINFO` / `WM_WINDOWPOSCHANGING` 进没进、进的是哪个 HWND（判断是不是跑了 overlay 那个窗）。

---

## 待办/下一步（都要求真机）

1. **[优先] 确认① 大圆角为何没渲染**：真机看圆角 + DevTools 查 #root computed 尺寸/圆角。这是"黑角月牙缝"的**上游**——圆角生效了，月牙缝的治理才有意义。
2. **[确认] ② 最小高度**：真机把主窗压到最矮，量到几；压不破即成功，压破再补 HWND 探针。
3. 两条**都只有真机测试能定性**，`npm run verify`（纯前端断言）测不到窗口边框/圆角/最小尺寸，不能当验证。

## 参考
- 骨架相关：`docs/DECISIONS.md` 第 13 条（UI 对齐 glassui）。
- 涉及文件：`app/ui/src/styles/{tokens,shell}.css`、`app/ui/index.html`、`app/src-tauri/src/{winminmax.rs,lib.rs,tauri.conf.json}`。