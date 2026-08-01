# macOS application shell

Swift/AppKitとRust/UniFFIの境界を検証する最小アプリです。`ClipboardEngine`を通してstore actorへ接続され、capture、recent、recopy=touch、delete、payload GC、representationの復元までを呼び出せます。UIは最近50件だけを一覧表示し、ダブルクリックまたはReturnで選択履歴をNSPasteboardへ戻せます。

captureは必ず2段階で行います。

1. `NSPasteboardItem.types`からtype identifierだけを集め、payloadを読む前にRustの`CaptureFilter`へ渡す。
2. acceptされた場合だけ、保存対象のUTIをwhitelistし、そのbytesを読み取ってUniFFI DTOへ変換する。

concealed/transient markerは保存representationにもcanonical identityにも入りません。現在は複数itemのうち先頭だけを保存候補にします。複数itemのidentityモデルは別途決定します。

SQLiteとpayloadは`~/Library/Application Support/ClipboardHistory/`へ保存します。Swiftの`HistoryStoreClient`が専用serial queueから同期UniFFI APIを呼ぶため、AppKit main threadはSQLiteやfilesystem I/Oを待ちません。Swiftが保持するrecent結果は最大50件です。

検索欄は完全一致、前方一致、正確な部分一致の3 modeだけを持ちます。入力は120ms debounceされ、古いgenerationの結果はUIへ反映しません。3文字未満の完全一致・部分一致は最新2,000件に限定したscanへ落とし、履歴全体を走査しません。

画像はcapture時にImageIOで最大96pxのJPEG previewをstore用serial queue上で生成します。previewは原representation、canonical identity、復元payloadから独立しており、最大64KiBです。一覧summaryにはpreview bytesを入れず、表示対象だけを専用APIで非同期取得します。decoded image cacheは最大64件・概算4MiBに制限されます。

menu bar iconの左クリックはメニューを挟まず、履歴panelを直接toggleします。panelはタイトルバーのない`.nonactivatingPanel`で、status itemの直下へ画面端を考慮して配置されます。背景はHUD blurと薄いindigo tint、前景は独立した不透明content viewに分離しています。これにより壁紙を透過させながら文字のcontrastを維持します。panel高は対象screenの表示可能領域の92%です。現在の設定はtext row 14pt、画像row 82pt、本文font 14ptで、固定header/footerを置かず履歴の目視数を優先します。開いた時点でメモリ上の最近50件が一覧表示され、検索欄へfocusします。外部clickまたはEscapeで閉じ、Returnで復元、Delete/Backspaceで削除します。右クリックだけが終了用context menuを表示します。

UI寸法と外観は`Sources/HistoryPanelConfiguration.swift`の`standard`へ集約しています。調整可能な項目はwindow幅・初期高・最小高・画面高比率、panel内margin、section間隔、検索欄間隔、text/image row高、row間隔、thumbnail幅・上下余白、textとmetadataの間隔、各font size、角丸、border、blur濃度、tintです。`AppDelegate`、`HistoryPanel`、`HistoryCellView`へ数値を重複して持たせません。

```sh
./app/macos/build-macos-app.sh
open ./app/macos/build/ClipboardHistory.app
```

build scriptはRustの`staticlib`と`cdylib`を生成し、`cdylib`からSwift bindingsを抽出します。アプリのリンク検証には同じ`cdylib`をbundle内へ配置しています。Xcode projectとstatic linkへの切り替えはUI実装開始時に行います。
