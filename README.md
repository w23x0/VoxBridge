# VoxBridge

A Windows desktop real-time speech translation tool for live conversations. Two independent pipelines run simultaneously:

| Pipeline | Input | Output |
| --- | --- | --- |
| Speak out | Microphone | Foreign-language speech to a virtual microphone, plus live captions |
| Listen in | A chosen program's audio | Chinese speech to headphones, plus live captions |

Built with **Tauri 2 + React 19 + Rust**. The UI handles configuration and status; audio capture, noise reduction, resampling, WebSocket transport, hotkeys and overlay captions live in Rust/Win32.

## Feature scope

- Windows only; process loopback for "Listen in" is fully supported on Windows 11 / Server 2022 (build 20348+).
- Providers: **Alibaba Cloud Bailian** and **Google Gemini**, selectable per pipeline.
- Each provider uses one fixed realtime model: `qwen3.5-livetranslate-flash-realtime` or `gemini-3.5-live-translate-preview`.
- "Listen in" translates into **Chinese**; source language can be set manually or auto-detected.
- UI language: 简体中文 / 日本語 / English — toggle from the sidebar; independent of translation languages.
- Provider API keys are stored locally, encrypted with Windows DPAPI per user.
- Provider metadata lives in [`catalog/aliyun.json`](catalog/aliyun.json) and [`catalog/gemini.json`](catalog/gemini.json), not in source code.

## Develop

Prereqs: Windows 11 x64, Node.js `^20.19.0` or `>=22.12.0`, Rust stable (`x86_64-pc-windows-msvc`), VS Build Tools (C++ desktop), WebView2, and an API key (or the frontend Mock, which needs no key).

```powershell
cd app\ui
npm ci
npm run tauri:dev        # full desktop app
npm run dev              # UI only, then open http://127.0.0.1:5183/?mock=1
```

## Build the installer

```powershell
cd app\ui
npm run tauri:build      # produces Windows NSIS at target/release/bundle/nsis/
```

Don't use `cargo build --release` for shipping; if you manually build the desktop binary, enable the Tauri custom protocol:

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## Test

```powershell
cargo test --workspace   # in repo root: Rust tests
npm run verify           # in app/ui: type-check, prod build, CSS/accessibility/disabled controls
```

For the home-screen visual smoke test: `npm run build` + `npm run preview -- --host 127.0.0.1`, then in another terminal run `node scripts\qa-home.mjs`.

## Layout

```text
VoxBridge/
├─ catalog/              # aliyun.json + gemini.json provider metadata
├─ crates/
│  ├─ vox-core/          # platform-neutral core: settings, state machine, protocol, usage
│  ├─ vox-net/           # generic WebSocket transport
│  ├─ vox-dsp/           # RNNoise denoise + resampling
│  ├─ vox-audio-win/     # WASAPI capture/playback, loopback, VB-CABLE
│  ├─ vox-input-win/     # global hotkeys
│  └─ vox-overlay-win/   # transparent Win32 caption window
├─ app/
│  ├─ src-tauri/         # Tauri layer, commands, tray, persistence, DPAPI
│  └─ ui/                # React settings UI + browser Mock
├─ docs/                 # ARCHITECTURE, DECISIONS, QWEN/GEMINI_PROTOCOL, PROVIDER_CATALOG
├─ Cargo.toml
└─ README.md
```

## Data flow

**Speak out:** `mic → mono → RNNoise → gain gate → 16 kHz PCM → provider WebSocket → (captions + 24 kHz speech to VB-CABLE/headphones)`

**Listen in:** `program loopback → mono → 16 kHz PCM → provider WebSocket → (Chinese captions + 24 kHz speech to default device)`

The two pipelines run independently; `vox-core::Runtime` is the single source of truth for all state.

## Config, usage & secrets

Production uses Tauri's `app_config_dir`:

- `settings.json` — UI & pipeline settings
- `usage.json` — per-model token usage
- `secret.bin` — DPAPI-encrypted API key

Settings and usage write atomically (temp file + rename, debounced). Keys never appear in `settings.json`, logs, the catalog, or git. If DPAPI can't decrypt an old key (user/system environment changed), the stale `secret.bin` is removed and the user re-enters the key.

## Maintenance constraints

- `vox-core` must not depend on Tauri, Win32, tokio, or audio devices — platform abilities are injected as traits.
- WebSocket JSON shapes are defined only in `crates/vox-core/src/cloud/protocol.rs`.
- Frontend/backend fields are `snake_case`, no camelCase aliases.
- Frontend listens to a single event channel: `voxbridge://event`.
- Models are fixed by the catalog; stale configs normalize to the current model.
- Input is 16 kHz PCM16LE mono; server output is 24 kHz PCM16LE mono.
- The caption window is permanently click-through (display only).
- API keys go through `SecretStore` only, never into normal config or debug output.

## Known limitations

- Windows shell only (for now).
- Auto-update is not yet wired to GitHub Releases (the button is a placeholder).
- Gemini is a Preview model; smoke-test real availability with your own AI Studio project.
- Real sound cards, loopback, VB-CABLE and live cloud services need manual testing.

## Further reading

- `docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, `docs/QWEN_PROTOCOL.md`, `docs/GEMINI_PROTOCOL.md`, `docs/PROVIDER_CATALOG.md`

## License

MIT