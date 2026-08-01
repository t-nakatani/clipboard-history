# ADR 0009: 履歴一覧と検索にkeyset pagingを使う

## Status

Accepted

## Context

履歴は100,000件まで保持するが、従来のUIは新しい50件だけを取得していた。全summaryをSwiftへ渡すと履歴件数に比例してmemoryを消費する。深い`OFFSET`は読み飛ばしコストが増え、閲覧中のcaptureによって行位置がずれるため、重複・欠落も起こしやすい。

履歴の表示順は`last_used_at DESC, id DESC`であり、同じ内容の再copyは既存行の`last_used_at`を更新して先頭へ移動する。

## Decision

- `(last_used_at, id)`をcursorとする双方向keyset pagingをrecentと全search modeへ適用する。
- storeは`limit + 1`件を読み、余分な1件の有無から`has_more`を計算する。全件countは行わない。
- 方向は`PageDirection::{Older, Newer}`、cursorは`HistoryCursor`、結果は`HistoryPage`としてcoreが所有し、actorとUniFFIは同じ意味をDTOへ写す。
- `HistoryPage`の`has_more`は要求した方向にさらにpageがあることを表す。cursorはDTOに保持せず、Swiftが現在保持する先頭・末尾行から導出する。
- Swiftはscroll末尾または先頭の10行以内で、要求方向の次の50件を非同期取得する。
- 検索文字列またはmodeが変わったら世代番号を進め、旧cursorと遅れて届いた結果を破棄する。
- Swiftのsummary windowは最大200件とする。追加と反対側の端を解放し、表示中のclip IDとpixel offsetからscroll位置を復元する。解放した側には`hasMoreOlder`または`hasMoreNewer`を立てる。
- page結合時はclip IDで重複を除去する。
- panelが先頭を表示している場合、captureとrecopyは先頭pageでwindowをresetする。古い位置を閲覧中なら位置を維持して`hasMoreNewer`だけを立てる。delete後は現在の検索条件で先頭pageを再取得する。長寿命read transactionによるsnapshotはWAL checkpointを妨げるため使わない。

## SQL

次pageは既存の`idx_clips_recent(last_used_at DESC, id DESC)`を利用して取得する。

```sql
WHERE last_used_at < :cursor_time
   OR (last_used_at = :cursor_time AND id < :cursor_id)
ORDER BY last_used_at DESC, id DESC
LIMIT :page_size_plus_one
```

新しい方向は同じindexを逆走する。SQLでは現在の先頭より新しい行を近い順に取得し、`limit + 1`判定後にRustで反転するため、APIの返却順は常に降順になる。

```sql
WHERE last_used_at > :cursor_time
   OR (last_used_at = :cursor_time AND id > :cursor_id)
ORDER BY last_used_at ASC, id ASC
LIMIT :page_size_plus_one
```

検索では一致条件へ同じcursor predicateを追加する。検索結果の順序はrankingではなく従来どおりrecencyとする。

## Consequences

- 100,000件の最後まで、Swiftが100,000 summaryを同時保持せずに辿れる。
- recentの深いpageも`OFFSET`へ比例する読み飛ばしを行わない。
- windowから解放したpageへ、panelを開き直さず上下どちらにも戻れる。
- recent scan型の短い検索は、cursor predicateを2000行の内側へ適用し、最初の2000行を越えても次pageへ進める。
- recopyでcursorより古い行が先頭へ移動する場合も、先頭表示中は即時resetし、古い位置の閲覧中は上方向pageとして到達できる。

100,000件の実測結果は[PoC 0005](../../experiments/0005-keyset-paging-results.md)に記録する。50件×2,000 pageを最大50行保持で走査し、page p95は20µsだった。
