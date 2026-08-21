# 服务商能力表维护说明

VoxBridge 只做实时语音翻译。当前启用两个服务商，各固定一个专用模型：

- 阿里云百炼：`qwen3.5-livetranslate-flash-realtime`
- Google Gemini：`gemini-3.5-live-translate-preview`

模型、语言、音色、API 元数据的唯一维护入口是：

`catalog/aliyun.json` 和 `catalog/gemini.json`

前端直接导入两份 JSON；`vox-core` 的构建脚本会读取并校验同一数据，再生成 Rust 常量。
不要在 TypeScript 或 Rust 文件里另抄一份模型、语言或音色表。

## 当前目录包含什么

- 官方推荐的稳定模型 ID 与当前稳定快照。
- WebSocket 旧公共地址和分地域业务空间地址模板。
- 60 种可识别/可翻译语言。
- 其中 29 种支持“音频+文本”输出，31 种仅支持文本。
- Qwen3.5 LiveTranslate 当前官方音色及中文说明。
- 已移除旧音色 ID，仅用于把老用户配置安全迁移到 Tina，不会出现在界面选项里。
- 官方文档地址与最后核对日期。
- Gemini Live Translation 的模型、WebSocket、音频规格和能力摘要。

## 更新流程

1. 先看对应服务商的模型上下架公告。
2. 核对实时翻译、客户端事件、服务端事件、音频格式和限流说明。
3. 修改对应的 `catalog/*.json`，同时更新 `verified_at`。
4. 运行 `cargo test --workspace` 和 `npm run verify`。
5. 用真实 API Key 做一次中文→外语、外语→中文的双线路冒烟测试。
6. 提升应用版本，发布经过签名的 GitHub Release。

构建脚本会强制检查语言、语音输出语言和音色数组的数量必须与 JSON 中的 `expected_counts` 一致，默认语言和默认音色必须存在，并拒绝重复代码。官方数量变化时，数据和期望数量在同一个 JSON 里一起更新。

Gemini 的公开限流值可能随项目层级变化，不写进目录。实际 RPM/TPM/RPD 以 AI Studio 的项目配额页为准；同一 Google Cloud 项目下的多个 API Key 共享配额。

## 官方资料

- 实时翻译总览：https://help.aliyun.com/zh/model-studio/qwen3-5-livetranslate-flash-realtime
- 客户端事件：https://help.aliyun.com/zh/model-studio/live-translator-client-events
- 服务端事件：https://help.aliyun.com/zh/model-studio/live-translator-server-events
- 音色列表：https://help.aliyun.com/zh/model-studio/omni-voice-list
- 模型上下架与更新：https://help.aliyun.com/zh/model-studio/newly-released-models
- Gemini Live Translation：https://ai.google.dev/gemini-api/docs/live-api/live-translate
- Gemini 限流：https://ai.google.dev/gemini-api/docs/rate-limits

## 软件更新入口

“关于 → 检查更新”目前是诚实的占位入口，不会连接未知地址。GitHub 仓库建立后，再接入 Tauri Updater、签名公钥、GitHub Actions 和 Releases 更新清单；签名私钥只能放在 GitHub Actions Secrets，不能提交到仓库。
