# Contributing

Thanks for looking. VoxBridge is deliberately **conservative**: the core scope is fixed and its behavior-contrors are documented, not improvised. Bugs and good bug reports are always welcome; big direction changes need a decision tracker, not a stream of PRs.

Your time is best spent **outside the core** — see below.

## Scope guardrail (read before coding)

- Core pipelines, providers, protocol JSON shapes, and the event channel are **decided** — see `docs/DECISIONS.md`.
- When docs and code disagree, **the code wins** and `DECISIONS.md` gets corrected to match.
- Contributions that silently change fixed scope are the one thing a collaborator must not do freeform.

## Where to help — the open extension point

VoxBridge treats **external modules** as its open surface. The planned Discord module (`docs/DISCORD_PROTOCOL.md`) is the concrete example: a self-contained crate (`crates/vox-discord/`) that only talks to existing `vox-net` and the single event channel — it **does not** touch the mic / VB-CABLE / loopback / protocol internals.

That's the bar: if your idea fits as an out-of-process module on the existing edge, it's welcome. Anything that rewires the core needs a decision first.

## Report a bug

Use the [issue template](.github/ISSUE_TEMPLATE/bug.yml). This is a realtime-audio Windows app: reproduce on a real sound card / loopback / live provider, and include the Windows build number, provider + pipeline, and steps.

## Set up the app

Prereqs: Windows 11 x64, Node.js `^20.19.0` / `>=22.12.0`, Rust stable (`x86_64-pc-windows-msvc`), VS Build Tools (C++ desktop), WebView2, an API key (or the Mock).

```powershell
cd app\ui
npm ci
npm run tauri:dev            # full desktop app
npm run dev                  # UI only: http://127.0.0.1:5183/?mock=1
```

## Building blocks to honor

- Frontend/backend fields are `snake_case` — no camelCase aliases.
- Every status flows over one event channel: `voxbridge://event`.
- WebSocket JSON shapes live only in `crates/vox-core/src/cloud/protocol.rs`.
- `vox-core` stays free of Tauri, Win32, tokio, and audio devices — platform abilities come in as traits.
- Provider metadata is edited in `catalog/*.json` (or the Rust build checks/`catalog_updater`), never hard-coded.
- API keys go through `SecretStore` only (DPAPI); never in config, logs, or git.

These are the ground rules from `docs/ARCHITECTURE.md` and `docs/DECISIONS.md` — read both before touching code.

## Commit message format

Conventional Commits:

```text
feat(area): short summary
fix(area): short summary
docs: ...
build(deps): ...
```

Examples from history: `feat(catalog): ...`, `fix(a11y): ...`, `feat(gpt): ...`.

Keep history tidy; one logical change per commit.

## Docs

- `docs/ARCHITECTURE.md` — layering, directory roles, thread topology
- `docs/DECISIONS.md` — decided behavior + the open decision list
- `docs/QWEN_PROTOCOL.md`, `docs/GEMINI_PROTOCOL.md` — WS protocol specs
- `docs/PROVIDER_CATALOG.md` — how to keep provider metadata current
- `docs/DISCORD_PROTOCOL.md` — the second-phase Discord module design (open)

Thank you for shipping this forward.