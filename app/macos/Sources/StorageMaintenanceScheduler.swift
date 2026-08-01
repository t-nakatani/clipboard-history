import Foundation

/// Schedules background storage work from the AppKit run loop.
///
/// The scheduler only decides *when* to ask for maintenance. Thresholds and
/// the actual SQLite work remain owned by the Rust store actor.
final class StorageMaintenanceScheduler {
    struct Configuration {
        let periodicInterval: TimeInterval
        let idleAfter: TimeInterval
        let deepIdleAfter: TimeInterval

        static let standard = Configuration(
            periodicInterval: 45,
            idleAfter: 15,
            deepIdleAfter: 180
        )
    }

    typealias Handler = (MaintenanceTriggerDto, @escaping () -> Void) -> Void

    private let configuration: Configuration
    private var timer: Timer?
    private var lastActivity = Date()
    private var requestInFlight = false
    private var handler: Handler?
    /// Incremented by every `start`/`stop` so the completion of a request issued
    /// before a restart cannot clear the in-flight flag of the current one.
    private var generation = 0

    init(configuration: Configuration = .standard) {
        self.configuration = configuration
    }

    func start(handler: @escaping Handler) {
        stop()
        self.handler = handler
        lastActivity = Date()
        let timer = Timer(
            timeInterval: configuration.periodicInterval,
            repeats: true
        ) { [weak self] _ in
            self?.tick()
        }
        // Background upkeep has no deadline, so let macOS coalesce this timer
        // with others instead of waking the CPU on its own.
        timer.tolerance = configuration.periodicInterval / 4
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        handler = nil
        requestInFlight = false
        generation &+= 1
    }

    func markActivity() {
        lastActivity = Date()
    }

    private func tick(now: Date = Date()) {
        guard !requestInFlight, let handler else { return }
        let idleDuration = now.timeIntervalSince(lastActivity)
        let trigger: MaintenanceTriggerDto
        if idleDuration >= configuration.deepIdleAfter {
            trigger = .deepIdle
        } else if idleDuration >= configuration.idleAfter {
            trigger = .idle
        } else {
            trigger = .periodic
        }

        requestInFlight = true
        let issued = generation
        handler(trigger) { [weak self] in
            guard let self, self.generation == issued else { return }
            self.requestInFlight = false
        }
    }
}
