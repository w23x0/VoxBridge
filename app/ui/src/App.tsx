/**
 * 外壳：38px 标题栏（只有居中页面名）+ 64px 浮空侧栏 + app-layout 主内容。
 * 结构照 kirox-ui skill 的 references/shell.md：没有顶部工具条，工具项在侧栏底部。
 *
 * 六个页面全在 DOM 里，靠 .page.active 切显示，不做入场动画 ——
 * skill 点名：切页时元素位移会被误认成闪烁。
 */

import { useEffect, useRef, useState } from "react";

import { ABOUT_NAV, DEFAULT_PAGE, PAGE_NAV } from "./nav";
import { AboutPage } from "./sections/About";
import { ProvidersPage } from "./sections/Aliyun";
import { HomePage } from "./sections/Home";
import { SettingsPage } from "./sections/Settings";
import { SubtitlePage } from "./sections/Subtitle";
import { UsagePage } from "./sections/Usage";
import { Sidebar } from "./shell/Sidebar";
import { Titlebar } from "./shell/Titlebar";
import { useStore } from "./store";
import { PIPELINE_LABEL } from "./pipeline";
import { restoreTheme } from "./ui/theme";
import { useToast } from "./ui/toast";

const PAGES: Record<string, () => React.ReactElement> = {
  home: HomePage,
  providers: ProvidersPage,
  subtitle: SubtitlePage,
  settings: SettingsPage,
  usage: UsagePage,
  about: AboutPage,
};

export function App() {
  const [page, setPage] = useState(DEFAULT_PAGE);
  useEffect(() => {
    restoreTheme();
  }, []);
  useNoticeToasts();

  return (
    <>
      <Titlebar />
      <Sidebar page={page} onNavigate={setPage} />
      <div className="app-layout">
        <div className="app-content">
          {PAGE_NAV.map((n) => {
            const Page = PAGES[n.id];
            if (!Page) return null;
            return (
              <section
                key={n.id}
                id={`page-${n.id}`}
                className={page === n.id ? "page active" : "page"}
                aria-labelledby={`title-${n.id}`}
              >
                <div className={n === ABOUT_NAV ? "page-scroll narrow" : "page-scroll"}>
                  <div className="page-head">
                    <h1 id={`title-${n.id}`}>{n.label}</h1>
                  </div>
                  <Page />
                </div>
              </section>
            );
          })}
        </div>
      </div>
    </>
  );
}

/**
 * 把后端的 notice 和通信错误转成 toast。
 *
 * notices 当队列用：弹过就摘掉，摘空为止。不用「处理到第几条」的游标 ——
 * store 里是 slice(-50)，到顶之后数组长度不再变，游标会永远停在 50。
 * 摘的时候固定传 0：函数式更新依次作用在上一次结果上，连调 N 次就是摘掉开头 N 条。
 */
function useNoticeToasts() {
  const store = useStore();
  const toast = useToast();
  const notices = store.snapshot?.notices;
  /** 记住处理过的那一批（比对数组身份），防止同一批弹两遍。 */
  const done = useRef<unknown>(null);
  const lastError = useRef<string | null>(null);
  const dismiss = useRef(store.dismissNotice);
  dismiss.current = store.dismissNotice;

  useEffect(() => {
    if (!notices || notices.length === 0) return;
    if (done.current === notices) return;
    done.current = notices;
    for (const n of notices) {
      // 普通 info 是后台状态同步；带流水线的 info 是“运行中需重启”提醒，不能吞掉。
      if (n.severity !== "info" || n.pipeline) {
        const who = n.pipeline ? `${PIPELINE_LABEL[n.pipeline]}：` : "";
        toast(n.severity === "error" ? "danger" : "warning", `${who}${n.text}`);
      }
      dismiss.current(0);
    }
  }, [notices, toast]);

  useEffect(() => {
    const err = store.error;
    if (err && err !== lastError.current) toast("danger", `连接后端失败：${err}`);
    lastError.current = err;
  }, [store.error, toast]);
}
