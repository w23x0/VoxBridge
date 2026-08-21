# VoxBridge

ライブ会話のための Windows デスクトップ向けリアルタイム音声翻訳アプリ。2 つの独立したパイプラインが同時に動作します:

| パイプライン | 入力 | 出力 |
| --- | --- | --- |
| **Speak out** | 自分のマイク | バーチャルマイクへの翻訳音声 + ライブ字幕 |
| **Listen in** | 選択したプログラムの音声 | ヘッドホンへの中国語音声 + ライブ字幕 |

**Tauri 2 + React 19 + Rust** で構築されています。UI は設定と状態を担当し、音声キャプチャ・ノイズ除去・リサンプリング・WebSocket 転送・ホットキー・字幕オーバーレイウィンドウは Rust/Win32 側に実装されています。

English： [`../README.md`](../README.md) · 简体中文： [`zh-CN.md`](zh-CN.md) · 한국어： [`ko.md`](ko.md) · Español： [`es.md`](es.md) · Français： [`fr.md`](fr.md) · Deutsch： [`de.md`](de.md)

## スコープ

- Windows のみ。プロセスループバック（"Listen in"）は **Win11 / Server 2022（build 20348+）** が必要。
- プロバイダ — **Alibaba Cloud Bailian**、**Google Gemini**、**OpenAI Realtime** — パイプラインごとに選択可能で、それぞれ固定のリアルタイム翻訳モデルを 1 つ使用。
- "Listen in" は常に **中国語へ** 翻訳。翻訳元の言語は自動検出または手動指定。
- UI 言語は 简体中文 / 日本語 / English で、翻訳の言語とは独立。
- プロバイダの API キーはローカルに保存され、ユーザーごとに **Windows DPAPI** で暗号化。
- プロバイダのメタデータは [`catalog/*.json`](../catalog/) 内に置かれ、ソースコードには含めない。

## 開発 (Develop)

前提条件: Windows 11 x64、Node.js `^20.19.0` または `>=22.12.0`、Rust stable（`x86_64-pc-windows-msvc`）、VS Build Tools（C++ デスクトップ）、WebView2、および API キー（またはフロントの Mock。これは不要）。

```powershell
cd app\ui
npm ci
npm run tauri:dev        # デスクトップアプリ全体
npm run dev              # UI のみ、http://127.0.0.1:5183/?mock=1 を開く
```

## インストーラのビルド

```powershell
cd app\ui
npm run tauri:build      # NSIS を target/release/bundle/nsis/ に生成
```

配布に `cargo build --release` は使わないでください。バイナリを手動でビルドする場合は、custom protocol フィーチャーを渡してください:

```powershell
cargo build --release -p voxbridge --features custom-protocol
```

## テスト

```powershell
cargo test --workspace   # Rust、リポジトリルート
npm run verify           # app/ui 内: 型チェック、本番ビルド、a11y/disabled チェック
```

## ディレクトリ構成

```text
VoxBridge/
├─ catalog/            # プロバイダのメタデータ: aliyun.json、gemini.json、gpt.json
├─ crates/
│  ├─ vox-core/        # プラットフォーム非依存のコア: 設定、プロトコル、状態機械、利用統計
│  ├─ vox-net/         # WebSocket 転送
│  ├─ vox-dsp/         # RNNoise ノイズ除去 + リサンプリング
│  ├─ vox-audio-win/   # WASAPI キャプチャ/再生、プロセスループバック、VB-CABLE
│  ├─ vox-input-win/   # グローバルホットキー
│  └─ vox-overlay-win/ # Win32 透過字幕ウィンドウ
├─ app/
│  ├─ src-tauri/       # Tauri レイヤー: コマンド、トレイ、永続化、DPAPI
│  └─ ui/              # React 設定 UI + ブラウザ Mock
├─ docs/               # ARCHITECTURE、DECISIONS、PROTOCOL、CATALOG
└─ README.md
```

## データフロー

**Speak:** `mic → mono → RNNoise → gate → 16 kHz PCM → provider → (字幕 + 24 kHz 音声を VB-CABLE へ)`

**Listen:** `loopback → mono → 16 kHz PCM → provider → (中国語字幕 + 24 kHz 音声をヘッドホンへ)`

両パイプラインは独立して動作し、`vox-core::Runtime` が唯一の真実の源です。

## 貢献 (Contributing)

VoxBridge は意図的に保守的で、コアスコープは固定されています。外部モジュールが拡張のための公開ポイントです — **[`../CONTRIBUTING.md`](../CONTRIBUTING.md)** を参照してください — また、計画中の Discord アウトプロセスモジュール（`docs/DISCORD_PROTOCOL.md`、第 2 期・未定）も含まれます。

## ドキュメント

`docs/ARCHITECTURE.md`、`docs/DECISIONS.md`、`docs/QWEN_PROTOCOL.md`、`docs/GEMINI_PROTOCOL.md`、`docs/PROVIDER_CATALOG.md`、`docs/DISCORD_PROTOCOL.md`。

## ライセンス

MIT

English: [`../README.md`](../README.md) · 简体中文: [`zh-CN.md`](zh-CN.md) · 한국어: [`ko.md`](ko.md) · Español: [`es.md`](es.md) · Français: [`fr.md`](fr.md) · Deutsch: [`de.md`](de.md)