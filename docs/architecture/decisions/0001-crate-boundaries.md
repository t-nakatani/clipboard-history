# ADR 0001: Rustを3クレートに限定する

## Status

Accepted

## Context

初期案では、ドメイン型、アプリケーションサービス、検索、SQLite、payload CAS、macOS Pasteboard、FFIを個別クレートに分けていた。しかし、この規模のローカルアプリでは境界が先回りしすぎるうえ、SQLite rowと外部payloadのcrash consistencyという最重要の不変条件が複数クレートに分断される。

## Decision

Rustは次の3クレートだけで開始する。

| Crate | Responsibility |
|---|---|
| `clipboard-core` | ドメイン型、ポート、`HistoryService`、`QueryPlanner`、`Normalizer`、`CaptureFilter`、クリップ同一性の定義 |
| `clipboard-store` | SQLite/FTS5、migration、paging、retention、payload CAS、参照管理、GC、crash recovery |
| `clipboard-ffi` | UniFFI facade、Swift向けDTO、ドメイン型との変換。同期APIはSwiftの専用serial queueから呼ぶ |

依存方向は以下とする。

```text
clipboard-ffi   -> clipboard-core
clipboard-store -> clipboard-core
```

`clipboard-core`はストレージ実装やFFIを知らない。`clipboard-store`はcoreが定義するポートを実装する。

検索の分類は純粋ロジックなので`clipboard-core::query`に置く。FTS5 SQL、trigram設定、query plan検証はスキーマと不可分なので`clipboard-store::search`に置く。

payload CASは`clipboard-store::payload`に置く。ファイルのstage、fsync、rename、row commit、孤児回収、削除時の参照管理を同一クレートが所有する。

`rusqlite::Connection`は共有せず、`clipboard-store`の専用スレッドがwrite connectionを所有するactorとして実装する。UI検索はgeneration ID付きmessageで受け、古い結果を破棄できるようにする。読み書き競合が測定上問題になった場合は、WAL上でwrite connection 1本とread connection 1本に分けるが、接続所有権は引き続きstore内部に閉じる。

Pasteboard監視と復元はSwift/AppKit側へ置く。Swiftは最初にpayloadを読まずtype identifier一覧だけを`clipboard-ffi`経由で`clipboard-core::CaptureFilter`へ渡す。acceptされた場合だけ保存対象UTIのraw bytesを読み、スナップショットとしてFFIへ渡す。marker UTIは保存representationやidentityへ混入させない。無視判定という純粋なポリシーは引き続き`clipboard-core`が所有する。詳細はADR 0004に定める。

## Consequences

- SQLiteスキーマという暗黙インターフェースがクレートを跨がない。
- crash consistencyをstore単独で設計・試験できる。
- Objective-C FFIはSwift/AppKit側に閉じ、Rustのunsafe面積を増やさない。
- 独立した型クレートは作らない。循環依存が実際に生じた場合のみ再検討する。
- 将来分割するときは、測定可能な独立性と安定したインターフェースが確認できたモジュールだけを切り出す。
