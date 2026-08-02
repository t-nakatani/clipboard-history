import AppKit
import Darwin
import Foundation

func runBindingSelfTest() -> Int32 {
    let normalTypes = ["public.utf8-plain-text", "public.html"]
    guard evaluateCaptureTypes(pasteboardTypes: normalTypes) == .accept else {
        fputs("ordinary types were unexpectedly rejected\n", stderr)
        return 1
    }

    let concealedTypes = ["public.utf8-plain-text", "org.nspasteboard.ConcealedType"]
    guard evaluateCaptureTypes(pasteboardTypes: concealedTypes) == .rejectConcealed else {
        fputs("concealed marker was not rejected\n", stderr)
        return 1
    }

    // The marker lists come from crates/clipboard-core/src/filter.rs through UniFFI, so this
    // check cannot drift out of sync when a marker is added. Matching is case-insensitive and
    // evaluated across every advertised type.
    let concealedMarkers = concealedMarkerTypes()
    let transientMarkers = transientMarkerTypes()
    guard !concealedMarkers.isEmpty, !transientMarkers.isEmpty else {
        fputs("core exposed no capture markers\n", stderr)
        return 1
    }
    for marker in concealedMarkers {
        guard evaluateCaptureTypes(pasteboardTypes: ["public.html", marker.uppercased()]) == .rejectConcealed else {
            fputs("concealed marker \(marker) was not rejected in a mixed type list\n", stderr)
            return 1
        }
    }
    for marker in transientMarkers {
        guard evaluateCaptureTypes(pasteboardTypes: ["public.utf8-plain-text", marker.uppercased()]) == .rejectTransient else {
            fputs("transient marker \(marker) was not rejected in a mixed type list\n", stderr)
            return 1
        }
    }

    // Restore stays defensive even if a marker somehow reached storage.
    do {
        let markerRepresentation = RepresentationDto(
            uti: "org.nspasteboard.ConcealedType",
            bytes: Data("secret".utf8)
        )
        try PasteboardWriter.restore(
            representations: [markerRepresentation],
            pasteboard: NSPasteboard(name: .init("clipboard-history-self-test-\(UUID().uuidString)"))
        )
        fputs("restore wrote a marker representation back to the pasteboard\n", stderr)
        return 1
    } catch PasteboardRestoreError.noWritableRepresentation {
        // Expected: the only representation was a marker and was skipped.
    } catch {
        fputs("restore failed for an unexpected reason: \(error)\n", stderr)
        return 1
    }

    let text = RepresentationDto(uti: "public.utf8-plain-text", bytes: Data("hello".utf8))
    let html = RepresentationDto(uti: "public.html", bytes: Data("<b>hello</b>".utf8))
    guard canonicalHash(representations: [text, html]) == canonicalHash(representations: [html, text]) else {
        fputs("canonical identity changed with representation order\n", stderr)
        return 1
    }

    var pageWindow = HistoryPageWindow(maximumCount: 200)
    for pageIndex in 0 ..< 5 {
        let start = pageIndex * 50
        let items = (start ..< start + 50).map { value in
            ClipSummaryDto(
                id: Int64(value),
                kind: "text",
                lastUsedAtMs: Int64(1_000 - value),
                pinned: false,
                copyCount: 1,
                payloadSize: 1,
                preview: "item-\(value)",
                hasImagePreview: false
            )
        }
        let page = HistoryPageDto(
            items: items,
            continuationCursor: pageIndex < 4
                ? HistoryCursorDto(
                    lastUsedAtMs: Int64(1_000 - (start + 49)),
                    id: Int64(start + 49)
                )
                : nil,
            hasMore: pageIndex < 4,
            truncated: false
        )
        if pageIndex == 0 {
            pageWindow.reset(with: page)
        } else {
            pageWindow.appendOlder(page)
        }
    }
    guard
        pageWindow.rows.count == 200,
        pageWindow.rows.first?.id == 50,
        pageWindow.rows.last?.id == 249,
        !pageWindow.hasMoreOlder,
        pageWindow.hasMoreNewer
    else {
        fputs("bounded history page window did not evict the oldest loaded page\n", stderr)
        return 1
    }

    let newestItems = (0 ..< 50).map { value in
        ClipSummaryDto(
            id: Int64(value),
            kind: "text",
            lastUsedAtMs: Int64(1_000 - value),
            pinned: false,
            copyCount: 1,
            payloadSize: 1,
            preview: "item-\(value)",
            hasImagePreview: false
        )
    }
    pageWindow.prependNewer(
        HistoryPageDto(
            items: newestItems,
            continuationCursor: nil,
            hasMore: false,
            truncated: false
        )
    )
    guard
        pageWindow.rows.count == 200,
        pageWindow.rows.first?.id == 0,
        pageWindow.rows.last?.id == 199,
        !pageWindow.hasMoreNewer,
        pageWindow.hasMoreOlder
    else {
        fputs("bounded history page window could not return to newer pages\n", stderr)
        return 1
    }

    let recopied = ClipSummaryDto(
        id: 150,
        kind: "text",
        lastUsedAtMs: 2_000,
        pinned: false,
        copyCount: 2,
        payloadSize: 1,
        preview: "item-150",
        hasImagePreview: false
    )
    pageWindow.prependNewer(
        HistoryPageDto(
            items: [recopied],
            continuationCursor: nil,
            hasMore: false,
            truncated: false
        )
    )
    guard
        pageWindow.rows.count == 200,
        pageWindow.rows.first?.id == 150,
        pageWindow.rows.first?.copyCount == 2,
        pageWindow.rows.filter({ $0.id == 150 }).count == 1
    else {
        fputs("recopied row was not moved to its new position\n", stderr)
        return 1
    }

    var scanWindow = HistoryPageWindow(maximumCount: 200)
    let scanCursor = HistoryCursorDto(lastUsedAtMs: 500, id: 500)
    scanWindow.reset(
        with: HistoryPageDto(
            items: [],
            continuationCursor: scanCursor,
            hasMore: true,
            truncated: true
        )
    )
    guard scanWindow.hasMoreOlder, scanWindow.olderAnchor == scanCursor else {
        fputs("truncated empty scan lost its continuation cursor\n", stderr)
        return 1
    }

    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("clipboard-swift-self-test-\(UUID().uuidString)", isDirectory: true)
    do {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var engine: ClipboardEngine? = try ClipboardEngine.open(
            databasePath: root.appendingPathComponent("history.sqlite").path,
            payloadDirectory: root.appendingPathComponent("payloads").path
        )
        let stored = try engine!.capture(representations: [text], imagePreview: nil, copiedAtMs: 1)
        guard stored.inserted, try engine!.recent(limit: 50).count == 1 else {
            fputs("Swift could not persist and read through ClipboardEngine\n", stderr)
            return 1
        }
        guard try engine!.select(id: stored.id) == [text] else {
            fputs("Swift could not restore representations through ClipboardEngine\n", stderr)
            return 1
        }
        guard try engine!.search(query: "hell", mode: .prefix, limit: 50).count == 1,
              try engine!.search(query: "ell", mode: .substring, limit: 50).count == 1,
              try engine!.search(query: "hello", mode: .exact, limit: 50).count == 1 else {
            fputs("Swift search modes did not preserve exact semantics\n", stderr)
            return 1
        }
        guard
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: 8,
                pixelsHigh: 8,
                bitsPerSample: 8,
                samplesPerPixel: 4,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: .deviceRGB,
                bitmapFormat: [],
                bytesPerRow: 0,
                bitsPerPixel: 0
            ),
            let imageBytes = bitmap.representation(using: .png, properties: [:])
        else {
            fputs("Swift could not create image preview fixture\n", stderr)
            return 1
        }
        let imageRepresentation = RepresentationDto(uti: "public.png", bytes: imageBytes)
        guard let generatedPreview = ImagePreviewGenerator.makePreview(from: [imageRepresentation]) else {
            fputs("ImageIO could not generate bounded clipboard preview\n", stderr)
            return 1
        }
        let imageStored = try engine!.capture(
            representations: [imageRepresentation],
            imagePreview: generatedPreview,
            copiedAtMs: 2
        )
        guard
            let loadedPreview = try engine!.imagePreview(id: imageStored.id),
            loadedPreview.bytes == generatedPreview.bytes,
            try engine!.recent(limit: 50).first(where: { $0.id == imageStored.id })?.hasImagePreview == true
        else {
            fputs("Image preview did not round-trip through ClipboardEngine\n", stderr)
            return 1
        }
        guard
            try engine!.delete(id: stored.id),
            try engine!.delete(id: imageStored.id),
            try engine!.recent(limit: 50).isEmpty
        else {
            fputs("Swift could not delete through ClipboardEngine\n", stderr)
            return 1
        }
        engine = nil
        try FileManager.default.removeItem(at: root)
    } catch {
        fputs("ClipboardEngine self-test failed: \(error)\n", stderr)
        return 1
    }

    return 0
}

// MARK: - History feed

/// Answers store calls synchronously from fixtures and records what was asked.
private final class StubHistoryStore: HistoryStore {
    var recentPageResult: HistoryPageDto
    var searchPageResult: HistoryPageDto
    var captureResult: PersistedCapture?
    private(set) var recentPageCalls = 0
    private(set) var searchPageCalls = 0
    private(set) var lastDirection: PageDirectionDto?
    private(set) var lastSearchText: String?

    init(recentPageResult: HistoryPageDto, searchPageResult: HistoryPageDto) {
        self.recentPageResult = recentPageResult
        self.searchPageResult = searchPageResult
    }

    func capture(
        representations: [RepresentationDto],
        copiedAtMs: Int64,
        completion: @escaping (Result<PersistedCapture, Error>) -> Void
    ) {
        guard let captureResult else { return }
        completion(.success(captureResult))
    }

    func recentPage(
        cursor: HistoryCursorDto?,
        direction: PageDirectionDto,
        limit: UInt32,
        completion: @escaping (Result<HistoryPageDto, Error>) -> Void
    ) {
        recentPageCalls += 1
        lastDirection = direction
        completion(.success(recentPageResult))
    }

    func searchPage(
        query: String,
        mode: SearchModeDto,
        cursor: HistoryCursorDto?,
        direction: PageDirectionDto,
        limit: UInt32,
        completion: @escaping (Result<HistoryPageDto, Error>) -> Void
    ) {
        searchPageCalls += 1
        lastDirection = direction
        lastSearchText = query
        completion(.success(searchPageResult))
    }

    func delete(id: Int64, completion: @escaping (Result<Bool, Error>) -> Void) {
        completion(.success(true))
    }

    func select(id: Int64, completion: @escaping (Result<[RepresentationDto], Error>) -> Void) {
        completion(.success([]))
    }

    func imagePreview(id: Int64, completion: @escaping (Result<RepresentationDto?, Error>) -> Void) {
        completion(.success(nil))
    }
}

private func summaries(ids: [Int64]) -> [ClipSummaryDto] {
    ids.map { id in
        ClipSummaryDto(
            id: id,
            kind: "text",
            lastUsedAtMs: id,
            pinned: false,
            copyCount: 1,
            payloadSize: 1,
            preview: "item-\(id)",
            hasImagePreview: false
        )
    }
}

private func fixturePage(ids: [Int64], hasMore: Bool = false) -> HistoryPageDto {
    HistoryPageDto(
        items: summaries(ids: ids),
        continuationCursor: hasMore ? ids.last.map { HistoryCursorDto(lastUsedAtMs: $0, id: $0) } : nil,
        hasMore: hasMore,
        truncated: false
    )
}

private func isNewerAvailable(_ status: HistoryStatus?) -> Bool {
    if case .newerAvailable = status { return true }
    return false
}

/// Exercises the refresh policy that used to live inside AppDelegate's capture
/// completion, where it could only be checked by hand against a running panel.
func runHistoryFeedSelfTest() -> Int32 {
    let store = StubHistoryStore(
        recentPageResult: fixturePage(ids: [3, 2, 1]),
        searchPageResult: fixturePage(ids: [2])
    )
    var activityCount = 0
    let feed = HistoryFeedModel(store: store, onStoreActivity: { activityCount += 1 })
    var statuses: [HistoryStatus] = []
    feed.statusDidChange = { statuses.append($0) }

    feed.reload()
    guard feed.rows.map(\.id) == [3, 2, 1] else {
        fputs("feed did not load the recent page\n", stderr)
        return 1
    }
    guard activityCount > 0 else {
        fputs("feed did not report store activity to the maintenance scheduler\n", stderr)
        return 1
    }

    let candidate = CapturedClipboardCandidate(
        identity: "0123456789abcdef",
        representations: [RepresentationDto(uti: "public.utf8-plain-text", bytes: Data("hi".utf8))]
    )

    // At the newest edge a capture takes over the rows.
    feed.shouldHoldNewestUpdate = { false }
    store.captureResult = PersistedCapture(
        result: CaptureResultDto(id: 4, inserted: true),
        recentPage: fixturePage(ids: [4, 3, 2, 1])
    )
    feed.capture(candidate)
    guard feed.rows.map(\.id) == [4, 3, 2, 1] else {
        fputs("capture did not refresh the rows while viewing the newest clip\n", stderr)
        return 1
    }

    // Reading older rows, the same capture must not move anything.
    feed.shouldHoldNewestUpdate = { true }
    store.captureResult = PersistedCapture(
        result: CaptureResultDto(id: 5, inserted: true),
        recentPage: fixturePage(ids: [5, 4, 3, 2, 1])
    )
    statuses.removeAll()
    feed.capture(candidate)
    guard feed.rows.map(\.id) == [4, 3, 2, 1], feed.hasMoreNewer else {
        fputs("capture disturbed the rows while the reader was away from the newest clip\n", stderr)
        return 1
    }
    guard isNewerAvailable(statuses.last) else {
        fputs("holding back a capture did not announce that newer history exists\n", stderr)
        return 1
    }

    // While a search is live, a capture re-runs the search instead of jumping
    // back to the recent feed.
    feed.search(text: "item", mode: .substring)
    guard feed.rows.map(\.id) == [2] else {
        fputs("search did not replace the rows with its results\n", stderr)
        return 1
    }
    feed.shouldHoldNewestUpdate = { false }
    let searchCallsBeforeCapture = store.searchPageCalls
    let recentCallsBeforeCapture = store.recentPageCalls
    feed.capture(candidate)
    guard
        store.searchPageCalls == searchCallsBeforeCapture + 1,
        store.recentPageCalls == recentCallsBeforeCapture,
        feed.rows.map(\.id) == [2]
    else {
        fputs("capture during a search abandoned the search results\n", stderr)
        return 1
    }

    // Paging stays inside the live query and asks in the requested direction.
    store.searchPageResult = fixturePage(ids: [1], hasMore: true)
    feed.search(text: "item", mode: .substring)
    let searchCallsBeforePaging = store.searchPageCalls
    feed.loadPage(.older)
    guard
        store.searchPageCalls == searchCallsBeforePaging + 1,
        store.lastDirection == .older
    else {
        fputs("paging a search fell back to the recent feed\n", stderr)
        return 1
    }

    // The row-update bracket has to wrap the change: the view reads the rows it
    // is showing before `apply` and the replacements after, which is what lets
    // it put the reader back where they were.
    let bracketStore = StubHistoryStore(
        recentPageResult: fixturePage(ids: [2, 1]),
        searchPageResult: fixturePage(ids: [])
    )
    let bracketFeed = HistoryFeedModel(store: bracketStore, onStoreActivity: {})
    var bracketObservations: [[Int64]] = []
    bracketFeed.updateRows = { [weak bracketFeed] _, apply in
        bracketObservations.append(bracketFeed?.rows.map(\.id) ?? [])
        apply()
        bracketObservations.append(bracketFeed?.rows.map(\.id) ?? [])
    }
    bracketFeed.reload()
    guard bracketObservations == [[], [2, 1]] else {
        fputs("row update bracket did not wrap the change to the resident rows\n", stderr)
        return 1
    }

    // The panel is usable before the store opens, so a restore attempted then
    // has to come back as a failure rather than drop the completion and leave
    // the caller showing progress forever.
    let detachedFeed = HistoryFeedModel(onStoreActivity: {})
    var detachedRestore: Result<[RepresentationDto], Error>?
    detachedFeed.representations(for: 1) { detachedRestore = $0 }
    guard let detachedRestore, case .failure = detachedRestore else {
        fputs("a restore before the store opened did not complete\n", stderr)
        return 1
    }

    return 0
}

func runSelfTest() -> Int32 {
    let stages: [(String, () -> Int32)] = [
        ("bindings", runBindingSelfTest),
        ("history feed", runHistoryFeedSelfTest),
    ]
    for (name, stage) in stages {
        let status = stage()
        guard status == 0 else {
            fputs("\(name) self-test failed\n", stderr)
            return status
        }
    }
    print("Swift/UniFFI self-test passed")
    return 0
}
