# ADR 0009: 履歴一覧と検索にkeyset pagingを使う

## Status

Accepted

## Context

履歴は100,000件まで保持するが、従来のUIは新しい50件だけを取得していた。全summaryをSwiftへ渡すと履歴件数に比例してmemoryを消費する。深い`OFFSET`は読み飛ばしコストが増え、閲覧中のcaptureによって行位置がずれるため、重複・欠落も起こしやすい。

履歴の表示順は`last_used_at DESC, id DESC`であり、同じ内容の再copyは既存行の`last_used_at`を更新して先頭へ移動する。

## Decision

- `(last_used_at, id)`をcursorとするkeyset pagingをrecentと全search modeへ適用する。
- storeは`limit + 1`件を読み、余分な1件の有無から`has_more`を計算する。全件countは行わない。
- cursorは`HistoryCursor`、結果は`HistoryPage`としてcoreが所有し、actorとUniFFIは同じ意味をDTOへ写す。
- Swiftはscroll末尾の10行以内で次の50件を非同期取得する。
- 検索文字列またはmodeが変わったら世代番号を進め、旧cursorと遅れて届いた結果を破棄する。
- Swiftのsummary windowは最大200件とする。古い側へ進む際は新しい側を解放し、表示中のclip IDとpixel offsetからscroll位置を復元する。
- page結合時はclip IDで重複を除去する。
- captureとrecopyは先頭pageを再取得してwindowをresetする。delete後も現在の検索条件で先頭pageを再取得する。長寿命read transactionによるsnapshotはWAL checkpointを妨げるため使わない。

## SQL

次pageは既存の`idx_clips_recent(last_used_at DESC, id DESC)`を利用して取得する。

```sql
WHERE last_used_at < :cursor_time
   OR (last_used_at = :cursor_time AND id < :cursor_id)
ORDER BY last_used_at DESC, id DESC
LIMIT :page_size_plus_one
```

検索では一致条件へ同じcursor predicateを追加する。検索結果の順序はrankingではなく従来どおりrecencyとする。

## Consequences

- 100,000件の最後まで、Swiftが100,000 summaryを同時保持せずに辿れる。
- recentの深いpageも`OFFSET`へ比例する読み飛ばしを行わない。
- windowから解放した新しいpageへ逆向きに戻る場合は、panelを開き直すか検索を再実行して先頭から再構築する。将来、任意の逆方向navigationが必要ならreverse cursorを追加する。
- recopyでcursorより古い行が先頭へ移動する場合も、capture完了時の先頭page resetによって表示へ反映される。

100,000件の実測結果は[PoC 0005](../../experiments/0005-keyset-paging-results.md)に記録する。50件×2,000 pageを最大50行保持で走査し、page p95は20µsだった。
