# ADR 0003: SQLiteと外部payloadの運用契約を固定する

## Status

Accepted for experimentation

## Context

10万件の履歴を低メモリで扱うには、SQLiteの論理スキーマだけでなく、page、WAL、cache、freelist、外部payloadの物理的な振る舞いまで設計対象にする必要がある。外部payload CASは、索引と値を分離するWiscKey型のvalue separationとして扱う。

## Payload value separation

大きいrepresentationはSQLiteにpayload hashとmetadataだけを置き、encoded bytesはCASへ保存する。永続化順序は次の規約から変えない。

1. 同一filesystem上の一時ファイルへstreamする。
2. ファイルをflush、`fsync`する。
3. hash pathへatomic renameし、必要なら親directoryも同期する。
4. SQLite transactionでclip、representation、payload参照をcommitする。

この順序により、プロセス停止後に起こりうる不整合を「参照されない孤児ファイル」へ限定する。存在しないpayloadを指すrowは作らない。重複hashが既に存在する場合は既存ファイルを再利用する。

削除時はpayloadを同期削除しない。row削除とGC tombstone enqueueを同じ短いtransactionで確定し、背景GCが参照を再確認してから物理ファイルを消す。物理削除後・metadata cleanup前に停止しても、次回GCがmissing fileを正常な中間状態として処理する。

全directoryを走査する起動時孤児recoveryと、hash queueだけを見る通常のtombstone GCは別commandとする。通常削除のたびにpayload全件を走査しない。共有payloadは最後のrepresentation参照が消えるまで削除しない。

## Row and overflow policy

- `id INTEGER PRIMARY KEY`の単調増加を維持し、UUIDv4をclustered primary keyとして使わない。
- metadata、固定長hash、時刻、flags、sizeを先に置き、長いTEXT/BLOBをrowの最後へ置く。
- `normalized_text`はnullableかつ初期上限16KiBとし、原文payloadとは分離する。
- inline/CAS閾値は初期値16KiBとする。2KiBまではoverflow pageが発生せず、16KiBから発生したが、長い列をrow末尾に置く限りmetadata readは悪化しなかった。一方、CASのdurable writeにはpayload sizeによらず約8〜13msのfsync costがあるため、小さいpayloadを外部化しない。実データ分布で再調整する。
- exact hash lookupは既に十分速いため、in-memory Bloom filterは導入しない。

## WAL and connection ownership

`clipboard-store`の専用actorがconnectionを所有する。read transactionは必ず1 query内で閉じ、UI sessionや検索generationを跨いで保持しない。長寿命readerによるcheckpoint starvationを防ぎ、WALサイズを有界に保つ。SQLite page cacheの初期値は1MiBとする。10万件で4MiBとの差は検索p95約1〜2msだった一方、反復runでRSSを約1〜5MiB削減できた。

継続運用ではpassive checkpointを定期実行し、物理WAL fileのtruncateはidleまたはclean shutdownに限定する。250,000操作でWAL fileは最大6.55MBに留まり、RSS増加は約0.41MBだった。

通常運用はWAL + `synchronous=NORMAL`とする。この設定の耐久性契約は「OSまたは電源障害時に直近数件を失う可能性を許容するが、正常にcommit済みの履歴を壊さない」である。最後の1件を必ず残すことより、capture latencyと書き込み量を優先する。

## Retention and free pages

保持制限は件数と論理payload容量の両方に適用し、pinned rowを保護する。pruningは100 rowずつのtransactionへ分割し、interactive writeのtail latencyを抑える。削除pageはfreelistへ戻るだけなので、新規DBでは`auto_vacuum=INCREMENTAL`を有効化し、低優先度で`incremental_vacuum`を実行する。大量prune後はFTS5の`optimize`が必要だが約0.5秒かかるため、idle maintenanceとして稀に実行する。full `VACUUM`は採用しない。

## Secure deletion

履歴にはpasswordやtokenが含まれうるため、削除は論理削除で終わらせない。全connectionで`PRAGMA secure_delete=ON`を設定し、`clips`、`representations`、`clip_previews`、FTS5索引の解放cellをSQLiteに零クリアさせる。secure_deleteはconnection単位の設定なので、`configure_connection`で書き込み前に必ず適用する。

secure_delete導入前に作られたDBはfree pageに平文を残している。schema version 3への一度きりのmigrationでfull `VACUUM`とWAL truncateを実行してファイルを再構築する。定常運用でfull VACUUMを採用しない方針は変えず、この経路だけを例外とする。`user_version`のbumpが再実行を防ぐ。

外部payloadは`unlink`前にファイル本体を零で上書きする。GC対象のstaged一時ファイルも同じ経路を通す。

残存リスクは以下のとおりで、storeでは解消できない。

- APFSはcopy-on-writeであり、上書きが元blockではなく新blockへ着地しうる。旧blockは再利用されるまで読める可能性がある。
- 既存のAPFS snapshot、Time Machine、iCloudなどのbackupは削除前のコピーを保持し続ける。
- SSDのwear levelingとTRIM挙動により、物理媒体上の残留は制御できない。

これらを前提に、より強い保証が必要な場合はfilesystem levelの暗号化（FileVault）に依存する。

## Migration and recovery

- schema versionは`PRAGMA user_version`で管理する。
- `PRAGMA quick_check`は10万件で約0.37〜0.40秒かかるため、同期起動pathでは実行しない。unclean shutdown後または低頻度のbackground maintenanceで実行し、失敗時はDBを隔離して再構築する。
- startup orphan scanは100,000 fileで約2.5秒、追加RSS約0.7MBだった。streaming走査を維持し、unclean shutdown後にbackgroundで実行してUI起動を待たせない。
- checksum VFSは初期スコープに含めない。
- FTS5とprefix expression indexのquery planを回帰テストし、SQLite更新で索引利用が外れた場合は起動前検証またはCIで検出する。

## Consequences

storeはSQLite schema、検索SQL、CAS、GC、checkpoint、vacuumを単独所有する。ファイルリークは回復可能だがdangling referenceは許容しない。Bloom filterや全履歴cacheへメモリを使わず、read性能はdisk-backed indexと小さいpage cacheで得る。

process-cold測定ではrecent queryの事前実行はprefix/substringを改善しなかったため、起動時warm-upは行わない。
