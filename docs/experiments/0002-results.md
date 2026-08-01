# PoC 0002 Results: cold-open・WAL・vacuum・payload

## Environment

- Measured: 2026-08-01
- macOS 14.4.1, arm64
- Rust 1.88.0
- SQLite page size: 4KiB
- Rows: 100,000 synthetic text clips
- Cold definition: fresh process and SQLite connection; OS page cache was not forcibly purged
- Raw performance output: `experiments/results/storage-engine-20260801T093047Z/`
- Raw crash-recovery output: `experiments/results/crash-recovery-20260801T0932.txt`

## Outcome

10万件DBのprocess-cold open、検索、継続write、pruning、payload分離は技術的に成立する。測定から、初期運用値をcache 1MiB、warm-upなし、inline上限16KiB、prune batch 100とする。

同時に2つの実装上の落とし穴を検出した。`auto_vacuum=INCREMENTAL`はPRAGMA設定だけではDB headerへ定着せず、初期migrationで`VACUUM`が必要だった。また大量削除後はincremental vacuumだけではFTS5の削除済みsegmentが残り、FTS `optimize`後に初めて十分なpageがfreelistへ移動した。

## Cold open and cache size

| Cache | RSS after workload | Recent p95 | Prefix p95 | Substring p95 | First prefix |
|---:|---:|---:|---:|---:|---:|
| 1MiB | 8.2MB | 59µs | 7.53ms | 0.47ms | 15.97ms |
| 4MiB | 9.1MB | 40µs | 6.27ms | 0.39ms | 7.03ms |
| 16MiB | 22.9MB | 37µs | 6.12ms | 0.39ms | 7.07ms |

1MiBは反復runで4MiBより約1〜5MB少ないRSSとなり、全queryが受け入れ基準内に収まった。メモリ最小化を価値とするため1MiBを採用する。より大きいcacheは設定として将来提供できるが、初期値にはしない。

recent warm-upはfirst recentを約76〜81µsから43〜50µsへ短縮しただけで、prefix/substringを改善しなかった。数十µsのために起動処理を追加しない。

query planは次を維持した。

- prefix: `idx_clips_text_prefix`を利用。ただしorderのためtemporary B-treeを利用する。
- substring: FTS5 virtual table indexを利用する。

`quick_check`は約0.37〜0.40秒で、open自体の約0.4〜0.9msより数百倍重い。同期起動pathから外し、unclean shutdown時またはbackgroundで行う。

## WAL checkpoint starvation

10,000 touch writeはreaderなし、長寿命readerありの両方で約31msだった。しかしreaderが古いsnapshotを保持するとWALは4.26MBまで増え、passive checkpointは1,035 frame中0 frameしか回収できなかった。readerを閉じた直後のtruncate checkpointでWALは0 byteへ戻った。

したがって、read transactionをquery単位で閉じる規約は必須である。通常pathではUI sessionや検索generationを跨ぐreaderを作らない。

## Pruning, FTS optimize, and vacuum

100,000件からpinned rowを保護して10,000件へ削減した。

| Batch | Total prune | Delete p50 | Delete p95 | Delete p99 |
|---:|---:|---:|---:|---:|
| 100 | 2.05s | 1.33ms | 5.62ms | 21.78ms |
| 250 | 1.88s | 3.01ms | 21.91ms | 23.77ms |
| 500 | 1.79s | 5.77ms | 25.18ms | 26.15ms |
| 1,000 | 1.46s | 10.65ms | 32.64ms | 34.68ms |

batch 100は総時間が少し増えるがtail latencyが大幅に小さいため採用する。

削除直後のDBは約81MBで縮まらず、freelistは約3,400 pageだった。FTS `optimize`は0.43〜0.56秒かかったが、freelistを約17,600 pageへ増やした。その後のincremental vacuumを小さく分割して約0.3秒で完了させ、DBを約9.2〜9.6MBまで縮小できた。FTS optimizeは操作同期pathではなく、充電中・idle時などの稀なmaintenanceとする。

## Overflow pages and inline threshold

100 rowの単純tableでpayload列を最後に置いて測定した。

| Payload/row | Overflow pages | Metadata read p95 | Payload read p95 |
|---:|---:|---:|---:|
| 512B | 0 | 9µs | 7µs |
| 2KiB | 0 | 4µs | 4µs |
| 16KiB | 400 | 5µs | 6µs |
| 128KiB | 3,200 | 4µs | 23µs |
| 1MiB | 25,600 | 4µs | 154µs |

overflow chainが増えてもmetadata readは悪化しなかった。長い列をrow末尾に置く方針は有効である。一方、payload readはsizeに比例するため大容量値はCASへ逃がす。

CASの新規durable write p95は2KiB〜1MiBで約8.5〜12.7msで、sizeよりfsync costが支配的だった。dedup hitは2KiBで約7µs、16KiBで34µs、128KiBで212µs、1MiBで1.14msだった。小payloadをCASへ出すとinode数とfsync回数だけが増えるため、初期inline閾値を16KiBとする。

## Decisions

- SQLite cache default: 1MiB
- Startup warm-up: no
- Search text cap: 16KiBを維持
- Representation inline threshold: 16KiB
- Prune batch: 100 rows
- FTS optimize: rare idle maintenance
- Incremental vacuum: small background steps after optimize
- Quick check: unclean-shutdown/background only
- Long-lived read transaction: forbidden
- Bloom filter: unnecessary

## Crash consistency and garbage collection

各caseを別processで実行し、exit code 86でRust destructorを実行せず停止させた。次processでstartup orphan recoveryとqueued tombstone GCを分けて実行した。

| Stop point | Recovery result |
|---|---|
| staged temp file | stage fileを1件回収。DB rowなし |
| atomic rename後、row commit前 | orphan payloadを1件回収。DB rowなし |
| row commit後 | clip、representation、payload row、fileを各1件保持 |
| delete+tombstone commit後、file削除前 | queueを1件処理し、payload fileとmetadataを回収 |
| file削除後、DB cleanup前 | missing fileを正常扱いし、payload metadataとqueueを回収 |

全caseでdangling representationは0件だった。GCを再実行しても同じ最終状態へ収束する。共有payloadについてもunit testを追加し、最後のrepresentationが消えるまでfileを削除しないことを確認した。

全payload directory走査は通常のtombstone GCから分離した。起動時または明示的なrecoveryでだけorphan scanを行い、通常削除ではqueue上のhashだけを処理する。

## Remaining validation

- 実clipboard分布を使った検索text上限とinline閾値の再評価
- 数日相当のcapture/delete/checkpoint loop
- Swift/AppKitを含むapplication RSS
- 数万payload fileが存在する場合のstartup orphan scan時間
