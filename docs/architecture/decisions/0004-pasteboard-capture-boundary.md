# ADR 0004: Pasteboard captureを型判定とpayload読取に分ける

## Status

Accepted

## Context

`org.nspasteboard.ConcealedType`や`org.nspasteboard.TransientType`は保存対象データではなく、capture可否を示すmarkerである。`CaptureFilter`がbytes込みの`ClipboardSnapshot`を受けると、Swift側が保存対象UTIだけを読む実装ではmarkerを見失う。一方、markerを空bytesのrepresentationとして渡すと保存内容とcanonical identityを汚染する。

また、大きなpayloadをUniFFI越しにコピーした後でrejectするのは、メモリとレイテンシの両面で無駄になる。

## Decision

capture境界を次の2段階に固定する。

1. Swiftは`NSPasteboardItem.types`からtype identifier一覧だけを収集する。この時点では`data(forType:)`を呼ばない。
2. 一覧をUniFFIの`evaluate_capture_types`へ渡し、`clipboard-core::CaptureFilter`で判定する。
3. rejectなら処理を終了し、payload bytesは読まない。
4. acceptならSwift側のstorage whitelistに含まれるUTIだけbytesを読み、`RepresentationDto`としてcapture facadeへ渡す。

marker UTIはstorage whitelistへ含めず、`ClipboardSnapshot`、payload CAS、canonical identityのいずれにも入れない。coreの`HistoryService::capture`は「すでにpolicyを通過した保存representation」を入力とし、marker判定を重複して行わない。

`clipboard-ffi`はmacOS applicationへのlink用に`staticlib`を、`uniffi-bindgen generate --library`がmetadataを抽出するために`cdylib`も生成する。bindings生成用binaryは`bindgen-cli` featureのときだけbuildする。

## Consequences

- concealed/transientな内容をFFIへコピーせずに破棄できる。
- capture policyはRust core、Pasteboard I/Oと保存UTI whitelistはSwift/AppKitという所有境界が明確になる。
- marker追加時はcoreのtype-list policyを変更し、保存形式やidentity versionを変更する必要がない。
- Swift側は「型一覧取得とpayload読取の間にchangeCountが変わりうる」ため、実運用版では読取後にchangeCountを再確認し、不一致なら候補を破棄する。
- 現application shellは複数pasteboard itemの先頭だけを候補にする。複数itemの正準identityは永続化接続前に別途決定する。
