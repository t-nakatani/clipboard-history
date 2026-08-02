# ADR 0008: 履歴panelの表示密度をcontent種別ごとに変える

## Status

Accepted

## Context

clipboard historyでは、装飾や常設操作よりも「一度に目視できる履歴数」が選択速度へ直接効く。textは1行で識別できる一方、imageは小さい正方形iconでは内容を判別しにくい。全rowを同じ高さにすると、textに余白を使うかimageを小さくするかのどちらかになる。

MaccyのUIは画面高の大部分を使い、textとimageで異なるrow heightを採用している。この密度をUX上の参照とする。

## Decision

- panelは開くたびにstatus itemが存在するscreenの`visibleFrame`を測り、高さを92%へ設定する。
- 現在の標準値はtext/file/previewなしのrowを14pt、image previewを持つrowを82ptとする。値は設定から変更可能にする。
- textは14pt medium、補助metadataは11.5ptとする。
- image previewは最大108pt幅でaspect ratioを保って表示する。画像だけのclipでは不要な`[image]` labelを隠す。
- 固定headerとfooter actionを履歴領域から外す。上部はapp名、検索欄、検索modeだけにする。
- 選択はpointerに追従させ、単一clickまたはReturnで復元する。削除は検索欄が空のときのDelete/Backspaceに割り当て、終了はstatus itemの右click menuに置く。
- 上記の寸法・font・margin・panel appearanceは`HistoryPanelConfiguration.standard`へ集約し、panel、table、cellは同じimmutable設定値を受け取る。

## Consequences

- 同じ画面で従来より多くのtext履歴を同時に確認できる。
- image rowはtext rowより高さを使うが、previewの内容を目視して選べる。
- statusやdetail messageはpanel内の固定領域を占有しない。将来通知が必要な場合は一時的なoverlayまたはaccessibility announcementを使う。
