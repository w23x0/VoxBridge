# VoxBridge

VoxBridge 是一个面向实时对话场景的 Windows 桌面语音翻译工具。它把“我说出去”和“我听进来”拆成两条可以同时运行的流水线：

| 功能 | 输入 | 输出 |
| --- | --- | --- |
| 对外说话 | 本机麦克风 | 外语译音送入虚拟麦克风，同时显示字幕 |
| 听人说话 | 指定 Windows 程序的声音 | 中文译音播放到耳机，同时显示字幕 |

项目采用 Tauri 2 + React 19 + Rust。界面负责配置与状态展示；音频采集、降噪、重采样、WebSocket、热键和悬浮字幕由 Rust/Win32 实现。

## 当前产品范围

- 仅支持 Windows；“听人说话”的进程环回完整支持以 Windows 11 / Server 2022（build 20348 及以上）为准。
- 支持阿里云百炼与 Google Gemini，两条流水线可分别选择服务商。
- 每个服务商固定一个专用实时翻译模型：`qwen3.5-livetranslate-flash-realtime` 或 `gemini-3.5-live-translate-preview`。
- 阿里云支持 60 种语言（29 种语音+字幕）；Gemini Live Translation 官方标注支持 70+ 语言语音互译。
- 听人说话固定翻成中文；源语言可手动指定，也可以交给服务端自动识别。
- 界面支持简体中文 / English 两种显示语言，可在设置中切换、随配置持久化；它和业务翻译的源/目标语种互不影响。
- 两个服务商的 API Key 在本地分开保存，并使用 Windows DPAPI 按当前用户加密。
- “关于 → 检查更新”目前是占位入口；GitHub Releases 尚未接入，不会下载任何文件。

服务商元数据统一维护在 [`catalog/aliyun.json`](catalog/aliyun.json) 与 [`catalog/gemini.json`](catalog/gemini.json)。

## 开发环境

建议准备：

- Windows 11 x64。
- Node.js `^20.19.0` 或 `>=22.12.0`。
- Rust stable，目标工具链为 `x86_64-pc-windows-msvc`。
- Visual Studio Build Tools 的“使用 C++ 的桌面开发”组件。
- Microsoft Edge WebView2 Runtime。
- 一个阿里云百炼或 Google AI Studio API Key；只看界面时可以使用前端 Mock，不需要 Key。

克隆或复制项目后安装前端依赖：

```powershell
cd app\ui
npm ci
```

启动完整桌面应用：

```powershell
npm run tauri:dev
```

只调试前端界面：

```powershell
npm run dev
```

然后打开 `http://127.0.0.1:5183/?mock=1`。Mock 会模拟设备、两条运行中的流水线、字幕和 token 用量。

## 构建安装包

在 `app/ui` 下运行：

```powershell
npm run tauri:build
```

当前打包目标是 Windows NSIS，产物位于工作区根目录下的 `target/release/bundle/nsis/`。

不要用普通的 `cargo build --release` 代替正式打包。如果确实要手工构建桌面二进制，必须启用 Tauri 自定义协议：

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

否则发布版窗口仍可能尝试连接开发服务器。

## 验证

Rust 全量测试，在项目根目录运行：

```powershell
cargo test --workspace
```

前端完整验证，在 `app/ui` 下运行：

```powershell
npm run verify
```

该命令依次执行 TypeScript 检查、生产构建、CSS 类检查、无障碍/窄窗口检查和控件禁用状态检查。

首页交互与视觉冒烟测试需要先构建并启动预览服务：

```powershell
npm run build
npm run preview -- --host 127.0.0.1
```

另开一个终端，在 `app/ui` 下运行：

```powershell
node scripts\qa-home.mjs
```

它会验证两张主卡等宽、模型控件已经移除、下拉菜单没有被遮挡、语言/音色最近使用排序，以及“检查更新”占位入口。

## 目录结构

```text
VoxBridge/
├─ catalog/
│  ├─ aliyun.json             # 阿里云模型、语言、音色与 API 元数据
│  └─ gemini.json             # Gemini Live Translation 元数据
├─ crates/
│  ├─ vox-core/               # 平台无关内核、设置、状态机、协议、用量
│  ├─ vox-net/                # 通用 WebSocket 传输
│  ├─ vox-dsp/                # RNNoise 降噪和重采样
│  ├─ vox-audio-win/          # WASAPI 采集、播放、进程环回、VB-CABLE
│  ├─ vox-input-win/          # Windows 全局热键
│  └─ vox-overlay-win/        # Win32 透明悬浮字幕窗
├─ app/
│  ├─ src-tauri/              # Tauri 装配层、命令、托盘、持久化、DPAPI
│  └─ ui/                     # React 设置界面和浏览器 Mock
├─ docs/
│  ├─ ARCHITECTURE.md         # 完整架构和线程/数据流说明
│  ├─ DECISIONS.md            # 已拍板与历史决策
│  ├─ QWEN_PROTOCOL.md        # LiveTranslate WebSocket 协议细节
│  ├─ GEMINI_PROTOCOL.md      # Gemini Live Translation 协议与配额行为
│  └─ PROVIDER_CATALOG.md     # 服务商能力表维护流程
├─ Cargo.toml                 # Rust workspace
└─ README.md
```

## 核心数据流

### 对外说话

```text
麦克风 → 单声道 → RNNoise → 音量门 → 16 kHz PCM → 服务商 WebSocket
                                                        ├→ 译文字幕
                                                        └→ 24 kHz 译音 → VB-CABLE/耳机
```

### 听人说话

```text
指定程序的进程环回 → 单声道 → 16 kHz PCM → 服务商 WebSocket
                                               ├→ 中文字幕
                                               └→ 24 kHz 中文译音 → 默认播放设备
```

两条流水线互相独立，可以同时运行；所有业务状态以 `vox-core::Runtime` 为唯一账本。

## 服务商能力表怎么维护

不要直接在 TypeScript 或 Rust 源码里手抄服务商元数据。修改对应的 `catalog/*.json`：

1. 核对服务商的模型、实时翻译、事件和限流官方文档。
2. 更新模型 ID、快照、语言、音色或 API 元数据。
3. 更新 `verified_at`。
4. 运行 `cargo test --workspace` 和 `npm run verify`。
5. 使用真实 API Key 分别做一次“中文翻出去”和“外语翻成中文”的冒烟测试。
6. 提升应用版本并发布新安装包。

`crates/vox-core/build.rs` 会在编译时读取两份 JSON；阿里云目录继续检查完整语言/音色计数，Gemini 目录检查 provider、模型 ID 和官方 WebSocket 域名。

- 语言、语音输出语言和音色数组的数量必须与 `expected_counts` 一致。
- 默认模型、语言和音色必须存在。
- 语言代码和音色 ID 不能重复。

当前 `expected_counts` 是 60 / 29 / 47；如果官方能力变化，只需在同一 JSON 中同时更新数据和计数。

前端直接导入同一文件，所以两端不会再出现“后端支持、界面没更新”或相反的情况。更详细的维护步骤见 [`docs/PROVIDER_CATALOG.md`](docs/PROVIDER_CATALOG.md)。

## 配置、用量和密钥

生产环境使用 Tauri 的 `app_config_dir`：

- `settings.json`：界面与流水线设置。
- `usage.json`：按模型累计的 token 用量。
- `secret.bin`：经 Windows DPAPI 加密的 API Key。

设置和用量使用临时文件 + rename 原子写入，并由后台线程去抖落盘。API Key 不会写进 `settings.json`、日志、能力表或 Git 仓库。

如果 Windows 用户账户或系统环境改变导致旧 DPAPI 密文无法解密，程序会删除失效的 `secret.bin`，用户需要重新填写 API Key。

## 软件更新与 GitHub 发布

项目当前还没有 GitHub 仓库和 Releases，因此更新按钮只显示“通道待配置”。正式接入时需要同时完成：

1. 初始化 Git 仓库并创建 GitHub 仓库。
2. 接入 Tauri Updater 插件和最小权限。
3. 生成更新签名密钥；公钥写进应用配置，私钥只放 GitHub Actions Secrets。
4. 用 GitHub Actions 构建 NSIS 安装包、更新包、签名和更新清单。
5. 在 GitHub Releases 发布产物，并把更新端点接到“关于”页按钮。
6. 验证旧版本能检查、下载、验签、安装并重启到新版本。

发布时需要同步检查以下版本号：

- 根 [`Cargo.toml`](Cargo.toml) 的 `workspace.package.version`。
- [`app/src-tauri/tauri.conf.json`](app/src-tauri/tauri.conf.json) 的 `version`。
- [`app/ui/package.json`](app/ui/package.json) 与 `package-lock.json` 的版本。

签名私钥、API Key、`.env`、安装包和 `target/` 不得提交。根目录 [`.gitignore`](.gitignore) 已预留这些规则。

## 维护时不要破坏的约束

- `vox-core` 不依赖 Tauri、Win32、tokio 或具体音频设备；平台能力通过 trait 注入。
- WebSocket 的 JSON 形状只在 `crates/vox-core/src/cloud/protocol.rs` 定义。
- 前端与后端通信字段使用 Rust serde 的 `snake_case`，不要自行加驼峰别名。
- 前端只监听一个事件通道：`voxbridge://event`。
- 模型固定由能力表决定；旧配置中的模型会归一到当前模型。
- 阿里云目标语言/音色可以热更新；Gemini 的 setup 只能作为首帧，目标语言变化需重建 WebSocket。
- 输入为 16 kHz PCM16LE 单声道，服务端译音为 24 kHz PCM16LE 单声道。
- 悬浮字幕窗永久鼠标穿透，只负责显示，不承载设置和控制按钮。
- API Key 只能经 `SecretStore` 保存，不能进入普通配置或调试输出。
- 不要把阿里云的 `idle_timeout_ms` 当作连接保活参数。

## 已知限制

- 只实现了 Windows 外壳。
- 当前连接仍使用阿里云旧公共 WebSocket 域名；维护表已经记录业务空间专属地址模板，但界面尚未收集地域和 Workspace ID。
- Gemini 模型仍是 Preview；真实服务可用性和项目级配额必须用用户自己的 AI Studio 项目冒烟确认。
- 自动更新尚未连接 GitHub Releases。
- CI 可以覆盖协议、状态机、界面和大部分音频逻辑，但真实声卡、进程环回、VB-CABLE 和真实云服务仍需人工冒烟测试。

## 进一步阅读

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)：分层、线程拓扑、音频管线和模块职责。
- [`docs/QWEN_PROTOCOL.md`](docs/QWEN_PROTOCOL.md)：WebSocket 握手、事件、采样率、错误和重连。
- [`docs/GEMINI_PROTOCOL.md`](docs/GEMINI_PROTOCOL.md)：Gemini setup、实时音频、返回事件和配额处理。
- [`docs/DECISIONS.md`](docs/DECISIONS.md)：为什么这样设计，以及已经拍板的取舍。
- [`docs/PROVIDER_CATALOG.md`](docs/PROVIDER_CATALOG.md)：模型、语言、音色和 API 元数据的维护流程。

## License

MIT
