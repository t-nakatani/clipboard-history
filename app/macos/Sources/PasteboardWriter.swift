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
    static func restore(
        representations: [RepresentationDto],
        pasteboard: NSPasteboard = .general
    ) throws {
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
    }
}
