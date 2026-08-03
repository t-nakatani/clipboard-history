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
        pasteboard: NSPasteboard = .general
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

        pasteboard.clearContents()
        guard pasteboard.writeObjects([item]) else { throw PasteboardRestoreError.writeFailed }
        return pasteboard.changeCount
    }
}
