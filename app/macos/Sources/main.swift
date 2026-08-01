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

    print("Swift/UniFFI self-test passed")
    return 0
}

if CommandLine.arguments.contains("--self-test") {
    exit(runBindingSelfTest())
}

let application = NSApplication.shared
let delegate = AppDelegate()
application.delegate = delegate
application.setActivationPolicy(.accessory)
application.run()
