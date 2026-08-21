/** 虚拟麦克风管理区：安装状态、二次确认卸载、重新安装、16 声道隐藏与恢复。 */
import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { chromium } from "playwright";

const PORT = 5187;
const server = spawn(
  process.platform === "win32" ? "npx.cmd" : "npx",
  ["vite", "preview", "--port", String(PORT), "--strictPort", "--host", "127.0.0.1"],
  { stdio: "ignore", shell: process.platform === "win32" },
);

const portOpen = () =>
  new Promise((resolve) => {
    const socket = createConnection({ port: PORT, host: "127.0.0.1" });
    socket.on("connect", () => (socket.end(), resolve(true)));
    socket.on("error", () => resolve(false));
    setTimeout(() => (socket.destroy(), resolve(false)), 800);
  });

/** 和 a11y.mjs 一样挨个试内置 / Chrome / Edge。 */
async function launch() {
  for (const channel of ["chrome", "msedge", undefined]) {
    try {
      return await chromium.launch(channel ? { channel } : {});
    } catch {
      /* 换下一个 */
    }
  }
  throw new Error("没有可用浏览器。");
}

let browser;
try {
  for (let i = 0; i < 60 && !(await portOpen()); i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  browser = await launch();
  const page = await browser.newPage({ viewport: { width: 1040, height: 780 } });
  await page.goto(`http://127.0.0.1:${PORT}/?mock=1`, { waitUntil: "networkidle" });
  await page.click('.sidebar .nav-item[data-page="settings"]');

  const panel = page.locator(".settings-item").filter({ hasText: "虚拟麦克风" });
  await panel.getByText("已安装", { exact: true }).waitFor();

  // 卸载走二次确认：假数据里 Discord 在占用，得先报出来再关
  await panel.getByRole("button", { name: "卸载", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "卸载虚拟麦克风" });
  await dialog.getByText("Discord", { exact: true }).waitFor();
  await dialog.getByRole("button", { name: "关闭应用并卸载" }).click();
  await panel.getByText("未安装", { exact: true }).waitFor();

  await panel.getByRole("button", { name: "安装", exact: true }).click();
  await panel.getByText("已安装", { exact: true }).waitFor();

  const channel = page.locator(".settings-item").filter({ hasText: "多声道设备" });
  await channel.getByText("已隐藏", { exact: true }).waitFor();
  await channel.getByRole("button", { name: "显示", exact: true }).click();
  await channel.getByText("已显示", { exact: true }).waitFor();
  await channel.getByRole("button", { name: "隐藏", exact: true }).click();
  await channel.getByText("已隐藏", { exact: true }).waitFor();
  console.log("虚拟麦克风管理区：安装、卸载、多声道隐藏与恢复全部通过。");
} finally {
  await browser?.close();
  server.kill();
}
