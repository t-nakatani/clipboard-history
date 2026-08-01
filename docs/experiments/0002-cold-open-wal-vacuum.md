# PoC 0002: cold-open・WAL・vacuum・payload整合性

## 目的

10万件DBを既存状態から開く実運用と、数日継続する書き込み・削除で、latency、RSS、disk usage、crash consistencyが設計上の範囲に収まることを確認する。全アプリ実装には進まず、`clipboard-store`だけで再現可能な実験にする。

## A. Cold open and page cache

同一の10万件DBを毎回新しいprocessで開き、OS cacheの影響を記録したうえで次のmatrixを測る。

| cache_size | warm-upなし | recent queryでwarm-up後 |
|---:|---:|---:|
| 1 MiB | first recent / prefix / substring / p95 | 同左 |
| 4 MiB | first recent / prefix / substring / p95 | 同左 |
| 16 MiB | first recent / prefix / substring / p95 | 同左 |

各caseでopen時間、first query、steady-state p50/p95/p99、peak/steady RSS、page count、FTS5とprefixの`EXPLAIN QUERY PLAN`を保存する。warm-upの改善が小さい場合は起動処理へ追加しない。

## B. Overflow and value separation

512B、2KiB、16KiB、128KiB、1MiBのtext/BLOBを使い、SQLite page size、`dbstat`で観測できるoverflow page、point read、insert latency、DB sizeを比較する。`normalized_text`上限とinline/CAS閾値を独立に振り、検索indexとpayload readのtrade-offを測る。

受け入れ条件は、一覧取得がpayload sizeに比例して悪化せず、1MiB payloadを扱っても全payload相当のRSSを保持しないこと。初期候補は検索text上限16KiBだが、測定結果で変更してよい。

## C. WAL checkpoint starvation

1. readerなしで継続captureし、WAL sizeとcheckpoint latencyを測る。
2. 意図的にread transactionを開いたまま同じwrite workloadを流す。
3. readerを閉じ、passive/restart/truncate checkpoint後のsize推移を記録する。

実装規約ではread transactionをquery単位で閉じる。長寿命reader caseは失敗注入であり、通常pathに存在してはならない。通常caseでWALが継続増加しないことを受け入れ条件とする。

## D. Pruning and vacuum

100,000件から10,000件まで、pinned rowを残して小batchで削除する。batch sizeごとのwrite p95/p99、総時間、`freelist_count`、DB file sizeを記録する。その後`auto_vacuum=INCREMENTAL`で段階的に回収し、1回あたりlatencyと回収page数を測る。full `VACUUM`は比較対象に留める。

interactive captureを長時間止める単一transactionを避け、低優先度maintenanceでdisk budgetへ収束できることを受け入れ条件とする。

## E. Payload crash consistency and GC

次の各境界でprocessを強制停止し、再起動後のrow、file、GC queueを検査する。

1. temp write中
2. file fsync後
3. atomic rename後、row commit前
4. row commit後
5. row削除とtombstone commit後、file削除前
6. file削除中

許容する不整合はtemp fileまたは参照されない孤児fileだけである。存在しないpayloadを参照するcommitted rowは1件も許容しない。GCを再実行可能かつ冪等にし、共有payloadは参照が残る限り削除しない。

## 記録する追加契約

- migrationは`PRAGMA user_version`で再実行可能にする。
- 起動時`quick_check`失敗を注入し、隔離・再構築pathを確認する。
- `synchronous=NORMAL`では直近数件の消失を許容するが、DB破損は許容しない。
- `content_hash` indexへのrandom insertがwrite costへ与える割合を計測する。Bloom filterは比較対象にしない。
