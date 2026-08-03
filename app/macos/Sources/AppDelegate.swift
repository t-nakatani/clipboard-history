import AppKit
import Foundation

/// Application lifecycle and wiring.
///
/// Everything about the history itself lives elsewhere: `HistoryFeedModel` owns
/// the query and the store, `HistoryListController` owns the panel contents.
/// What remains here is the menu bar item, the panel, startup and shutdown.
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// macOS force-terminates an app that lingers in `applicationWillTerminate`.
    private static let shutdownTimeout: TimeInterval = 2

    private let uiConfiguration = HistoryPanelConfiguration.standard
    private let maintenanceScheduler = StorageMaintenanceScheduler()
    private var statusItem: NSStatusItem?
    private var monitor: PasteboardMonitor?
    private var storeClient: HistoryStoreClient?
    private var panel: HistoryPanel?
    private var listController: HistoryListController?
    private var feed: HistoryFeedModel?

    private lazy var statusView = HistoryStatusView(configuration: uiConfiguration)

    func applicationDidFinishLaunching(_ notification: Notification) {
        configureStatusItem()

        // The panel is built before the store opens so the menu bar item stays
        // usable throughout startup, and still responds if the store never
        // opens at all. The feed simply has nowhere to send requests until then.
        let feed = HistoryFeedModel(
            onStoreActivity: { [weak self] in self?.maintenanceScheduler.markActivity() }
        )
        feed.statusDidChange = { [weak self] status in self?.show(status) }
        self.feed = feed

        configurePanel(feed: feed, previewLoader: ImagePreviewLoader(fetch: feed.imagePreview))

        show(.preparingStorage)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result { try HistoryStoreClient() }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case let .success(client):
                    self.start(storeClient: client, feed: feed)
                case let .failure(error):
                    self.show(.failed(error))
                }
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        feed?.cancelPendingSearch()
        maintenanceScheduler.stop()
        monitor?.stop()
        guard let storeClient else { return }
        do {
            // Stopping the scheduler prevents new requests but cannot cancel one
            // already running, so bound the wait rather than hold the main thread.
            if try storeClient.shutdown(timeout: Self.shutdownTimeout) == .timedOut {
                NSLog("Clipboard History clean shutdown timed out; the next launch will run startup recovery")
            }
        } catch {
            NSLog("Clipboard History clean shutdown failed: %@", error.localizedDescription)
        }
    }

    // MARK: - Startup

    private func start(storeClient: HistoryStoreClient, feed: HistoryFeedModel) {
        self.storeClient = storeClient
        feed.attach(store: storeClient)

        maintenanceScheduler.start { [weak self] trigger, finished in
            guard let storeClient = self?.storeClient else {
                finished()
                return
            }
            storeClient.runMaintenance(trigger: trigger) { result in
                if case let .failure(error) = result {
                    NSLog("Clipboard History maintenance failed: %@", error.localizedDescription)
                }
                finished()
            }
        }

        monitor = PasteboardMonitor(
            onCapture: { [weak feed] candidate in feed?.capture(candidate) },
            onRejection: { [weak self] decision in self?.show(.rejected(decision)) }
        )
        monitor?.start()

        guard storeClient.startupRecoveryRequired else {
            show(.idle)
            feed.reload()
            return
        }
        recoverStartup(storeClient: storeClient, feed: feed)
    }

    /// Repairs the store before the first read.
    ///
    /// Recovery can quarantine and rebuild the database, so reading history
    /// first would either surface an error from a store that is about to be
    /// repaired, or race the recovery outcome against the loaded page for
    /// ownership of the status labels.
    private func recoverStartup(storeClient: HistoryStoreClient, feed: HistoryFeedModel) {
        show(.recovering)
        storeClient.recoverStartup { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(report):
                if report.databaseRebuilt {
                    NSLog(
                        "Clipboard History rebuilt a corrupted database; previous history quarantined at %@",
                        report.quarantinePath ?? "(unknown path)"
                    )
                    self.show(.recoveryRebuilt(quarantinePath: report.quarantinePath))
                } else {
                    let reclaimed = report.garbageCollection.payloadFilesDeleted
                        + report.garbageCollection.orphanFilesDeleted
                        + report.garbageCollection.stagedFilesDeleted
                    NSLog("Clipboard History startup recovery reclaimed %llu orphaned files", reclaimed)
                    self.show(.recoveryCompleted(reclaimedFiles: reclaimed))
                }
            case let .failure(error):
                // Rust keeps the recovery marker so the next launch retries. The
                // connection may still be usable, so load rather than leave the
                // panel empty; a broken store fails again in the load below.
                NSLog("Clipboard History startup recovery failed: %@", error.localizedDescription)
                self.show(.recoveryFailed(error.localizedDescription))
            }
            // A completed recovery carries no headline, so the load owns it,
            // and the outcome is on the detail line the load is asked to leave
            // alone. A rebuild or a failure is important and owns both lines;
            // the load's routine status cannot displace it. Either way the
            // preview comes back with the next thing the user does.
            feed.reload(keepingDetail: true)
        }
    }

    // MARK: - Menu bar item

    private func configureStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "clipboard", accessibilityDescription: "Clipboard History")
        item.button?.target = self
        item.button?.action = #selector(performStatusItemClick)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        statusItem = item
    }

    @objc private func performStatusItemClick() {
        guard let event = NSApplication.shared.currentEvent else {
            togglePanel()
            return
        }
        if event.type == .rightMouseUp, let button = statusItem?.button {
            NSMenu.popUpContextMenu(makeStatusItemContextMenu(), with: event, for: button)
            return
        }
        togglePanel()
    }

    private func makeStatusItemContextMenu() -> NSMenu {
        let menu = NSMenu()
        let quitItem = NSMenuItem(title: "終了", action: #selector(terminate), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)
        return menu
    }

    @objc private func terminate() {
        NSApplication.shared.terminate(nil)
    }

    // MARK: - Panel

    private func togglePanel() {
        guard let panel, let listController, let button = statusItem?.button else { return }
        feed?.markActivity()
        panel.toggle(relativeTo: button, firstResponder: listController.searchField)
        if panel.isVisible {
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    private func configurePanel(feed: HistoryFeedModel, previewLoader: ImagePreviewLoader) {
        let panel = HistoryPanel(configuration: uiConfiguration)
        panel.visibilityDidChange = { [weak self] visible in
            self?.statusItem?.button?.highlight(visible)
            // The row is in front of the user now, so an important status has
            // been delivered and routine updates may take it back over.
            if visible { self?.statusView.markSeen() }
        }
        statusView.isOnScreen = { [weak panel] in panel?.isVisible ?? false }

        let listController = HistoryListController(
            configuration: uiConfiguration,
            feed: feed,
            previewLoader: previewLoader,
            statusView: statusView,
            contentFrame: panel.contentRect(forFrameRect: panel.frame)
        )
        listController.statusDidChange = { [weak self] status in self?.show(status) }
        listController.dismissPanel = { [weak panel] in panel?.dismiss() }
        // The monitor does not exist until the store opens, and neither does
        // restoring: the feed has no representations to hand out before then.
        listController.didWriteToPasteboard = { [weak self] changeCount in
            self?.monitor?.acknowledgeSelfWrite(changeCount: changeCount)
        }
        panel.contentView = listController.contentView

        self.panel = panel
        self.listController = listController
    }

    // MARK: - Status presentation

    private func show(_ status: HistoryStatus) {
        // The row lives in a panel the user may never open, so a storage
        // failure is also written where it can be found afterwards. The other
        // important cases log at the point they are detected.
        if case let .failed(error) = status {
            NSLog("Clipboard History storage error: %@", error.localizedDescription)
        }
        statusView.show(status)
    }
}
