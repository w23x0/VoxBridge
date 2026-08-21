import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 的 dev server 约定：固定端口、失败即报错、不要试图换端口。
// 样式是纯 CSS（见 styles/spec.css），没有 Tailwind / 预处理器插件。
export default defineConfig({
  plugins: [react()],
  // Tauri 用 file:// 之外的自定义协议加载资源，相对路径最稳。
  base: "./",
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 5183,
    strictPort: true,
    host: "127.0.0.1",
    // 前后端共用仓库根目录 catalog/*.json，开发服务器要允许读取它。
    fs: {
      allow: [fileURLToPath(new URL("../..", import.meta.url))],
    },
  },
  build: {
    // Tauri v2 的 WebView2 跟得上现代语法，不必降级到 ES5。
    target: "chrome110",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
  },
});
