# VoxBridge

Ein Windows-Desktop-Übersetzer für Echtzeit-Sprache in Live-Gesprächen. Zwei unabhängige Pipelines laufen gleichzeitig:

| Pipeline | Eingang | Ausgang |
| --- | --- | --- |
| **Speak out** | dein Mikrofon | übersetzte Sprache in ein virtuelles Mikrofon + Live-Untertitel |
| **Listen in** | Audio eines gewählten Programms | chinesische Sprache zu deinen Kopfhörern + Live-Untertitel |

Basiert auf **Tauri 2 + React 19 + Rust**. Die UI steuert Konfiguration und Status; Audioaufnahme, Rauschunterdrückung, Resampling, WebSocket-Transport, Hotkeys und das Overlay-Untertitelfenster liegen in Rust/Win32.

English： [`../README.md`](../README.md) · 简体中文： [`zh-CN.md`](zh-CN.md) · 日本語： [`ja.md`](ja.md) · 한국어： [`ko.md`](ko.md) · Español： [`es.md`](es.md) · Français： [`fr.md`](fr.md) · Deutsch： [`de.md`](de.md)

## Umfang

- Nur Windows; Prozess-Loopback („Listen in") benötigt **Win11 / Server 2022 (Build 20348+)**.
- Anbieter — **Alibaba Cloud Bailian**, **Google Gemini**, **OpenAI Realtime** — pro Pipeline wählbar, jeweils ein festes Echtzeit-Übersetzungsmodell.
- „Listen in" übersetzt immer **ins Chinesische**; die Quellsprache wird automatisch erkannt oder manuell festgelegt.
- UI-Sprache: 简体中文 / 日本語 / English, unabhängig von den Übersetzungssprachen.
- API-Schlüssel der Anbieter werden lokal gespeichert und pro Benutzer mit **Windows DPAPI** verschlüsselt.
- Anbieter-Metadaten liegen in [`catalog/*.json`](../catalog/), nicht im Quellcode.

## Entwickeln

Voraussetzungen: Windows 11 x64, Node.js `^20.19.0` oder `>=22.12.0`, Rust stable (`x86_64-pc-windows-msvc`), VS Build Tools (C++ Desktop), WebView2 sowie ein API-Schlüssel (oder das Frontend-Mock, das keines benötigt).

```powershell
cd app\ui
npm ci
npm run tauri:dev        # komplette Desktop-App
npm run dev              # nur UI; öffne http://127.0.0.1:5183/?mock=1
```

## Installer erstellen

```powershell
cd app\ui
npm run tauri:build      # erzeugt NSIS unter target/release/bundle/nsis/
```

Für Releases nicht `cargo build --release` verwenden; beim manuellen Bauen des Binaries das Feature für das Custom-Protocol mitgeben:

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## Testen

```powershell
cargo test --workspace   # Rust, Repository-Wurzel
npm run verify           # in app/ui: Typ-Check, Prod-Build, a11y/disabled-Prüfung
```

## Struktur

```text
VoxBridge/
├─ catalog/            # Anbieter-Metadaten: aliyun.json, gemini.json, gpt.json
├─ crates/
│  ├─ vox-core/        # plattformneutraler Kern: Einstellungen, Protokoll, Zustandsmaschine, Verbrauch
│  ├─ vox-net/         # WebSocket-Transport
│  ├─ vox-dsp/         # RNNoise-Entrauschung + Resampling
│  ├─ vox-audio-win/   # WASAPI-Aufnahme/Wiedergabe, Prozess-Loopback, VB-CABLE
│  ├─ vox-input-win/   # globale Hotkeys
│  └─ vox-overlay-win/ # transparentes Win32-Untertitel-Fenster
├─ app/
│  ├─ src-tauri/       # Tauri-Ebene: Befehle, Tray, Persistenz, DPAPI
│  └─ ui/              # React-Einstellungs-UI + Browser-Mock
├─ docs/               # ARCHITECTURE, DECISIONS, PROTOCOLs, CATALOG
└─ README.md
```

## Datenfluss

**Speak:** `mic → mono → RNNoise → gate → 16 kHz PCM → provider → (Untertitel + 24 kHz Sprache zu VB-CABLE)`

**Listen:** `loopback → mono → 16 kHz PCM → provider → (chinesische Untertitel + 24 kHz Sprache zu Kopfhörern)`

Beide Pipelines laufen unabhängig; `vox-core::Runtime` ist die einzige Wahrheitsquelle.

## Beitragen

VoxBridge ist bewusst konservativ: der Kern-Umfang ist fest. Externe Module sind der offene Erweiterungspunkt — siehe **[`../CONTRIBUTING.md`](../CONTRIBUTING.md)** — einschließlich des geplanten externen Discord-Moduls (`docs/DISCORD_PROTOCOL.md`, Phase 2 · unentschieden).

## Doku

`docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, `docs/QWEN_PROTOCOL.md`, `docs/GEMINI_PROTOCOL.md`, `docs/PROVIDER_CATALOG.md`, `docs/DISCORD_PROTOCOL.md`.

## Lizenz

MIT
