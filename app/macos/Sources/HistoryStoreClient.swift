import Foundation

struct PersistedCapture {
    let result: CaptureResultDto
    let recentPage: HistoryPageDto
}

final class HistoryStoreClient {
    typealias Completion<Value> = (Result<Value, Error>) -> Void

    private let engine: ClipboardEngine
    private let queue = DispatchQueue(label: "dev.clipboard-history.store", qos: .userInitiated)
    let startupRecoveryRequired: Bool

    init(
        databasePath: String,
        payloadDirectory: String
    ) throws {
        engine = try ClipboardEngine.open(
            databasePath: databasePath,
            payloadDirectory: payloadDirectory
        )
        startupRecoveryRequired = engine.startupRecoveryRequired()
    }

    convenience init(fileManager: FileManager = .default) throws {
        let applicationSupport = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let root = applicationSupport.appendingPathComponent("ClipboardHistory", isDirectory: true)
        let payloads = root.appendingPathComponent("payloads", isDirectory: true)
        try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        try self.init(
            databasePath: root.appendingPathComponent("history.sqlite").path,
            payloadDirectory: payloads.path
        )
    }

    func recoverStartup(completion: @escaping Completion<StartupRecoveryDto>) {
        queue.async { [engine] in
            let result = Result { try engine.recoverStartup() }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func shutdown() throws {
        try queue.sync { try engine.shutdown() }
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
                let recentPage = try engine.recentPage(cursor: nil, direction: .older, limit: 50)
                return PersistedCapture(result: stored, recentPage: recentPage)
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

    func recentPage(
        cursor: HistoryCursorDto? = nil,
        direction: PageDirectionDto = .older,
        limit: UInt32 = 50,
        completion: @escaping Completion<HistoryPageDto>
    ) {
        queue.async { [engine] in
            let result = Result {
                try engine.recentPage(cursor: cursor, direction: direction, limit: limit)
            }
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

    func searchPage(
        query: String,
        mode: SearchModeDto,
        cursor: HistoryCursorDto? = nil,
        direction: PageDirectionDto = .older,
        limit: UInt32 = 50,
        completion: @escaping Completion<HistoryPageDto>
    ) {
        queue.async { [engine] in
            let result = Result {
                try engine.searchPage(
                    query: query,
                    mode: mode,
                    cursor: cursor,
                    direction: direction,
                    limit: limit
                )
            }
            DispatchQueue.main.async { completion(result) }
        }
    }
}
