import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { LanguageProvider } from "./i18n/context";
import { StoreProvider, useStore } from "./store";
import { ToastProvider } from "./ui/toast";

/* 顺序要紧：fonts 先注册 @font-face，tokens 落变量，shell/components 都依赖变量。 */
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/shell.css";
import "./styles/components.css";
import "./styles/subtitle.css";

const host = document.getElementById("root");
if (!host) throw new Error("找不到 #root，index.html 被改坏了。");

/** 从 store 取持久化的界面语言，注入 LanguageProvider（语言驱动源）。 */
function LangBridge({ children }: { children: React.ReactNode }) {
  const { settings } = useStore();
  return <LanguageProvider uiLang={settings.ui_language}>{children}</LanguageProvider>;
}

createRoot(host).render(
  <StrictMode>
    <StoreProvider>
      <LangBridge>
        <ToastProvider>
          <App />
        </ToastProvider>
      </LangBridge>
    </StoreProvider>
  </StrictMode>,
);
