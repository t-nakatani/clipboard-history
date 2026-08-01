# PoC 0004: 画像previewの速度・容量・忠実度

測定日: 2026-08-01

## 結論

96px・JPEG品質0.58という現在の設定は、速度と不透明画像の容量について採用可能である。3種類を各200回生成したp95は4.54〜8.44ms、previewは1.1〜2.5KiBに収まり、64KiB上限には十分な余裕があった。

100,000件すべてが画像という極端なDBを実際に作成すると、previewによるSQLite増分は約261.4MiBだった。previewを個別CAS fileへ分離すると小さいfileを100,000個管理することになり、4KiB allocationを仮定した実体だけでも約390.6MiBになる。このため、現時点では`clip_previews`のSQLite BLOBを維持する。

ただし、透過PNGが無条件にJPEGへ変換され、alphaを失う問題を確認した。dark panelでは白い矩形背景として見えるため、alphaを持つ入力だけPNG previewにする変更が必要である。同じ透過fixtureを96px PNGにすると5,625 bytesで、64KiB上限内だった。

## 検証対象

productionの[ImagePreviewGenerator.swift](../../app/macos/Sources/ImagePreviewGenerator.swift)をPoCから直接コンパイルして使用した。別実装による測定ではない。

決定論的に生成する1,440×900のfixtureは次の3種類である。

| fixture | 特性 | 入力形式 |
|---|---|---|
| photo-like | gradientと細かなnoiseを含む写真風画像 | PNG |
| screenshot | 高contrast、細線、UI風の反復要素 | PNG |
| transparent | 完全透明領域と半透明図形 | PNG |

測定環境はmacOS 14.4.1、Swift 5.10、arm64。各fixtureを1回warm-upした後に200回生成した。PSNRは、原画像とpreviewをpreview解像度へ描画し、白背景上のRGBで比較した。

## 生成結果

| fixture | preview | p50 | p95 | max | PSNR | alpha |
|---|---:|---:|---:|---:|---:|---|
| photo-like | 1,065 B | 6.79 ms | 8.14 ms | 9.79 ms | 46.11 dB | 不要・なし |
| screenshot | 2,511 B | 4.07 ms | 4.54 ms | 5.33 ms | 24.04 dB | 不要・なし |
| transparent | 1,879 B | 4.81 ms | 8.44 ms | 13.17 ms | 30.50 dB | **必要だが消失** |

全previewは96×60だった。写真風fixtureの再現性は高い。スクリーンショットは細線を96pxへ縮小するためPSNRが低いが、一覧で内容を識別する用途では視認できた。透過fixtureのPSNRは白背景へ合成した値であり、alpha消失を表現しない点に注意する。

## 100,000件DB実測

3種類のpreviewを順番に割り当て、production schemaへ100,000 clipを投入した。clipだけのDBをcheckpointしたサイズと、preview追加後のサイズを同一DBで比較した。

| 項目 | 結果 |
|---|---:|
| 論理preview bytes | 181,832,580 B（約173.4MiB） |
| clipだけのDB | 14,897,152 B（約14.2MiB） |
| previewによるDB増分 | 274,071,552 B（約261.4MiB） |
| preview込みDB全体 | 288,968,704 B（約275.6MiB） |
| preview増分 / clip | 約2,740.7 B |
| 論理previewに対する増幅率 | 1.507倍 |

画像比率が10%なら、この分布におけるpreview増分は約26.1MiBである。summaryにはBLOBを含めず、表示対象だけを非同期取得し、Swift側cacheも約4MiBに制限しているため、このdisk容量は履歴件数に比例した常駐memory増加にはならない。

## 判定

| 条件 | 判定 |
|---|---|
| 最長辺96px | pass |
| 64KiB以下 | pass |
| 生成p95 10ms以下 | pass |
| 100,000画像のpreview増分300MiB以下 | pass（約261.4MiB） |
| 透明度を維持する | **fail** |

性能・容量の設計は維持する。次の実装変更は、thumbnailのalpha有無を判定し、不透明ならJPEG品質0.58、alphaありならPNGを選ぶ方式とする。preview UTIは既にDBとUniFFIを通して保持されるため、schema変更は不要である。

## 再現方法

```sh
./experiments/run-image-preview-poc.sh
```

既定では生成を各200回、SQLiteを100,000件で測定する。短いsmoke testでは環境変数で件数を下げられる。

```sh
IMAGE_PREVIEW_ITERATIONS=5 IMAGE_PREVIEW_STORAGE_ROWS=1000 \
  ./experiments/run-image-preview-poc.sh
```

生成物は`experiments/results/image-preview-<timestamp>/`へ置かれ、Git管理対象には含めない。
