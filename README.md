# Clipboard History

大量のクリップボード履歴を、履歴件数にほぼ比例しないメモリ使用量で保持・検索するmacOSアプリケーションのプロジェクトです。

現段階ではアプリ全体を実装せず、10,000〜100,000件の履歴を低メモリかつ高速に扱えることの技術検証を優先しています。

## 価値

- 通常10,000件、最大100,000件の履歴を保持する
- 全履歴をメモリへロードしない
- 完全一致、前方一致、正確な部分一致に絞る
- 曖昧検索、OCR、クラウド同期などは初期スコープに含めない
- UIが保持する履歴を50〜200件に制限する
- 大容量payloadはエンコード状態のままディスクへ保存する

## ディレクトリ

```text
clipboard-history/
├── Cargo.toml
├── README.md
├── crates/
│   ├── clipboard-core/
│   ├── clipboard-store/
│   └── clipboard-ffi/
├── app/
│   └── macos/                  # Swift/AppKit + UniFFI application shell
├── docs/
│   └── architecture/
│       ├── clipboard-100k-architecture.drawio
│       └── decisions/
└── experiments/
    └── clipboard-history-poc/
        ├── src/main.rs
        ├── README.md
        ├── RESULTS.md
        └── run-benchmarks.sh
```

## 現在の状態

最初のPoCでは、SQLite、FTS5 trigram、BLAKE3、keyset pagingを想定したインデックス構成を検証しました。10万件でピークRSS約12.8MB、最近100件取得p95約30µs、部分一致検索p95約493µsを確認しています。

- [構成図](docs/architecture/clipboard-100k-architecture.drawio)
- [クレート境界の設計判断](docs/architecture/decisions/0001-crate-boundaries.md)
- [クリップ同一性とストレージ整合性](docs/architecture/decisions/0002-clip-identity-and-storage.md)
- [ストレージエンジン運用方針](docs/architecture/decisions/0003-storage-engine-policies.md)
- [Pasteboard二段階capture境界](docs/architecture/decisions/0004-pasteboard-capture-boundary.md)
- [FFI threadingと100k retention](docs/architecture/decisions/0005-ffi-threading-and-retention.md)
- [menu barから1クリックで履歴表示](docs/architecture/decisions/0006-one-click-history-panel.md)
- [macOS application shell](app/macos/README.md)
- [次PoC: cold-open・WAL・vacuum・payload](docs/experiments/0002-cold-open-wal-vacuum.md)
- [PoC 0002測定結果](docs/experiments/0002-results.md)
- [PoC 0003耐久・孤児scan結果](docs/experiments/0003-soak-results.md)
- [PoC 0002実行スクリプト](experiments/run-storage-engine-poc.sh)
- [耐久・孤児scan実行スクリプト](experiments/run-soak-poc.sh)
- [PoCの説明](experiments/clipboard-history-poc/README.md)
- [PoCの測定結果](experiments/clipboard-history-poc/RESULTS.md)

## 次の検証

PoC 0002により、cache 1MiB、warm-upなし、inline 16KiB、prune batch 100を初期値として採用しました。10万件で検索p95はすべて目標内、workload後RSSは約6.9〜8.2MBでした。長寿命readerがWAL checkpointを妨げること、FTS optimize後のincremental vacuumで100,000件DBを10,000件相当の約9.2〜9.6MBまで縮小できることも確認しています。

さらに5つのcrash pointを別processで検証し、temp/orphan file、row commit後、delete tombstone後、file削除後のすべてからdangling referenceなしで回復しました。

PoC 0003では250,000操作、約5.8日相当のcapture/delete/checkpoint loop後もRSS増加約0.41MB、WAL最大6.55MB、live row 10,000件、GC queue 0を確認しました。100,000 payloadのstreaming orphan scanは約2.5秒、追加RSS約0.7MBでした。

現在はSwift/AppKit + UniFFIのapplication shellからstore actorまで接続済みです。Pasteboard type一覧をpayload読取前にRustの`CaptureFilter`へ渡し、acceptされたrepresentationだけをSQLite/CASへ保存します。recopy=touch、最近50件の一覧、完全一致・前方一致・正確な部分一致検索、選択履歴のPasteboard復元、deleteと遅延payload GC、100,000件のtransactional retentionまでapplication経路で動作します。次はキーボード操作とウィンドウ呼び出しを整え、日常利用できる操作性へ進めます。
