# Feasibility results

Measured on 2026-08-01 using an arm64 Mac running macOS 14.4.1. The release binary was compiled with Rust 1.88.0. Each row count ran in a fresh process and a fresh SQLite database.

## Outcome

The backend hypothesis is technically feasible for 100,000 text clipboard records. The revised model includes nullable search text, pin and retention fields, versioned canonical clip hashes, and recopy-as-touch semantics. Maximum RSS grew by only about 4.9 MB between the 1,000-row and 100,000-row processes.

| Metric | 1,000 | 10,000 | 100,000 | 100k target |
|---|---:|---:|---:|---:|
| Insert elapsed | 43 ms | 507 ms | 5.89 s | No interactive blocking requirement for bulk import |
| Insert throughput | 23.4k rows/s | 19.7k rows/s | 17.0k rows/s | Informational |
| Recent 100 rows p95 | 30 us | 32 us | 32 us | under 5 ms |
| Exact clip hash lookup p95 | 1 us | 2 us | 2 us | under 2 ms |
| Recopy touch p95 | 49 us | 30 us | 24 us | under 2 ms |
| Common prefix p95 | 790 us | 6.69 ms | 6.10 ms | under 20 ms |
| Common substring p95 | 394 us | 1.40 ms | 498 us | under 50 ms |
| Rare substring p95 | 21 us | 531 us | 104 us | under 50 ms |
| Maximum RSS | 7.82 MB | 11.24 MB | 12.73 MB | under 40 MB |
| SQLite database | 3.50 MB | 10.64 MB | 83.14 MB | Informational |

Latency and database values in the table come from the repeatable benchmark script. Maximum RSS was measured externally with macOS `/usr/bin/time -l`; the 100,000-row process reported approximately 12.73 MB after the query workload.

## Prefix index experiment

Removing the full `idx_clips_text(normalized_text)` B-tree reduced the 100,000-row database to about 73.97 MB, but a deliberately common prefix query regressed to 51.8 ms p95. Adding a partial expression index on `substr(normalized_text, 1, 64)` increased the database by about 9.17 MB and reduced the same worst-case prefix query to 6.1 ms p95, about 8.5 times faster.

The benchmark now fails unless `EXPLAIN QUERY PLAN` confirms both expected paths:

- substring: FTS5 virtual table index
- prefix: `idx_clips_text_prefix` expression index

## What this validates

- SQLite can remain the source of truth without loading the full history into Rust memory.
- A 4 MB SQLite page cache and disabled mmap keep memory growth bounded.
- A versioned canonical BLAKE3 hash over sorted `(UTI, raw bytes)` representations makes duplicate lookup effectively constant-time.
- `INSERT ... ON CONFLICT(content_hash) DO UPDATE` models recopy as a touch without creating a duplicate row.
- FTS5's trigram tokenizer provides exact substring search without fuzzy-search machinery.
- A bounded prefix expression index avoids storing the entire text in a second B-tree while keeping common-prefix latency predictable.
- A recent-history query returning 100 lightweight rows does not degrade at 100,000 records.
- Building the full 100,000-row searchable database takes seconds, so normal one-at-a-time capture has ample performance headroom.

## What this does not validate

- AppKit and Swift bridge memory overhead
- cold launch and first query after reopening an existing database
- real clipboard text distributions and very long text
- encoded image, HTML, RTF, and file URL payload storage
- pasteboard polling and raw UTI snapshot capture
- crash consistency between the SQLite transaction and external payload files, which will be owned entirely by `clipboard-store`
- pruning latency and payload garbage collection
- sustained one-at-a-time writes over days of runtime
- WAL growth while a reader holds an old snapshot and checkpoint recovery after it closes
- overflow-page cost and the inline/CAS threshold for long values
- freelist growth, incremental-vacuum latency, and physical file shrinkage after pruning

## Decision

Proceed with the disk-backed and page-oriented architecture and the three-crate design in ADR 0001. Do not build the full app yet. ADR 0003 fixes the storage-engine contracts: query-scoped readers, WiscKey-style payload separation, file-before-row commit, tombstone GC, incremental vacuum, and `synchronous=NORMAL` durability. The next experiment is specified in `docs/experiments/0002-cold-open-wal-vacuum.md` and covers cache-size/warm-up matrices, WAL checkpoint starvation, overflow thresholds, pruning/vacuum, and crash-injected payload recovery.
