# ADR 0005: 同期UniFFI facadeをSwiftの直列queueから呼ぶ

## Status

Accepted

## Context

`clipboard-store`は専用actor threadが`rusqlite::Connection`を単独所有し、commandとreply channelで操作する。この上に別のRust async runtimeを導入してもSQLite処理自体は同じactorへ直列化される。一方、同期UniFFIメソッドをAppKit main threadから直接呼ぶと、migration、capture、filesystem I/Oの完了までUIを停止させる。

保持上限もapplication shell接続時点で実装されていないと、100,000件という価値が単なるUI設定になる。

## Decision

- `clipboard-ffi::ClipboardEngine`をUniFFI objectとして公開し、`capture`、`recent`、`search`、`select`、`delete`の同期メソッドだけを持たせる。
- Swiftの`HistoryStoreClient`が専用serial `DispatchQueue`からengineを呼ぶ。完了callbackだけmain queueへ戻す。
- engineのopenと`quick_check`もbackground queueで行い、AppKit起動をブロックしない。
- FFIはstore内部型を公開せず、representation、capture結果、最大200件のsummary DTOだけを公開する。
- store actorは起動時にlive件数を1回だけ数え、その後はinsert/delete/prune結果から件数を更新する。履歴全体をメモリへロードしない。
- 新規insertで100,000件を超える場合、同じSQLite transaction内で古いunpinned rowを削除する。削除triggerはpayloadをGC queueへ積み、capture reply後に物理payloadを回収する。
- `select`だけが保存representationをmaterializeし、合計64MiBを超えるclipは復元前に拒否する。通常の一覧取得ではpayloadを読まない。
- 検索はexact、prefix、literal substringだけを許可する。Swiftで120ms debounceし、generationが古い結果を破棄する。3文字未満のexact/substringは最新2,000件だけをscanする。

## Consequences

- AppKit main threadはSQLite、fsync、GCを待たない。
- SQLite connectionの所有権とcommand順序はstore actorに保たれる。
- Swiftが保持するrecent一覧は50件、FFIが許可する上限は200件になる。
- captureとretention row削除は原子的だが、外部payload回収は意図的に遅延する。回収失敗時もdurable queueが残る。
- Pasteboardへ復元した内容はmonitorが通常のcopyとして再検知し、recopy=touchにより先頭へ移動する。
- prefix expression indexとFTS5 virtual indexの`EXPLAIN QUERY PLAN`を回帰テストする。LIKE wildcardはescapeし、ユーザー入力を常にliteralとして扱う。
- 将来、検索キャンセルや並行read connectionが必要になった時点でasync UniFFI methodを再検討する。
