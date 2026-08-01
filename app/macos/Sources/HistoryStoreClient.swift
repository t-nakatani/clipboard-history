import Foundation

struct PersistedCapture {
    let result: CaptureResultDto
    let recent: [ClipSummaryDto]
}

final class HistoryStoreClient {
    typealias Completion<Value> = (Result<Value, Error>) -> Void

    private let engine: ClipboardEngine
    private let queue = DispatchQueue(label: "dev.clipboard-history.store", qos: .userInitiated)

    init(fileManager: FileManager = .default) throws {
        let applicationSupport = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let root = applicationSupport.appendingPathComponent("ClipboardHistory", isDirectory: true)
        let payloads = root.appendingPathComponent("payloads", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        engine = try ClipboardEngine.open(
            databasePath: root.appendingPathComponent("history.sqlite").path,
            payloadDirectory: payloads.path
        )
    }

    func capture(
        representations: [RepresentationDto],
        copiedAtMs: Int64,
        completion: @escaping Completion<PersistedCapture>
    ) {
        queue.async { [engine] in
            let result = Result {
                // ImageIO decode stays off AppKit's main thread. The original
                // bytes are already present from pasteboard capture.
                let imagePreview = ImagePreviewGenerator.makePreview(from: representations)
                let stored = try engine.capture(
                    representations: representations,
                    imagePreview: imagePreview,
                    copiedAtMs: copiedAtMs
                )
                let recent = try engine.recent(limit: 50)
                return PersistedCapture(result: stored, recent: recent)
            }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func imagePreview(id: Int64, completion: @escaping Completion<RepresentationDto?>) {
        queue.async { [engine] in
            let result = Result { try engine.imagePreview(id: id) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func recent(limit: UInt32 = 50, completion: @escaping Completion<[ClipSummaryDto]>) {
        queue.async { [engine] in
            let result = Result { try engine.recent(limit: limit) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func delete(id: Int64, completion: @escaping Completion<Bool>) {
        queue.async { [engine] in
            let result = Result { try engine.delete(id: id) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func select(id: Int64, completion: @escaping Completion<[RepresentationDto]>) {
        queue.async { [engine] in
            let result = Result { try engine.select(id: id) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func search(
        query: String,
        mode: SearchModeDto,
        limit: UInt32 = 50,
        completion: @escaping Completion<[ClipSummaryDto]>
    ) {
        queue.async { [engine] in
            let result = Result { try engine.search(query: query, mode: mode, limit: limit) }
            DispatchQueue.main.async { completion(result) }
        }
    }
}
