# Clipboard History

[![CI](https://github.com/t-nakatani/clipboard-history/actions/workflows/ci.yml/badge.svg)](https://github.com/t-nakatani/clipboard-history/actions/workflows/ci.yml)

大量のクリップボード履歴を、履歴件数に比例してメモリへ展開せず保持・検索するmacOSメニューバーアプリです。バックエンドはRust、UIとPasteboard連携はSwift/AppKitで実装しています。

> [!NOTE]
> 現在は技術PoCを日常利用できるalphaへ育てている段階です。署名・notarization済みの配布版ではありません。

## 目標

- 通常10,000件、最大100,000件の履歴を保持する
- UIが保持するsummaryを50〜200件に制限する
- 大容量payloadをエンコード状態のままディスクへ分離する
- 完全一致、前方一致、正確な部分一致だけを提供する
- メニューバーから1クリックで履歴へアクセスする
- クラッシュ後も実体のないpayload参照を作らない

曖昧検索、OCR、クラウド同期は初期スコープに含めません。

## 現在の実装

- Pasteboard type一覧をpayload読取前に検査する二段階capture
- concealed/transient markerの保存拒否
- BLAKE3によるclip identityと`recopy = touch`
- SQLite + FTS5 trigramによる検索
- 100,000件のtransactional retention
- 16KiBを超えるpayloadのcontent-addressed file保存
- tombstone queueによる遅延GCと孤児payload回収
- Swift専用serial queueからRust store actorを呼ぶ非同期境界
- 1クリックで開く半透明のAppKit panel
- text/image別のrow heightと画像thumbnail
- Returnで復元、Delete/Backspaceで削除

### 現在の制約

- UIが取得する履歴は最近50件までで、keyset pagingは未接続
- 複数Pasteboard itemは先頭itemだけを保存
- ディスク容量上限、pin、定期checkpoint/vacuumはアプリ経路へ未接続
- global shortcutと設定画面は未実装
- Xcode project、code signing、notarizationは未整備

## ストレージPoCの結果

以下はアプリ全体ではなく、ストレージエンジン単体の測定結果です。

| 検証 | 結果 |
|---|---:|
| 100,000件・最近100件取得 | p95 約30µs |
| 100,000件・部分一致検索 | p95 約493µs |
| 100,000件・初期PoC peak RSS | 約12.8MB |
| production設定でのworkload後RSS | 約6.9〜8.2MB |
| 250,000操作・約5.8日相当のRSS増加 | 約0.41MB |
| soak中の最大WAL | 6.55MB |
| 100,000 payloadの孤児scan | 約2.5秒・追加RSS約0.7MB |

詳細は[最初のPoC結果](experiments/clipboard-history-poc/RESULTS.md)、[PoC 0002](docs/experiments/0002-results.md)、[PoC 0003](docs/experiments/0003-soak-results.md)、[画像preview PoC](docs/experiments/0004-image-preview-results.md)を参照してください。

## アーキテクチャ

```text
Swift / AppKit
  ├─ NSStatusItem・HistoryPanel
  ├─ NSPasteboard capture / restore
  └─ HistoryStoreClient（serial queue）
           │ UniFFI
           ▼
clipboard-ffi
           ▼
clipboard-core
  ├─ domain model・identity・normalization
  ├─ CaptureFilter・QueryPlanner
  └─ repository port
           ▼
clipboard-store
  ├─ SQLite / FTS5 schema・search・retention
  ├─ payload CAS・GC・recovery
  └─ connection owner actor
```

Rustは意味のある境界だけを残した3クレート構成です。

| クレート | 責務 |
|---|---|
| `clipboard-core` | ドメイン型、identity、filter、query planning、repository port |
| `clipboard-store` | SQLite/FTS5、migration、retention、payload CAS、GC、整合性 |
| `clipboard-ffi` | UniFFI facadeとSwift向けDTO |

全体図は[clipboard-100k-architecture.drawio](docs/architecture/clipboard-100k-architecture.drawio)にあります。

## 必要環境

- macOS 13以降
- Rust 1.88以降
- XcodeまたはXcode Command Line Tools
- Swift 5.9以降

## ビルドと実行

Rust workspaceのテストを実行します。

```sh
cargo test --workspace
```

Rust library、UniFFI bindings、macOS appをまとめてビルドします。スクリプト末尾でSwift/UniFFIセルフテストも実行されます。

```sh
./app/macos/build-macos-app.sh
open ./app/macos/build/ClipboardHistory.app
```

履歴DBとpayloadは次へ保存されます。

```text
~/Library/Application Support/ClipboardHistory/
```

macOS shellの詳細は[app/macos/README.md](app/macos/README.md)を参照してください。

## CI

GitHub Actionsではpush、pull request、手動実行に対して次を検証します。

- Rust format、Clippy、workspace test、release build
- 独立ストレージPoCのClippy、test、release build
- macOS 14でのRust/UniFFI/Swift app buildとセルフテスト
- production実装を使った画像preview PoCのsmoke test
- unsigned appの7日間artifact保存

Rust toolchainは[rust-toolchain.toml](rust-toolchain.toml)で1.88.0へ固定しています。CargoとGitHub Actionsの更新はDependabotが毎週確認します。

## UIの調整

panel、row、余白、font、thumbnail、blurなどの値は、次の1ファイルへ集約しています。

- [HistoryPanelConfiguration.swift](app/macos/Sources/HistoryPanelConfiguration.swift)

`HistoryPanelConfiguration.standard`を変更すると、window幅・画面高比率・text/image row高・各margin・文字サイズ・半透明背景をまとめて調整できます。

## ディレクトリ

```text
clipboard-history/
├── crates/
│   ├── clipboard-core/
│   ├── clipboard-store/
│   └── clipboard-ffi/
├── app/macos/                 # Swift/AppKit application
├── docs/
│   ├── architecture/          # draw.io構成図とADR
│   └── experiments/           # PoC計画と結果
└── experiments/
    ├── clipboard-history-poc/ # 最初の独立PoC
    └── results/               # 再現可能な測定ログ
```

## 設計判断

1. [クレート境界](docs/architecture/decisions/0001-crate-boundaries.md)
2. [clip identityとストレージ整合性](docs/architecture/decisions/0002-clip-identity-and-storage.md)
3. [ストレージエンジン運用方針](docs/architecture/decisions/0003-storage-engine-policies.md)
4. [Pasteboard二段階capture](docs/architecture/decisions/0004-pasteboard-capture-boundary.md)
5. [FFI threadingと100k retention](docs/architecture/decisions/0005-ffi-threading-and-retention.md)
6. [メニューバーからの1クリック表示](docs/architecture/decisions/0006-one-click-history-panel.md)
7. [画像preview pipeline](docs/architecture/decisions/0007-image-preview-pipeline.md)
8. [履歴panelの表示密度](docs/architecture/decisions/0008-history-panel-density.md)

## 次の優先事項

詳細なチェックリストと完了条件は[TODO.md](TODO.md)で管理します。

1. recent/searchのkeyset pagingをアプリ経路へ接続する
2. 復元時の自己再captureを抑止する
3. 起動時の孤児payload recoveryとidle maintenanceを接続する
4. 画像込み100,000件でアプリ全体のRSSと描画時間を測定する
5. global shortcut、エラー表示、設定画面を追加する
6. ディスク容量retention、pin、配布工程を整備する
