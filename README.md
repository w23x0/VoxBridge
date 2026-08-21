# VoxBridge

A Windows desktop real-time speech translator for live conversations. Two independent pipelines run at once:

| Pipeline | Input | Output |
| --- | --- | --- |
| **Speak out** | your microphone | translated speech into a virtual mic + live captions |
| **Listen in** | a chosen program's audio | Chinese speech to your headphones + live captions |

Built with **Tauri 2 + React 19 + Rust**. The UI handles config and status; audio capture, noise reduction, resampling, WebSocket transport, hotkeys and the overlay caption window live in Rust/Win32.

简体中文：[`READMEs/zh-CN.md`](READMEs/zh-CN.md) · 日本語：[`READMEs/ja.md`](READMEs/ja.md) · 한국어：[`READMEs/ko.md`](READMEs/ko.md) · Español：[`READMEs/es.md`](READMEs/es.md) · Français：[`READMEs/fr.md`](READMEs/fr.md) · Deutsch：[`READMEs/de.md`](READMEs/de.md)

## Scope

- Windows only; process loopback ("Listen in") needs **Win11 / Server 2022 (build 20348+)**.
- Providers — **Alibaba Cloud Bailian**, **Google Gemini**, **OpenAI Realtime** — selectable per pipeline, each with one fixed realtime translation model.
- "Listen in" always translates **into Chinese**; the source language is auto-detected or set manually.
- UI language: 简体中文 / 日本語 / English, independent of the translation languages.
- Provider API keys are stored locally, encrypted with **Windows DPAPI** per user.
- Provider metadata lives in [`catalog/*.json`](catalog/), not in source code.

## Develop

Prereqs: Windows 11 x64, Node.js `^20.19.0` or `>=22.12.0`, Rust stable (`x86_64-pc-windows-msvc`), VS Build Tools (C++ desktop), WebView2, and an API key (or the frontend Mock, which needs none).

```powershell
cd app\ui
npm ci
npm run tauri:dev        # full desktop app
npm run dev              # UI only, open http://127.0.0.1:5183/?mock=1
```

## Build the installer

```powershell
cd app\ui
npm run tauri:build      # produces NSIS at target/release/bundle/nsis/
```

Don't use `cargo build --release` for shipping; if you build the binary by hand, pass the custom protocol feature:

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## Test

```powershell
cargo test --workspace   # Rust, repo root
npm run verify           # in app/ui: type-check, prod build, a11y/disabled checks
```

## Layout

```text
VoxBridge/
├─ catalog/            # provider metadata: aliyun.json, gemini.json, gpt.json
├─ crates/
│  ├─ vox-core/        # platform-neutral core: settings, protocol, state machine, usage
│  ├─ vox-net/         # WebSocket transport
│  ├─ vox-dsp/         # RNNoise denoise + resampling
│  ├─ vox-audio-win/   # WASAPI capture/playback, process loopback, VB-CABLE
│  ├─ vox-input-win/   # global hotkeys
│  └─ vox-overlay-win/ # Win32 transparent caption window
├─ app/
│  ├─ src-tauri/       # Tauri layer: commands, tray, persistence, DPAPI
│  └─ ui/              # React settings UI + browser Mock
├─ docs/               # ARCHITECTURE, DECISIONS, PROTOCOLs, CATALOG
└─ README.md
```

## Data flow

**Speak:** `mic → mono → RNNoise → gate → 16 kHz PCM → provider → (captions + 24 kHz speech to VB-CABLE)`

**Listen:** `loopback → mono → 16 kHz PCM → provider → (Chinese captions + 24 kHz speech to headphones)`

Both pipelines run independently; `vox-core::Runtime` is the single source of truth.

## Contributing

VoxBridge is deliberately conservative: core scope is fixed. External modules are the open extension point — see **[`CONTRIBUTING.md`](CONTRIBUTING.md)** — including the planned Discord out-of-process module (`docs/DISCORD_PROTOCOL.md`, second phase, undecided).

## Docs

`docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, `docs/QWEN_PROTOCOL.md`, `docs/GEMINI_PROTOCOL.md`, `docs/PROVIDER_CATALOG.md`, `docs/DISCORD_PROTOCOL.md`.

## License

MIT