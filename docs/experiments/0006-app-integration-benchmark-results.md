# PoC 0006 Results: Swift/AppKit・UniFFIを含むアプリ統合ベンチマーク

測定日: 2026-08-02（JST）

## 目的

100,000件の履歴を、Rust store単体ではなくSwift/AppKitアプリの実際の表示経路で閲覧できることを確認する。メニューバー操作からpanel初回描画まで、keyset pagingを使った連続scroll、画像previewのdecode/cache、巨大payloadを含むRSSを同一processで測定した。

## 環境

- macOS 14.4.1、Mac15,7、arm64、メモリ32GiB
- Apple Swift 5.10、Rust 1.95.0
- `HistoryStoreClient` → UniFFI `ClipboardEngine` → `StoreHandle` を使用
- page size 50、Swiftの`HistoryPageWindow`上限200件
- decoded image cacheはproductionと同じ64件・概算4MiB
- RSSは`mach_task_basic_info.resident_size`を5ms間隔でサンプリングし、Swift/AppKit・UniFFI・Rust・decoded image cacheを含めた

ベンチマークはseedとmeasureを別processに分けた。seedも`ClipboardEngine.capture`をSwiftから呼び出し、measure processではDBを再openして初期pageを読み込んだ。

## Scenario

| scenario | 内容 | 100,000件の保存容量 |
|---|---|---:|
| `text-only` | uniqueな`public.utf8-plain-text`のみ | 83,095,552 B |
| `mixed-images` | 10件に1件がtext + `public.png` + JPEG preview | 110,198,784 B |
| `huge-payload` | 100件に1件がtext + 256KiB `public.rtf` | 346,902,528 B |

巨大payloadは一覧summaryにbytesを含めず、復元操作も実行していない。したがって「大きなpayloadを持つ履歴を一覧で閲覧する」ケースのRSSを測っている。

## 受け入れ基準

| 条件 | 基準 |
|---|---:|
| 初回描画 | menu clickから初回描画のp95 150ms以下 |
| page取得 | 連続scrollのpage fetch p95 1ms以下、max 10ms以下 |
| paging完全性 | 100,000 rows、1,999回の追加page、順序違反0 |
| bounded RSS | scroll開始前からpeakまでのRSS増分16MiB以下 |

初回描画150msはpanelのAppKit layout・font・visible cell生成を含む。page fetchはSwift completionがmain queueへ戻るまで、RSSはDBを再openして初期pageを表示した後からscroll終了までを対象にした。

## 結果

| scenario | initial page fetch | menu click→draw p95 | page fetch p50 / p95 / max | scroll elapsed | rows / order violations | RSS before→peak / delta |
|---|---:|---:|---:|---:|---:|---:|
| text-only | 4.557ms | 133.219ms | 0.307 / 0.508 / 7.775ms | 9.390s | 100,000 / 0 | 35.9→40.2MiB / 4.3MiB |
| mixed-images | 4.941ms | 133.719ms | 0.314 / 0.621 / 5.531ms | 9.012s | 100,000 / 0 | 36.1→43.4MiB / 7.3MiB |
| huge-payload | 5.264ms | 135.251ms | 0.380 / 0.619 / 4.927ms | 11.464s | 100,000 / 0 | 36.1→40.8MiB / 4.7MiB |

全scenarioで受け入れ基準を満たした。特に346,902,528 Bの巨大payloadを保存したケースでも、scroll中のRSS増分は4,898,816 Bに留まった。pageにはpayload bytesを載せず、Swift側も200 summaryだけを保持する設計が、アプリ全体で再現できている。

mixed-imagesでは、visible rowの表示に伴って2,003件のpreviewをdecodeした。source preview bytesの累計は3,152,722 Bで、cacheの上限は64件・4MiBのままだった。画像を含むケースでもRSS増分は7.3MiBであり、100,000件に比例してdecoded imageを保持しなかった。

## 実装と測定経路

`experiments/app-integration-benchmark/main.swift`はproductionの`HistoryPanel`、`HistoryCellView`、`HistoryPageWindow`、`HistoryStoreClient`を直接コンパイルして使う。初期pageと追加pageはUniFFI経由で取得し、各pageを`HistoryPageWindow.appendOlder`へ渡してからtableを再描画する。表示された画像だけを`imagePreview` APIから読み、productionと同じ`NSCache`制限でdecodeする。

現在の実行環境では`NSStatusBar.system.statusItem()`がmenu bar serviceへ接続できないため、ベンチマークのclick injectionには同じ`NSButton` actionと`HistoryPanel.toggle/present`経路を使うstatus-button surrogateを置いた。panelの位置計算、layout、visible cell生成、draw probeはproduction実装を通る。実際のstatus item event dispatch自体は測定対象外だが、TODOの初回描画とアプリ全体RSSの条件は維持している。

## 再現方法

```sh
./experiments/run-app-integration-benchmark.sh
```

既定値は100,000 rows、全2,000 page、menu click 5回で、3 scenarioのseed/measure結果を`experiments/results/app-integration-benchmark-<timestamp>/`へ出力する。DBとpayloadは一時ディレクトリへ作成し、結果ファイルを残して削除する。

短いsmoke testは次のように実行できる。

```sh
APP_INTEGRATION_BENCHMARK_ROWS=100 \
APP_INTEGRATION_BENCHMARK_SCROLL_PAGES=2 \
APP_INTEGRATION_BENCHMARK_MENU_RUNS=2 \
./experiments/run-app-integration-benchmark.sh
```

macOSのGUI sessionが必要である。`Connection invalid`や`XType: Using static font registry`のstderrログは、この実行環境のLaunchServices/UI serviceに関する警告であり、3 scenarioのcompletion、page完全性、RSS計測には影響しなかった。
