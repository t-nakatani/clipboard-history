# TODO

最終更新: 2026-08-03

このファイルは、Clipboard Historyを技術PoCから日常利用可能なmacOSアプリへ進めるための残タスクを管理します。

## P0: プロジェクトの価値を成立させる

### 自己再captureの抑止

- [ ] 履歴復元によるPasteboard更新をmonitorが再保存しない仕組みを決める
- [ ] change countまたはcanonical identityで次の1回だけ抑止する
- [ ] 通常の再copyは従来どおり`recopy = touch`になることをテストする

完了条件: 履歴を復元しても`copy_count`と並び順が意図せず変化しない。

### ユーザーへ見えるエラー表示

このアプリの価値は「コピーしたものは全部ここにある」という信頼です。守るべきは、保存されなかったのにされたと思い込んでいる状態を作らないこと。ユーザーはコピー元を閉じた後にそれを発見します。ここがP0である理由はその一点で、見た目の丁寧さではありません。

土台は実装済みです。panel下部に固定高1行のstatus rowがあり（`HistoryStatusView`）、状態は`HistoryStatus` enumで表して文言はview層に置いています。store初期化、capture、restore、delete、search、pagingの失敗はすでに`HistoryStatus.failed`へ流れ、`Priority.important`はpanelが閉じている間に発生しても保持されて後続のroutineな更新に埋もれません。

方針: 失敗はpanelを開いた時に届けます。menu barアイコンでの常時通知や、他アプリへ割り込む通知は使いません。エラーの分類はRust、提示と文言はアプリレイヤーに閉じます。

以下は独立して着手・完了できる3つの塊です。1項目を除いて順序の依存はありません（その1項目には依存を明記しています）。

#### 開いた時に届く失敗通知

`hasUnseenImportantStatus`と`markSeen()`が機構としては既にあるため、残りは挙動の決定と調整です。

- [ ] panelを閉じている間に複数回失敗したとき、最新1件へ件数を添えて示す（現在は`show()`が上書きし、最後の1件しか残らない）
- [ ] `markSeen()`が可視になった瞬間に既読とする挙動を見直す（履歴を探すために開いてfooterを読まずに閉じると消える）
- [ ] 未読のimportantがある間はstatus rowを一時的に強調し、固定領域は増やさない
- [ ] 強調の解除条件を決める（既読、次の操作、時間経過のいずれか）
- [ ] 閉じている間の失敗が次に開いたときへ持ち越されることを、`SelfTest`の既存のclosed/openケースへ追加して回帰テストする
- [ ] アプリ再起動を跨いだ持ち越しをどうするか決める。`hasUnseenImportantStatus`はプロセス内の状態なので、気づかれないまま再起動すると未読の失敗は消える。これはこの節の出発点である「保存されたと思い込んでいる状態」そのものなので、永続化するか、しないと決めて理由を書くかのどちらかにする

完了条件: panelを閉じている間に起きた失敗が、同じ起動の中で次に開いたときへ必ず一度は届く。再起動を跨ぐ場合の扱いは上の項目で決める。

#### エラー分類をFFI境界で保つ

- [ ] `ClipboardFfiError`を`StoreError`のvariantへ対応させる（現在は`Store { message: String }`へ潰れている）
- [ ] `ActorStopped`（以降すべて失敗する。再起動が要る）と`InvalidData`（その1件だけ壊れている）をSwiftが`switch`で区別できるようにする
- [ ] Swift側はエラー文字列を判定しない。分類はRust、提示はアプリレイヤーという線引きを保つ
- [ ] 回復不能な失敗と単発の失敗で、ユーザーへ促す次の行動を変える
- [ ] 対応付けのないvariantへ落ちたときのfallbackと、原文の残し方を決める

完了条件: 再起動が必要な失敗と、その1件だけの失敗を、ユーザーが区別できる。

#### 通知の信号対雑音比

繰り返し出る通知は読まれなくなり、読まれない通知は他の項目の投資も無駄にします。何を出さないかを決める塊です。

- [ ] concealed/transient拒否を必要な場合だけに絞る（現在は拒否のたびに「保存対象外」を出す）
- [ ] 抑制ルールを決める（初回だけ、panelが開いているときだけ、など）
- [ ] サイズ超過拒否（`rejectedOversized`、important扱いで実装済み）との出し分けを明文化する
- [ ] password managerから連続でcopyしても通知が積み上がらないことをテストする
- [ ] 画像preview取得の失敗を表示する（現在は`HistoryFeedModel.imagePreview`が`try?`で捨てており、thumbnailが出ない理由が伝わらない）
- [ ] 同じ失敗が連続したときに同一通知を繰り返さない抑制を入れる（「エラー分類をFFI境界で保つ」の後。何を同じ失敗とみなすかに分類が要る。`Store { message: String }`のままだと文字列比較になり、そちらの「Swift側はエラー文字列を判定しない」と衝突する）

出さないと決めたもの:

- background maintenanceとclean shutdownの失敗は`NSLog`のままにします。ユーザーに打つ手がなく、データも失われず、shutdownの失敗は次回起動時のrecoveryが吸収するためです
- P0の時点では`HistoryStatus.failed`のdetailに`error.localizedDescription`がそのまま出るため、Rust/UniFFI由来の英語が混じります。日本語の文言への対応付けはP2のAccessibilityと品質へ置いています

完了条件: 通知の頻度が日常のcopyで邪魔にならず、出る通知には対応する行動がある（文言そのものの読みやすさはP2）。

### アプリ統合ベンチマーク

- [ ] 100,000件DBでmenu clickから初回描画までを測る
- [ ] 連続scroll時のRSSとpage取得latencyを測る
- [ ] textのみ、画像混在、巨大payload混在を別scenarioにする
- [ ] Swift、UniFFI、decoded image cacheを含むRSSを記録する
- [ ] 結果と受け入れ基準を`docs/experiments/`へ残す

完了条件: 「100,000件を低メモリで閲覧できる」をアプリ全体で再現可能に証明する。

## P1: 日常利用に必要な機能

### ストレージmaintenance

- [x] 定期的なpassive WAL checkpointを接続する
- [x] clean shutdown時にtruncate checkpointを実行する
- [x] idle時にtruncate checkpointを実行する
- [x] idle時のincremental vacuumを接続する
- [x] 条件付きでFTS5 optimizeを実行する
- [ ] maintenanceがcapture/searchのp95を悪化させないことを測る

実装: Rust store actorが`Periodic`・`Idle`・`DeepIdle`を閾値判定し、GC、WAL checkpoint、bounded incremental vacuum、削除量に応じたFTS5 optimizeを実行する。macOS側はcapture/searchの活動を基準に45秒周期でbackground maintenanceを投入する。

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
- [ ] status rowとエラー通知の更新時に`NSAccessibility.post(element:notification:.announcementRequested)`を発行する
- [ ] `HistoryStatus.Priority`をannouncementのpriorityへ対応付け、routineな更新で読み上げを溢れさせない
- [ ] `HistoryStatus.failed`のdetailを日本語の説明文へ対応付ける（現在は`error.localizedDescription`のまま、Rust/UniFFI由来の英語が出る）
- [ ] 再試行できる失敗とできない失敗を文言で区別する

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
- [x] recent/searchの双方向keyset paging
  - [x] 50件単位で上下方向へ移動し、summary保持を最大200件に制限
  - [x] scroll位置を維持したまま解放済みpageへ戻る
  - [x] bounded scanの`truncated`と`continuation_cursor`で疎な短文検索を継続
  - [x] capture、delete、recopy中の重複・欠落・並び順を回帰テスト
- [x] 起動時recovery
  - [x] clean/unclean shutdown markerとUniFFI recovery API
  - [x] unclean時だけbackgroundでquick check、queued GC、orphan scanを実行
  - [x] recovery前にrecentを読み、panel表示をscan完了で待たせない
  - [x] 破損DB・WAL/SHM・payloadをdurable manifestで隔離・再開して空storeを再構築
