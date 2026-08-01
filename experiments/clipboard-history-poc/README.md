# Clipboard history feasibility PoC

This experiment tests the backend properties that matter before building the application:

- 1,000, 10,000, and 100,000 text clipboard records
- bounded SQLite page cache with memory mapping disabled
- BLAKE3 fingerprints for constant-time duplicate lookup
- a versioned canonical clip identity over sorted `(UTI, raw bytes)` representations
- recopy-as-touch using SQLite upsert semantics
- FTS5 trigram indexing for exact substring search, not fuzzy search
- a partial 64-character expression index for predictable prefix search
- recent-history paging that returns 100 rows only
- process RSS sampled before insertion, after insertion, and after query workloads

It intentionally excludes UI, FFI, pasteboard polling, image decoding, OCR, and rich previews.

## Run

```sh
./run-benchmarks.sh
```

Each count runs as a separate process so RSS measurements are not contaminated by a previous scenario. Results and disposable SQLite databases are written under `results/`.

See [RESULTS.md](RESULTS.md) for the first measured feasibility run.

## Acceptance thresholds

For the 100,000-row scenario:

- recent page p95 below 5 ms
- exact fingerprint lookup p95 below 2 ms
- recopy touch p95 below 2 ms
- common prefix p95 below 20 ms
- substring search p95 below 50 ms
- post-query RSS below 40 MB for this headless Rust process
- RSS growth from 1,000 to 100,000 rows below 10 MB
