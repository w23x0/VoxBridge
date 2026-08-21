# VoxBridge

面向实时对话的 Windows 桌面实时语音翻译器。两条独立流水线同时运行：

| 流水线 | 输入 | 输出 |
| --- | --- | --- |
| **对外说话（Speak out）** | 你的麦克风 | 译音送入虚拟麦克风 + 实时字幕 |
| **听人说话（Listen in）** | 指定程序的音频 | 中文语音送耳机 + 实时字幕 |

基于 **Tauri 2 + React 19 + Rust**。界面负责配置与状态；音频采集、降噪、重采样、WebSocket 传输、快捷键与悬浮字幕窗口都落在 Rust/Win32 里。

English：[`../README.md`](../README.md) · 日本語：[`ja.md`](ja.md) · 한국어：[`ko.md`](ko.md) · Español：[`es.md`](es.md) · Français：[`fr.md`](fr.md) · Deutsch：[`de.md`](de.md)

## 适用范围

- 仅 Windows；「听人说话」的进程环回需 **Win11 / Server 2022（build 20348+）**。
- 服务商——**阿里云百炼**、**Google Gemini**、**OpenAI Realtime**——每条流水线单独选择，各固定一个实时翻译模型。
- 「听人说话」固定**翻成中文**；源语言自动检测或手动指定。
- 界面语言：简体中文 / 日本語 / English，与翻译语言相互独立。
- 服务商 API Key 本地存储，按用户用 **Windows DPAPI** 加密。
- 服务商元数据在 [`catalog/*.json`](../catalog/)，不进源码。

## 开发

前置：Windows 11 x64、Node.js `^20.19.0` 或 `>=22.12.0`、Rust stable（`x86_64-pc-windows-msvc`）、VS Build Tools（C++ 桌面）、WebView2，以及 API key（或前端 Mock，可不申请 Key）。

```powershell
cd app\ui
npm ci
npm run tauri:dev        # 完整桌面应用
npm run dev              # 仅 UI；打开 http://127.0.0.1:5183/?mock=1
```

## 构建安装包

```powershell
cd app\ui
npm run tauri:build      # 产物在 target/release/bundle/nsis/
```

发布别用 `cargo build --release`；若要手动构建二进制，需带上 custom-protocol 特性：

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## 测试

```powershell
cargo test --workspace   # Rust，仓库根目录
npm run verify           # app/ui 内：类型检查、生产构建、a11y/disabled 检查
```

## 目录结构

```text
VoxBridge/
├─ catalog/            # 服务商元数据：aliyun.json、gemini.json、gpt.json
├─ crates/
│  ├─ vox-core/        # 平台无关核心：设置、协议、状态机、用量
│  ├─ vox-net/         # WebSocket 传输
│  ├─ vox-dsp/         # RNNoise 降噪 + 重采样
│  ├─ vox-audio-win/   # WASAPI 采集/播放、进程回环、VB-CABLE
│  ├─ vox-input-win/   # 全局快捷键
│  └─ vox-overlay-win/ # Win32 透明字幕窗口
├─ app/
│  ├─ src-tauri/       # Tauri 层：命令、托盘、持久化、DPAPI
│  └─ ui/              # React 设置界面 + 浏览器 Mock
├─ docs/               # 架构、决策、协议、catalog
└─ README.md
```

## 数据流

**Speak：** `mic → mono → RNNoise → gate → 16 kHz PCM → provider →（字幕 + 24 kHz 语音送 VB-CABLE）`

**Listen：** `loopback → mono → 16 kHz PCM → provider →（中文字幕 + 24 kHz 语音送耳机）`

两条流水线独立运行；`vox-core::Runtime` 是唯一真源。

## Contributing

VoxBridge 刻意保持保守：核心范围固定，**外挂模块**是开放的扩展点——详见 **[`../CONTRIBUTING.md`](../CONTRIBUTING.md)**，包括计划中的 Discord 独立进程模块（`docs/DISCORD_PROTOCOL.md`，二期，尚未拍板）。

## Docs

`docs/ARCHITECTURE.md`、`docs/DECISIONS.md`、`docs/QWEN_PROTOCOL.md`、`docs/GEMINI_PROTOCOL.md`、`docs/PROVIDER_CATALOG.md`、`docs/DISCORD_PROTOCOL.md`。

## License

MIT