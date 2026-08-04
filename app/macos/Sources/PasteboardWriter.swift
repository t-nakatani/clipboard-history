import AppKit
import Foundation

enum PasteboardRestoreError: LocalizedError {
    case empty
    case noWritableRepresentation
    case writeFailed

    var errorDescription: String? {
        switch self {
        case .empty:
            return "復元するrepresentationがありません。"
        case .noWritableRepresentation:
            return "安全に復元できるrepresentationがありません。"
        case .writeFailed:
            return "NSPasteboardへの書き込みに失敗しました。"
        }
    }
}

/// The pasteboard operations a restore performs.
///
/// Narrow on purpose: the general pasteboard is shared with every other process
/// on the machine, and the self-test needs to stand in an implementation that
/// changes it while a restore is in flight.
protocol PasteboardWriteTarget: AnyObject {
    func clearContents() -> Int
    func writeObjects(_ objects: [any NSPasteboardWriting]) -> Bool
}

extension NSPasteboard: PasteboardWriteTarget {}

enum PasteboardWriter {
    /// Puts a stored clip back on the pasteboard and reports the `changeCount`
    /// the write produced.
    ///
    /// The count is what lets the monitor tell this write apart from a copy the
    /// user made: writing is indistinguishable from any other pasteboard change
    /// once it has happened, so the one moment it can be identified is here.
    @discardableResult
    static func restore(
        representations: [RepresentationDto],
        pasteboard: any PasteboardWriteTarget = NSPasteboard.general
    ) throws -> Int {
        guard !representations.isEmpty else { throw PasteboardRestoreError.empty }

        let item = NSPasteboardItem()
        var wroteRepresentation = false
        for representation in representations {
            // Stored data should never contain marker types. Keep the output boundary defensive.
            guard evaluateCaptureTypes(pasteboardTypes: [representation.uti]) == .accept else {
                continue
            }
            let type = NSPasteboard.PasteboardType(representation.uti)
            wroteRepresentation = item.setData(representation.bytes, forType: type) || wroteRepresentation
        }
        guard wroteRepresentation else { throw PasteboardRestoreError.noWritableRepresentation }

        // `clearContents` is what moves the count, and it hands back the value it
        // moved to; the write that follows lands on that same count. Reading
        // `pasteboard.changeCount` back instead would report whatever the shared
        // pasteboard holds by then, so a write from another process landing in
        // between would be the count that got acknowledged — and the monitor
        // would skip the user's copy, which is the race this return value exists
        // to close.
        let changeCount = pasteboard.clearContents()
        // Only a write that landed is ours to claim; a failure throws instead so
        // nothing is acknowledged.
        guard pasteboard.writeObjects([item]) else { throw PasteboardRestoreError.writeFailed }
        return changeCount
    }
}
