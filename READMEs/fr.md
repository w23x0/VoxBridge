# VoxBridge

Un traducteur vocal en temps réel pour Windows, conçu pour les conversations en direct. Deux pipelines indépendants tournent en parallèle :

| Pipeline | Entrée | Sortie |
| --- | --- | --- |
| **Speak out** (parler) | votre micro | voix traduite vers un micro virtuel + sous-titres en direct |
| **Listen in** (écouter) | l'audio d'un programme choisi | voix en chinois vers votre casque + sous-titres en direct |

Basé sur **Tauri 2 + React 19 + Rust**. L'UI gère la configuration et l'état ; la capture audio, la réduction de bruit, le rééchantillonnage, le transport WebSocket, les raccourcis clavier et la fenêtre de sous-titres overlay vivent côté Rust/Win32.

English： [`../README.md`](../README.md) · 简体中文： [`zh-CN.md`](zh-CN.md) · 日本語： [`ja.md`](ja.md) · 한국어： [`ko.md`](ko.md) · Español： [`es.md`](es.md) · Français： [`fr.md`](fr.md) · Deutsch： [`de.md`](de.md)

## Périmètre

- Windows uniquement ; le loopback par processus (« Listen in ») nécessite **Win11 / Server 2022 (build 20348+)**.
- Fournisseurs — **Alibaba Cloud Bailian**, **Google Gemini**, **OpenAI Realtime** — sélectionnables par pipeline, chacun avec un modèle de traduction temps réel fixe.
- « Listen in » traduit toujours **vers le chinois** ; la langue source est détectée automatiquement ou fixée manuellement.
- Langue de l'UI : 简体中文 / 日本語 / English, indépendante des langues de traduction.
- Les clés API des fournisseurs sont stockées localement, chiffrées par utilisateur avec **Windows DPAPI**.
- Les métadonnées des fournisseurs vivent dans [`catalog/*.json`](../catalog/), hors code source.

## Développer

Prérequis : Windows 11 x64, Node.js `^20.19.0` ou `>=22.12.0`, Rust stable (`x86_64-pc-windows-msvc`), VS Build Tools (C++ desktop), WebView2, et une clé d'API (ou le Mock frontal, qui n'en exige aucune).

```powershell
cd app\ui
npm ci
npm run tauri:dev        # application desktop complète
npm run dev              # UI seule ; ouvrir http://127.0.0.1:5183/?mock=1
```

## Construire l'installeur

```powershell
cd app\ui
npm run tauri:build      # produit l'installeur NSIS dans target/release/bundle/nsis/
```

Ne livrez pas avec `cargo build --release` ; si vous assemblez le binaire à la main, passez la feature custom-protocol :

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## Tester

```powershell
cargo test --workspace   # Rust, à la racine du dépôt
npm run verify           # dans app/ui : type-check, build de prod, vérifs a11y/disabled
```

## Structure

```text
VoxBridge/
├─ catalog/            # métadonnées des fournisseurs : aliyun.json, gemini.json, gpt.json
├─ crates/
│  ├─ vox-core/        # cœur neutre vis-à-vis de la plateforme : settings, protocole, machine à états, usage
│  ├─ vox-net/         # transport WebSocket
│  ├─ vox-dsp/         # débruitage RNNoise + rééchantillonnage
│  ├─ vox-audio-win/   # capture/lecture WASAPI, loopback par processus, VB-CABLE
│  ├─ vox-input-win/   # raccourcis clavier globaux
│  └─ vox-overlay-win/ # fenêtre de sous-titres transparente Win32
├─ app/
│  ├─ src-tauri/       # couche Tauri : commandes, tray, persistance, DPAPI
│  └─ ui/              # UI de réglages React + Mock navigateur
├─ docs/               # ARCHITECTURE, DECISIONS, PROTOCOLs, CATALOG
└─ README.md
```

## Flux de données

**Speak :** `mic → mono → RNNoise → gate → 16 kHz PCM → provider → (captions + 24 kHz speech to VB-CABLE)`

**Listen :** `loopback → mono → 16 kHz PCM → provider → (Chinese captions + 24 kHz speech to headphones)`

Les deux pipelines tournent indépendamment ; `vox-core::Runtime` est l'unique source de vérité.

## Contribuer

VoxBridge reste volontairement conservateur : le périmètre cœur est fixé. Les modules externes sont le point d'extension ouvert — voir **[`../CONTRIBUTING.md`](../CONTRIBUTING.md)** — y compris le module Discord hors-processus prévu (`docs/DISCORD_PROTOCOL.md`, phase 2, non décidé).

## Documentation

`docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, `docs/QWEN_PROTOCOL.md`, `docs/GEMINI_PROTOCOL.md`, `docs/PROVIDER_CATALOG.md`, `docs/DISCORD_PROTOCOL.md`.

## Licence

MIT
