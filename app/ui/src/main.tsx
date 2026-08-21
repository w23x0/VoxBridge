import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { StoreProvider } from "./store";
import { ToastProvider } from "./ui/toast";

/* 顺序要紧：fonts 先注册 @font-face，tokens 落变量，shell/components 都依赖变量。 */
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/shell.css";
import "./styles/components.css";
import "./styles/subtitle.css";

const host = document.getElementById("root");
if (!host) throw new Error("找不到 #root，index.html 被改坏了。");

createRoot(host).render(
  <StrictMode>
    <StoreProvider>
      <ToastProvider>
        <App />
      </ToastProvider>
    </StoreProvider>
  </StrictMode>,
);
