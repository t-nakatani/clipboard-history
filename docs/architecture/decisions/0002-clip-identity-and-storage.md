# ADR 0002: クリップ同一性とpayload整合性を分離する

## Status

Accepted for experimentation

## Clip identity

`content_hash`はクリップ全体の同一性を表す。各representationを`(UTI, raw bytes)`としてUTI、次にbytesで安定ソートし、domain separator、各フィールド長、フィールド本体を順にBLAKE3へ投入する。

```text
hash(
  "clipboard-history.clip.v1\0" ||
  len(uti) || uti || len(bytes) || bytes || ...
)
```

並び順に依存せず、異なるrepresentation集合は異なるクリップとして扱う。正準化方式を変更するときはdomain separatorのversionを上げる。

個別の外部payloadファイル名に使う`payload_hash`は、そのpayload bytesだけのBLAKE3とする。`content_hash`と`payload_hash`は目的が異なり、同じキーとして扱わない。

## Recopy semantics

同じ`content_hash`を再コピーした場合は挿入失敗や新規row作成にしない。既存rowの`last_used_at`と`copy_count`を更新し、履歴先頭へ浮上させる。`first_copied_at`、pin状態、payload参照は保持する。

## Searchable text

`normalized_text`はnullableとする。画像のみ、ファイルのみなど検索可能テキストが存在しないクリップを許容する。FTS5 indexにはnullable値を渡し、検索対象がある行だけが結果になる。

検索対象テキストにはbyte上限を設け、初期値は16KiBとする。UTF-8文字境界で切り詰め、原文payload自体は失わない。この上限とinline/CAS閾値は固定観念ではなく、SQLite page sizeとoverflow pageの実測で調整する。SQLite rowではmetadataと固定長値を先に、`normalized_text`やinline bytesのような長い可変長列を最後に置く。

## Retention

`pinned`、`last_used_at`、論理payload容量をスキーマに持つ。件数上限とディスク容量上限の両方を適用し、pinされたrowは自動削除しない。

## Crash consistency owner

SQLite transactionと外部payloadの整合性は`clipboard-store`だけが所有する。予定する書き込み順序は以下。

1. payloadを一時ファイルへstreamする。
2. flush、fsync後、hash pathへatomic renameする。
3. SQLite transactionでclip、representation、payload参照をcommitする。
4. commit前にクラッシュしたファイルは起動時GCで孤児として回収する。

削除は先にDB参照の削除とGC tombstoneの追加を同じtransactionでcommitし、UIへ即座に返す。payloadの参照を再確認したうえでの物理削除は低優先度GCへ遅延する。ファイル削除失敗はリークにはなるが履歴row破損にはしない。起動時の孤児走査は、row commit前のクラッシュで残ったファイルも同じ回収経路へ載せる。
