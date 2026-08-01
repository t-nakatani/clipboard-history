# ADR 0007: 画像previewを原payloadから分離する

## Status

Accepted

## Context

画像履歴を一覧内で識別できるようにする必要がある。一覧表示時に原画像representationを読む方式は、数十MBの画像が複数あるだけでI/Oと一時memoryが急増し、「10万〜100万件を低memoryで保持する」という価値に反する。一方、thumbnail bytesを全件の`ClipSummary`へ含めると、menuを開くだけで不要なBLOBをFFI越しに複製する。

## Decision

- Swiftのcapture pathで原画像bytesが既にmemory上にある間に、ImageIOで最大96pxのJPEG thumbnailを1回だけ生成する。生成はstore用serial queueで行い、AppKit main threadを止めない。
- previewは最大64KiBとし、storeでも上限を検証する。
- previewはcanonical identityおよびpasteboardへ戻すrepresentationから除外する。
- SQLiteの`clip_previews` tableが`clip_id`に対して0..1件のpreviewを所有する。clip削除時はforeign key cascadeで同時に削除する。
- `ClipSummary`には`has_image_preview`だけを含め、preview bytesは専用APIで表示対象の行だけ取得する。
- Swiftはpreviewを非同期取得し、最大64件・概算4MiBの`NSCache`に保持する。原画像payloadは一覧表示のために読み出さない。
- 同一内容の再copy時にpreviewが渡された場合は既存clipへupsertする。旧schemaから移行した画像も再copyすればpreviewが補完される。

## Consequences

- menuを開く初期pathは従来どおり小さいsummary 50件だけで完了する。
- thumbnail生成はcapture時に一度だけ発生するが、その時点では原画像がSwift側に既に存在するため追加のpayload readはない。
- 画像10万件ではthumbnailのdisk容量も無視できないため、96px・64KiBを上限とする。実際の分布は今後の画像PoCで測定する。
- 既存画像は再copyされるまでplaceholder iconを表示する。自動backfillのために原payloadを一括decodeする処理は設けない。
