# PoC 0003 Results: sustained operations and startup orphan scan

## Environment and workload

- Measured: 2026-08-01
- macOS 14.4.1, arm64, Rust 1.88.0
- 250,000 one-at-a-time captures
- 2秒間隔換算で5.79日
- live history上限10,000件
- 上限超過後は各captureに続けて最古rowを削除
- 2,500 captureごとに32KiB external payload
- 1,000操作ごとにtombstone GC
- 5,000操作ごとにpassive WAL checkpoint
- Raw output: `experiments/results/soak-20260801T093654Z/`

## Outcome

履歴件数を10,000件へ固定した状態で250,000回のcapture/deleteを行っても、RSSとWALは継続増加しなかった。実運用の0.5 capture/sに対して約3,513 operations/sを処理しており、十分な余裕がある。

| Metric | Result |
|---|---:|
| Total elapsed | 71.17s |
| Capture p50 / p95 / p99 | 88µs / 219µs / 2.20ms |
| Capture max | 54.55ms |
| Delete p50 / p95 / p99 | 80µs / 204µs / 2.10ms |
| Delete max | 19.21ms |
| GC p95 | 190µs |
| Passive checkpoint p95 | 1.90ms |
| RSS at 25% | 7.16MB |
| RSS at 100% | 7.57MB |
| RSS growth | 0.41MB |
| Maximum observed WAL file | 6.55MB |
| Final live rows | 10,000 |
| Final GC queue | 0 |
| Final payload rows/files | 4 / 4 |

capture/deleteのmax latencyは稀なcheckpoint、filesystem scheduling、allocatorなどを含む。ただし全処理はStore actor上で行い、UI threadを同期blockしない。p99は約2.2ms以内だった。

passive checkpointはWAL内のframeを回収可能にするが、物理fileをtruncateしない。WAL fileは6.55MBで安定した。通常運用はpassive、idleまたはclean shutdownでtruncate checkpointを行う。

最終DBは25.84MB、freelistは1,692 pageだった。論理row数は安定していてもFTS segmentとfree pageは蓄積するため、PoC 0002で検証したFTS optimize + incremental vacuumをdisk budgetまたはidle条件で実行する。

## Startup orphan scan

孤児率10%のfan-out CASを作り、startup recoveryを測定した。scanは全fileをmemoryへ集めず、directory iteratorから1件ずつ処理する。

| Files | Scan time | Throughput | RSS delta | Deleted orphans |
|---:|---:|---:|---:|---:|
| 1,000 | 23.6ms | 42.4k files/s | 0.39MB | 100 |
| 10,000 | 280.0ms | 35.7k files/s | 0.38MB | 1,000 |
| 50,000 | 1.47s | 33.9k files/s | 0.46MB | 5,000 |
| 100,000 | 2.50s | 40.0k files/s | 0.70MB | 10,000 |

100,000 fileでも追加RSSは1MB未満だったため、streaming scanのメモリ特性は受け入れ可能である。一方2.5秒は同期起動には長い。orphan scanはunclean shutdown後にbackgroundで開始し、UI表示やrecent queryを待たせない。通常の削除はdirectory scanを行わず、GC queueだけを見る。

## Decisions

- Store actor + bounded historyで数日相当の継続運用は成立する。
- passive checkpointを定期実行し、WAL物理fileのtruncateはidle/clean shutdownに限定する。
- tombstone GCは1,000操作程度ごとの小batchで十分軽い。
- startup orphan recoveryはstreamingのまま維持し、backgroundで実行する。
- FTS optimize / incremental vacuumは時間ではなくdisk budgetとidle状態で起動する。

## Remaining validation

- Swift/AppKit + UniFFIを含むapplication全体RSS
- 実clipboard分布でのtext/payload size、UTI数、dedup率
- sleep/wake、force quit、macOS再起動を含む実機lifecycle
