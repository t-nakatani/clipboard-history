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

    let text = RepresentationDto(uti: "public.utf8-plain-text", bytes: Data("hello".utf8))
    let html = RepresentationDto(uti: "public.html", bytes: Data("<b>hello</b>".utf8))
    guard canonicalHash(representations: [text, html]) == canonicalHash(representations: [html, text]) else {
        fputs("canonical identity changed with representation order\n", stderr)
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
