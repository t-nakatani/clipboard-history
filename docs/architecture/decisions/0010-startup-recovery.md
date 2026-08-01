# ADR 0010: unclean shutdown時だけ起動リカバリーを行う

## Status

Accepted

## Context

`PRAGMA quick_check`は100,000件で約0.4秒、payload directoryの100,000 file走査は約2.5秒かかる。毎回の同期起動へ置くと、menu bar iconから履歴を即時表示するUXを損なう。一方、process killや電源断では、SQLite commit前にdurable renameされた孤児payloadや、処理途中のGC queueが残り得る。

破損DBを単に削除すると調査・手動回復の余地がなくなる。payloadだけを元のdirectoryへ残して空DBを作ると、次のorphan scanが隔離DBからしか参照できないpayloadを削除してしまう。

## Decision

- Rust storeはDBと同じ場所の`history.sqlite.running`を単独所有する。open時に原子的に作成し、clean shutdownのtruncate checkpoint完了後に削除する。
- open時にmarkerが残っていれば`startup_recovery_required`をUniFFIへ公開する。通常起動では`quick_check`を実行しない。
- Swiftはrecentの初回requestを先にserial queueへ積み、その後で`recover_startup`を実行する。panelはscan完了を待たず、読み込み済みsummaryを表示する。
- startup recoveryは`quick_check`、queued payload GC、streaming orphan scanの順で実行する。markerがなければこの処理をskipする。
- `quick_check`失敗、またはunclean open時にSQLiteが`CORRUPT`/`NOTADB`を返した場合は、接続を閉じてDB、WAL、SHM、payload directoryを同じtimestamp suffixで隔離し、新しい空storeを作る。
- 隔離データは自動削除しない。UIには隔離DBのpathを表示する。
- `recover_orphans`は診断・手動maintenance用にもUniFFIへ公開する。

## Consequences

- clean startupの同期pathから`quick_check`とdirectory scanが外れる。
- unclean startupでも最初のrecent pageを先に表示できる。recovery中に要求された追加のstore操作はserial queueで待機する。
- 破損時は履歴を空から再開するが、旧DBとpayloadを一組で保持するため手動回復の可能性を残せる。
- markerはsingle-instance applicationを前提とする。同じstoreを複数processから同時に開く用途にはfile lockを別途導入する必要がある。
