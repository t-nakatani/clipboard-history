import Foundation

struct PersistedCapture {
    let result: CaptureResultDto
    let recentPage: HistoryPageDto
}

/// Carries the shutdown result off the store queue. The semaphore that guards it
/// orders the write before the read, so no further synchronisation is needed.
private final class ShutdownBox {
    var result: Result<Void, Error>?
}

final class HistoryStoreClient {
    typealias Completion<Value> = (Result<Value, Error>) -> Void

    private let engine: ClipboardEngine
    private let queue = DispatchQueue(label: "dev.clipboard-history.store", qos: .userInitiated)
    let startupRecoveryRequired: Bool

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
        startupRecoveryRequired = engine.startupRecoveryRequired()
    }

    func recoverStartup(completion: @escaping Completion<StartupRecoveryDto>) {
        queue.async { [engine] in
            let result = Result { try engine.recoverStartup() }
            DispatchQueue.main.async { completion(result) }
        }
    }

    func runMaintenance(
        trigger: MaintenanceTriggerDto,
        completion: @escaping Completion<MaintenanceReportDto>
    ) {
        queue.async { [engine] in
            let result = Result { try engine.runMaintenance(trigger: trigger) }
            DispatchQueue.main.async { completion(result) }
        }
    }

    enum ShutdownOutcome {
        case completed
        case timedOut
    }

    /// Requests a clean shutdown, giving up after `timeout`.
    ///
    /// Maintenance may already own the store queue, and the Rust actor runs its
    /// commands serially, so the shutdown command can be queued behind an
    /// incremental vacuum or an FTS optimize. Abandoning the wait only costs a
    /// startup recovery pass on the next launch, which is what recovery exists
    /// for; blocking indefinitely at termination risks being killed instead.
    func shutdown(timeout: TimeInterval) throws -> ShutdownOutcome {
        let outcome = ShutdownBox()
        let semaphore = DispatchSemaphore(value: 0)
        queue.async { [engine] in
            outcome.result = Result { try engine.shutdown() }
            semaphore.signal()
        }
        guard semaphore.wait(timeout: .now() + timeout) == .success else {
            return .timedOut
        }
        try outcome.result?.get()
        return .completed
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
