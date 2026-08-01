# PoC 0005: 100,000件keyset paging

測定日: 2026-08-01

## 目的

`(last_used_at, id)` cursorで100,000件の最後まで欠落なく辿れ、深いpageでも保持件数とlatencyが増えないことを確認する。

## 条件

- productionと同じSQLite schemaと`idx_clips_recent`
- 100,000 clips
- page size 50
- cursor条件: `last_used_at < ? OR (last_used_at = ? AND id < ?)`
- 各pageで直前行より厳密に降順であることを検査

## 結果

```text
rows_seen=100000
pages=2000
max_page_rows=50
page_p95_us=20
elapsed_ms=34.781
```

全100,000件を2,000 pageで一度ずつ読み、重複・順序違反は発生しなかった。Rust側が同時に保持した行は最大50件で、page位置による保持件数増加はない。

Swift側は最大200 summaryの`HistoryPageWindow`を使い、5 page目の追加時に最も新しい50件を解放すること、そこから新しいpageをprependして元の方向へ戻れることをセルフテストしている。画像bytesはsummary pageに含まれず、従来どおり表示対象だけを別APIで読む。

production testでは、同一timestampと途中のcapture/deleteを含む3 pageを古い方向へ進んだ後、新しい方向へ2 page戻して同じ行列が得られることを検証した。`EXPLAIN QUERY PLAN`では`< / DESC`と`> / ASC`の両方が`idx_clips_recent`を利用する。また、新しい2,000件がすべて不一致で最古側の1件だけが一致する2,105件の短い部分一致検索を使い、空のscan windowが`truncated + continuation_cursor`を返して次のwindowへ継続できることを確認した。

## 再現方法

```sh
cargo run --release -p clipboard-store --example storage_engine_poc -- \
  seed /tmp/keyset.sqlite 100000

cargo run --release -p clipboard-store --example keyset_paging_poc -- \
  /tmp/keyset.sqlite 50
```
