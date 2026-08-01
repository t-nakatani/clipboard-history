# TODO

最終更新: 2026-08-01

このファイルは、Clipboard Historyを技術PoCから日常利用可能なmacOSアプリへ進めるための残タスクを管理します。

## P0: プロジェクトの価値を成立させる

### 履歴のkeyset paging

- [x] recent一覧にcursorを導入する
  - [x] `(last_used_at, id)`をcursorとしてRust APIへ追加する
  - [x] UniFFI DTOへ方向、`has_more`、bounded scan用`continuation_cursor`と`truncated`を追加する
  - [x] scroll上下端で前後50件を非同期取得する
  - [x] Swiftが保持するsummaryを最大100〜200件に制限する
  - [x] 古いpageを解放してもscroll位置を維持する
  - [x] 解放した新しいpageへ上方向のcursorで戻れる
- [x] search結果にも同じpagingモデルを適用する
  - [x] 疎な短文検索で2000行の空windowを越えて継続する
- [x] capture、delete、recopy発生中も重複・欠落しないことをテストする

完了条件: 100,000件の履歴を、全件をメモリへ載せず最後まで辿れる。

### 自己再captureの抑止

- [ ] 履歴復元によるPasteboard更新をmonitorが再保存しない仕組みを決める
- [ ] change countまたはcanonical identityで次の1回だけ抑止する
- [ ] 通常の再copyは従来どおり`recopy = touch`になることをテストする

完了条件: 履歴を復元しても`copy_count`と並び順が意図せず変化しない。

### 起動時recovery

- [ ] `recover_orphans`をUniFFIへ公開する
- [ ] unclean shutdownを判定するmarkerを追加する
- [ ] unclean shutdown後だけbackground orphan scanを実行する
- [ ] recovery中もpanelと最近の履歴を開けるようにする
- [ ] quick check失敗時にDBを隔離し、再構築できる回復経路を作る

完了条件: crash後の再起動でdangling referenceを作らず、孤児fileを回収できる。

### ユーザーへ見えるエラー表示

- [ ] panel内の固定領域を増やさないtoast/overlayを実装する
- [ ] store初期化、capture、restore、delete、searchの失敗を表示する
- [ ] concealed/transient拒否は必要な場合だけ静かに通知する
- [ ] VoiceOver向けaccessibility announcementを追加する

完了条件: 主要操作が失敗した理由を、表示密度を損なわず確認できる。

### アプリ統合ベンチマーク

- [ ] 100,000件DBでmenu clickから初回描画までを測る
- [ ] 連続scroll時のRSSとpage取得latencyを測る
- [ ] textのみ、画像混在、巨大payload混在を別scenarioにする
- [ ] Swift、UniFFI、decoded image cacheを含むRSSを記録する
- [ ] 結果と受け入れ基準を`docs/experiments/`へ残す

完了条件: 「100,000件を低メモリで閲覧できる」をアプリ全体で再現可能に証明する。

## P1: 日常利用に必要な機能

### ストレージmaintenance

- [ ] 定期的なpassive WAL checkpointを接続する
- [ ] idle時またはclean shutdown時にtruncate checkpointを実行する
- [ ] idle時のincremental vacuumを接続する
- [ ] 条件付きでFTS5 optimizeを実行する
- [ ] maintenanceがcapture/searchのp95を悪化させないことを測る

### retentionとpin

- [ ] 件数に加えて論理payload容量の上限を実装する
- [ ] `pinned`のRust APIとUniFFI DTOを完成させる
- [ ] pin/unpin操作をUIへ追加する
- [ ] pruningがpinned clipを削除しないことをテストする
- [ ] 上限を超えたpinned dataの扱いを決める

### 画像previewの検証

- [x] 写真、スクリーンショット、透過画像のfixtureを用意する
- [x] 96px・JPEG品質0.58の生成時間と容量分布を測る
- [x] 100,000画像時のDB容量を推定・測定する
- [x] SQLite BLOBとCAS分離を比較し、現時点ではSQLite BLOB継続と判断する
- [x] 既存画像は一括backfillせず、再copy時に補完する方針を決める
- [ ] alphaを持つ画像だけPNG previewへ切り替え、dark panel上の白背景化を防ぐ

測定結果: [PoC 0004](docs/experiments/0004-image-preview-results.md)

### キーボード操作

- [ ] 検索欄で↓を押すと履歴tableへ移動する
- [ ] ⌘1〜9による即時復元を検討・実装する
- [ ] Escape、Return、Delete、上下移動の回帰テストを追加する
- [ ] keyboardだけで全操作を完結できるようにする

### global shortcut

- [ ] shortcut登録方式と権限要件を決める
- [ ] global shortcutから既存のpanel toggleを呼ぶ
- [ ] 他アプリのshortcutと衝突した場合の表示を追加する
- [ ] shortcutを設定画面から変更可能にする

### 設定画面

- [ ] 保持件数とディスク容量上限
- [ ] global shortcut
- [ ] login時の自動起動
- [ ] 履歴の一括削除
- [ ] capture除外type/application
- [ ] UI density presetまたは設定値の選択

## P2: 配布可能な製品にする

### Pasteboard互換性

- [ ] 複数Pasteboard itemのidentityと復元モデルを決める
- [ ] Finderの複数file copyへ対応する
- [ ] Safari、Chrome、Firefoxのtext/image/HTML copyを検証する
- [ ] Xcode、Terminal、Office系アプリを検証する
- [ ] 1Passwordなどconcealed dataを保存しないことを実機確認する
- [ ] unsupported representationのfallback方針を決める

### プライバシーとデータ管理

- [ ] 保存データが平文であることを明示する
- [ ] Application Support dataをbackup対象外にするか決める
- [ ] 「すべて削除」がDB、WAL、CAS、previewを回収することを検証する
- [ ] application別・type別の除外ruleを追加する
- [ ] retention設定の初期値を決める

### macOS配布

- [ ] Xcode projectを作成する
- [ ] Rust libraryをstatic linkへ切り替える
- [ ] app icon、bundle identifier、versioningを正式化する
- [ ] Hardened Runtimeとcode signingを設定する
- [ ] notarizationとstaplingを自動化する
- [ ] release artifactと更新方式を決める

### Accessibilityと品質

- [ ] VoiceOver label、role、selection stateを検証する
- [ ] Reduce TransparencyとIncrease Contrastへ対応する
- [ ] 複数display、menu bar位置、full screen Spaceを検証する
- [ ] light/dark appearanceを検証する
- [ ] AppKit UI testと実Pasteboard integration testを追加する
- [ ] schema migrationを実DB fixtureで回帰テストする

### CI

- [x] `cargo fmt --check`
- [x] `cargo clippy --workspace --all-targets`
- [x] `cargo test --workspace`
- [x] Swift/UniFFIセルフテストを含むmacOS build
- [x] query plan回帰テスト
- [ ] release buildのartifact検査

## P3: 初期スコープ外

以下はP0〜P2が完了するまで実装しません。

- [ ] OCR
- [ ] 曖昧検索
- [ ] クラウド同期
- [ ] 複数Mac間同期
- [ ] plugin system
- [ ] clipboard内容の自動分類・要約

## 完了済みの基盤

- [x] Rust 3クレート構成
- [x] SQLite + FTS5 trigram
- [x] BLAKE3 identityと`recopy = touch`
- [x] payload CASとcrash consistency PoC
- [x] 遅延GCと孤児scan PoC
- [x] 100,000件retention
- [x] 250,000操作のsoak test
- [x] Swift/AppKit + UniFFI application shell
- [x] Pasteboard二段階capture filter
- [x] 1クリックで開くmenu bar panel
- [x] text/image別row height
- [x] 非同期画像previewと上限付きcache
- [x] UI寸法のconfiguration集約
