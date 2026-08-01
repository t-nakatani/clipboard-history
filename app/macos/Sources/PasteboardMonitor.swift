import AppKit
import Foundation

struct CapturedClipboardCandidate {
    let identity: String
    let representations: [RepresentationDto]

    var representationTypes: [String] {
        representations.map(\.uti)
    }

    var payloadBytes: Int {
        representations.reduce(0) { $0 + $1.bytes.count }
    }
}

final class PasteboardMonitor {
    typealias CaptureHandler = (CapturedClipboardCandidate) -> Void
    typealias RejectionHandler = (CaptureFilterDecisionDto) -> Void

    private static let storageTypes: [NSPasteboard.PasteboardType] = [
        .string,
        .html,
        .rtf,
        .png,
        .tiff,
        .fileURL,
    ]

    private let pasteboard: NSPasteboard
    private let onCapture: CaptureHandler
    private let onRejection: RejectionHandler
    private var lastChangeCount: Int
    private var timer: Timer?

    init(
        pasteboard: NSPasteboard = .general,
        onCapture: @escaping CaptureHandler,
        onRejection: @escaping RejectionHandler
    ) {
        self.pasteboard = pasteboard
        self.onCapture = onCapture
        self.onRejection = onRejection
        self.lastChangeCount = pasteboard.changeCount
    }

    func start() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func poll() {
        let changeCount = pasteboard.changeCount
        guard changeCount != lastChangeCount else { return }
        lastChangeCount = changeCount

        guard let items = pasteboard.pasteboardItems, !items.isEmpty else { return }

        // Phase 1: inspect identifiers only. Marker types never become stored representations.
        let advertisedTypes = items.flatMap { item in item.types.map(\.rawValue) }
        let decision = evaluateCaptureTypes(pasteboardTypes: advertisedTypes)
        guard decision == .accept else {
            onRejection(decision)
            return
        }

        // Phase 2: only an accepted item may copy whitelisted payload bytes across UniFFI.
        // Multi-item modeling is intentionally deferred; this shell captures the first item.
        let item = items[0]
        let advertised = Set(item.types)
        let representations = Self.storageTypes.compactMap { type -> RepresentationDto? in
            guard advertised.contains(type), let data = item.data(forType: type) else { return nil }
            return RepresentationDto(uti: type.rawValue, bytes: data)
        }
        // Do not persist a mixture if the pasteboard changed between discovery and payload read.
        guard pasteboard.changeCount == changeCount else { return }
        guard !representations.isEmpty else { return }

        onCapture(CapturedClipboardCandidate(
            identity: canonicalHash(representations: representations),
            representations: representations
        ))
    }
}
