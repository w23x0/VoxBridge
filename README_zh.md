# VoxBridge

面向实时对话场景的 Windows 桌面语音翻译工具，把「说出去」和「听进来」拆成两条可以同时运行的流水线：

| 流水线 | 输入 | 输出 |
| --- | --- | --- |
| 对外说话 | 本机麦克风 | 外语译音送入虚拟麦克风，同时显示字幕 |
| 听人说话 | 指定 Windows 程序的声音 | 中文译音播放到耳机，同时显示字幕 |

技术栈：**Tauri 2 + React 19 + Rust**。界面负责配置与状态展示；音频采集、降噪、重采样、WebSocket 传输、热键和悬浮字幕由 Rust/Win32 实现。

## 功能范围

- 仅支持 Windows；「听人说话」的进程环回完整支持以 Windows 11 / Server 2022（build 20348 及以上）为准。
- 服务商：**阿里云百炼** 与 **Google Gemini**，两条流水线可分别选择。
- 每个服务商固定一个专用实时模型：`qwen3.5-livetranslate-flash-realtime` 或 `gemini-3.5-live-translate-preview`。
- 「听人说话」固定翻成**中文**；源语言可手动指定，也可交由服务端自动识别。
- 界面语言：简体中文 / 日本語 / English —— 从侧栏一键切换，与业务翻译源/目标语种互不影响。
- 服务商 API Key 在本地分开保存，并使用 Windows DPAPI 按当前用户加密。
- 服务商元数据统一维护在 [`catalog/aliyun.json`](catalog/aliyun.json) 与 [`catalog/gemini.json`](catalog/gemini.json)，不进源码。

## 开发环境

前置要求：Windows 11 x64、Node.js `^20.19.0` 或 `>=22.12.0`、Rust stable（目标工具链 `x86_64-pc-windows-msvc`）、VS Build Tools（C++ 桌面开发）、WebView2 Runtime、以及一个 API Key（只调界面时可用前端 Mock，无需 Key）。

```powershell
cd app\ui
npm ci
npm run tauri:dev        # 完整桌面应用
npm run dev              # 仅前端，然后打开 http://127.0.0.1:5183/?mock=1
```

## 构建安装包

```powershell
cd app\ui
npm run tauri:build      # 产物为 Windows NSIS，位于 target/release/bundle/nsis/
```

正式打包不要用 `cargo build --release`；若确需手工构建桌面二进制，必须启用 Tauri 自定义协议：

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## 测试

```powershell
cargo test --workspace   # 仓库根目录：Rust 测试
npm run verify           # app/ui 下：类型检查、生产构建、CSS/无障碍/禁用控件检查
```

首页视觉冒烟测试：`npm run build` + `npm run preview -- --host 127.0.0.1`，另开一个终端运行 `node scripts\qa-home.mjs`。

## 目录结构

```text
VoxBridge/
├─ catalog/              # aliyun.json + gemini.json 服务商元数据
├─ crates/
│  ├─ vox-core/          # 平台无关内核：设置、状态机、协议、用量
│  ├─ vox-net/           # 通用 WebSocket 传输
│  ├─ vox-dsp/           # RNNoise 降噪 + 重采样
│  ├─ vox-audio-win/     # WASAPI 采集/播放、进程环回、VB-CABLE
│  ├─ vox-input-win/     # 全局热键
│  └─ vox-overlay-win/   # Win32 透明悬浮字幕窗
├─ app/
│  ├─ src-tauri/         # Tauri 装配层、命令、托盘、持久化、DPAPI
│  └─ ui/                # React 设置界面 + 浏览器 Mock
├─ docs/                 # ARCHITECTURE、DECISIONS、QWEN/GEMINI_PROTOCOL、PROVIDER_CATALOG
├─ Cargo.toml
└─ README.md
```

## 数据流

**对外说话：** `麦克风 → 单声道 → RNNoise → 音量门 → 16 kHz PCM → 服务商 WebSocket →（译文字幕 + 24 kHz 译音到 VB-CABLE/耳机）`

**听人说话：** `进程环回 → 单声道 → 16 kHz PCM → 服务商 WebSocket →（中文字幕 + 24 kHz 中文译音到默认播放设备）`

两条流水线互相独立、可同时运行；所有业务状态以 `vox-core::Runtime` 为唯一账本。

## 配置、用量与密钥

生产环境使用 Tauri 的 `app_config_dir`：

- `settings.json` — 界面与流水线设置
- `usage.json` — 按模型累计的 token 用量
- `secret.bin` — 经 Windows DPAPI 加密的 API Key

设置与用量用临时文件 + rename 原子写入，并去抖落盘。API Key 不会写入 `settings.json`、日志、能力表或 Git 仓库。若 Windows 用户账户/系统环境变化导致旧 DPAPI 密文无法解密，程序会删除失效的 `secret.bin`，用户需重新填写 Key。

## 维护约束

- `vox-core` 不依赖 Tauri、Win32、tokio 或音频设备；平台能力通过 trait 注入。
- WebSocket 的 JSON 形状只在 `crates/vox-core/src/cloud/protocol.rs` 定义。
- 前端/后端字段统一用 `snake_case`，不加驼峰别名。
- 前端只监听一个事件通道：`voxbridge://event`。
- 模型固定由能力表决定；旧配置中的模型会归一到当前模型。
- 输入为 16 kHz PCM16LE 单声道；服务端译音为 24 kHz PCM16LE 单声道。
- 悬浮字幕窗永久鼠标穿透，只负责显示。
- API Key 只能经 `SecretStore` 保存，不能进入普通配置或调试输出。

## 已知限制

- 目前只实现了 Windows 外壳。
- 自动更新尚未接入 GitHub Releases（按钮为占位入口）。
- Gemini 仍是 Preview 模型；真实可用性需用你自己的 AI Studio 项目冒烟确认。
- 真实声卡、进程环回、VB-CABLE 和真实云服务仍需人工冒烟测试。

## 延伸阅读

- `docs/ARCHITECTURE.md`、`docs/DECISIONS.md`、`docs/QWEN_PROTOCOL.md`、`docs/GEMINI_PROTOCOL.md`、`docs/PROVIDER_CATALOG.md`

## License

MIT

---

> 英文版见 [README.md](README.md)。