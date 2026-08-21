import { chromium } from "playwright";

const baseUrl = process.env.VOXBRIDGE_UI_URL ?? "http://127.0.0.1:5188/?mock=1";
const browser = await chromium.launch();
const failures = [];
const browserErrors = [];
const secretSentinel = "VB_TEST_SECRET_9f41c0d2";

function watch(page, label) {
  page.on("console", (message) => {
    if (message.type() === "error" || message.type() === "warning") {
      browserErrors.push(`${label} console ${message.type()}: ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => browserErrors.push(`${label} pageerror: ${error.message}`));
}

async function selectOption(page, trigger, option) {
  await page.locator(trigger).click();
  await page.locator(".dropdown-options.show").getByRole("option", { name: option, exact: true }).click();
}

async function expectText(locator, expected, message) {
  const actual = (await locator.innerText()).trim();
  if (actual !== expected) failures.push(`${message}：期望“${expected}”，实际“${actual}”`);
}

async function expectCount(locator, expected, message) {
  const actual = await locator.count();
  if (actual !== expected) failures.push(`${message}：期望 ${expected}，实际 ${actual}`);
}

async function inspectVisibleLayout(page, label) {
  const issues = await page.locator(".page.active").evaluate((activePage) => {
    const output = [];
    const root = document.documentElement;
    if (root.scrollWidth > root.clientWidth + 1) {
      output.push(`document 横向溢出 ${root.scrollWidth - root.clientWidth}px`);
    }
    const scroll = activePage.querySelector(".page-scroll");
    if (scroll && scroll.scrollWidth > scroll.clientWidth + 1) {
      output.push(`页面横向溢出 ${scroll.scrollWidth - scroll.clientWidth}px`);
    }
    const candidates = activePage.querySelectorAll("button, input, .badge, .chip");
    for (const element of candidates) {
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      if (style.display === "none" || style.visibility === "hidden" || rect.width === 0) continue;
      if (element.scrollWidth > element.clientWidth + 2 && style.overflowX === "visible") {
        output.push(`${element.tagName.toLowerCase()} 文本溢出：${element.textContent?.trim() ?? ""}`);
      }
    }
    return output;
  });
  for (const issue of issues) failures.push(`${label}：${issue}`);
}

async function navigate(page, label) {
  await page.getByRole("button", { name: label, exact: true }).click();
  await page.getByRole("heading", { name: label, exact: true }).waitFor();
}

try {
  const cold = await browser.newPage({ viewport: { width: 1180, height: 820 } });
  watch(cold, "cold");
  await cold.goto(`${baseUrl}&cold=1`, { waitUntil: "networkidle" });
  await cold.waitForSelector(".stats-row.cols-2 > .stat-card");

  const coldCards = cold.locator(".stats-row.cols-2 > .stat-card");
  await expectCount(coldCards, 2, "首页应正好有两条流水线");
  await expectCount(
    cold.getByText("请先配置 阿里云百炼 API 密钥", { exact: true }),
    2,
    "无 Key 时两条流水线都应显示明确配置提示",
  );
  const coldStartButtons = cold.getByRole("button", { name: "启动", exact: true });
  if (
    (await coldStartButtons.count()) !== 2 ||
    !(await coldStartButtons.nth(0).isDisabled()) ||
    !(await coldStartButtons.nth(1).isDisabled())
  ) {
    failures.push("无 Key 时启动按钮必须禁用");
  }

  await navigate(cold, "模型服务商");
  const keyInput = cold.locator("#f-api-key");
  if ((await keyInput.getAttribute("type")) !== "password") failures.push("API Key 输入框不是 password");
  if (!(await cold.getByRole("button", { name: "保存", exact: true }).isDisabled())) {
    failures.push("空 Key 时保存按钮没有禁用");
  }

  await keyInput.fill(secretSentinel);
  const leakedBeforeSave = await cold.evaluate((secret) => ({
    html: document.documentElement.outerHTML.includes(secret),
    text: document.body.innerText.includes(secret),
  }), secretSentinel);
  if (leakedBeforeSave.html || leakedBeforeSave.text) failures.push("Key 出现在 HTML 属性或页面文本中");

  await cold.getByRole("button", { name: "保存", exact: true }).dblclick();
  await cold.getByText("已配置", { exact: true }).waitFor();
  await cold.waitForTimeout(100);
  await expectCount(
    cold.getByText("Google Gemini 密钥已保存", { exact: true }),
    1,
    "重复点击保存不应提交两次",
  );
  if ((await keyInput.inputValue()) !== "") failures.push("保存后 Key 输入框没有清空");

  await selectOption(cold, "#dd-provider-config", "阿里云百炼");
  await cold.getByText("未配置", { exact: true }).waitFor();
  await selectOption(cold, "#dd-provider-config", "Google Gemini");
  await cold.getByText("已配置", { exact: true }).waitFor();

  await cold.getByRole("button", { name: "清除密钥", exact: true }).click();
  await cold.getByRole("button", { name: "确认清除", exact: true }).click();
  await cold.getByText("未配置", { exact: true }).waitFor();
  if (await cold.getByRole("button", { name: "清除密钥", exact: true }).isVisible().catch(() => false)) {
    failures.push("清除 Gemini Key 后清除按钮仍然可见");
  }
  if (browserErrors.some((message) => message.includes(secretSentinel))) {
    failures.push("Key 出现在浏览器控制台中");
  }
  await cold.close();

  const page = await browser.newPage({ viewport: { width: 1180, height: 820 } });
  watch(page, "warm");
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.waitForSelector(".stats-row.cols-2 > .stat-card");

  await selectOption(page, "#dd-home-speak-provider", "Google Gemini");
  await expectText(page.locator("#dd-home-speak-voice"), "Gemini 自动音色", "Gemini 对外说话音色");
  if (!(await page.locator("#dd-home-speak-voice").isDisabled())) failures.push("Gemini 对外说话音色仍可手动选择");
  await page.getByText(/重启「对外说话」后生效/).waitFor({ timeout: 1500 }).catch(() => {
    failures.push("运行中切换对外说话服务商后没有重启提示");
  });

  await selectOption(page, "#dd-home-listen-provider", "Google Gemini");
  await expectText(page.locator("#dd-home-listen-source"), "自动识别", "Gemini 听人说话源语言");
  if (!(await page.locator("#dd-home-listen-source").isDisabled())) failures.push("Gemini 源语言仍可手动选择");
  await expectText(page.locator("#dd-home-listen-voice"), "Gemini 自动音色", "Gemini 听人说话音色");
  if (!(await page.locator("#dd-home-listen-voice").isDisabled())) failures.push("Gemini 听人说话音色仍可手动选择");
  await page.getByText(/重启「听人说话」后生效/).waitFor({ timeout: 1500 }).catch(() => {
    failures.push("运行中切换听人说话服务商后没有重启提示");
  });

  const restartCountBeforeLanguage = await page.locator("#toast-container .toast-text").count();
  await selectOption(page, "#dd-home-target-language", "英语");
  await page.waitForFunction(
    ({ selector, before }) => document.querySelectorAll(selector).length > before,
    { selector: "#toast-container .toast-text", before: restartCountBeforeLanguage },
    { timeout: 1500 },
  ).catch(() => {
    failures.push("Gemini 运行中切换目标语言后没有重启提示");
  });
  const listenAudioToggle = page.getByRole("switch", { name: "播放译音", exact: true });
  await listenAudioToggle.click();
  if ((await listenAudioToggle.getAttribute("aria-checked")) !== "false") failures.push("播放译音开关关闭后状态不清楚");
  await listenAudioToggle.click();
  if ((await listenAudioToggle.getAttribute("aria-checked")) !== "true") failures.push("播放译音开关开启后状态不清楚");

  const catalogCheck = await page.evaluate(async () => {
    const catalog = await import("/src/catalog.ts");
    return catalog.defaultModelForProvider("gemini");
  });
  if (catalogCheck !== "gemini-3.5-live-translate-preview") {
    failures.push(`Gemini 默认模型错误：${catalogCheck}`);
  }

  await inspectVisibleLayout(page, "首页 1180x820 亮色");
  await navigate(page, "模型服务商");
  await expectText(page.locator("#dd-provider-config"), "Google Gemini", "服务商页默认选项");
  await expectCount(page.getByText("gemini-3.5-live-translate-preview", { exact: true }), 1, "Gemini 模型能力");
  await inspectVisibleLayout(page, "模型服务商 1180x820 亮色");
  await page.locator("#toast-container .toast-item").first().waitFor({ state: "detached", timeout: 5000 }).catch(() => {});
  await page.screenshot({ path: "../../target/ui-qa-gemini-provider-1180x820-light.png" });

  await navigate(page, "设置");
  await inspectVisibleLayout(page, "设置 1180x820 亮色");
  await navigate(page, "关于");
  await inspectVisibleLayout(page, "关于 1180x820 亮色");

  await page.getByRole("button", { name: "切到暗色主题", exact: true }).click();
  for (const label of ["首页", "模型服务商", "设置", "关于"]) {
    await navigate(page, label);
    await inspectVisibleLayout(page, `${label} 1180x820 暗色`);
  }
  await page.screenshot({ path: "../../target/ui-qa-about-1180x820-dark.png" });

  await page.setViewportSize({ width: 900, height: 620 });
  for (const label of ["首页", "模型服务商", "设置", "关于"]) {
    await navigate(page, label);
    await inspectVisibleLayout(page, `${label} 900x620 暗色`);
  }
  await navigate(page, "首页");
  await page.screenshot({ path: "../../target/ui-qa-gemini-home-900x620-dark.png" });
  await navigate(page, "模型服务商");
  const saveButtonBox = await page.getByRole("button", { name: "保存", exact: true }).boundingBox();
  if (!saveButtonBox || saveButtonBox.height > 44) failures.push("900px 窄窗的保存按钮发生换行");
  await page.screenshot({ path: "../../target/ui-qa-gemini-provider-900x620-dark.png" });
  await page.close();

  failures.push(...browserErrors);
  if (failures.length > 0) {
    console.error(failures.join("\n"));
    process.exitCode = 1;
  } else {
    console.log("Gemini UI、密钥隔离、跨页面布局与窄窗口检查通过。");
  }
} finally {
  await browser.close();
}
