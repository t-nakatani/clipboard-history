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
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        handler = nil
        requestInFlight = false
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
        handler(trigger) { [weak self] in
            self?.requestInFlight = false
        }
    }
}
