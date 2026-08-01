import AppKit
import Darwin
import Foundation

private enum BenchmarkError: LocalizedError {
    case missingValue(String)
    case invalidValue(String)
    case unsupportedArgument(String)
    case missingPageCursor
    case noImageFixture

    var errorDescription: String? {
        switch self {
        case let .missingValue(option):
            return "missing value for \(option)"
        case let .invalidValue(value):
            return "invalid benchmark value: \(value)"
        case let .unsupportedArgument(argument):
            return "unsupported argument: \(argument)"
        case .missingPageCursor:
            return "history page reported more rows without a continuation cursor"
        case .noImageFixture:
            return "could not create image fixture"
        }
    }
}

private enum BenchmarkMode: String {
    case seed
    case measure
}

private enum BenchmarkScenario: String, CaseIterable {
    case textOnly = "text-only"
    case mixedImages = "mixed-images"
    case hugePayload = "huge-payload"

    static func parse(_ rawValue: String) throws -> BenchmarkScenario {
        guard let scenario = BenchmarkScenario(rawValue: rawValue) else {
            throw BenchmarkError.invalidValue(rawValue)
        }
        return scenario
    }
}

private struct BenchmarkArguments {
    let mode: BenchmarkMode
    let scenario: BenchmarkScenario
    let root: URL
    let rowCount: Int
    let scrollPages: Int
    let menuRuns: Int

    var databasePath: String {
        root.appendingPathComponent("history.sqlite").path
    }

    var payloadDirectory: String {
        root.appendingPathComponent("payloads", isDirectory: true).path
    }

    init(arguments: [String]) throws {
        var mode: BenchmarkMode = .measure
        var scenario: BenchmarkScenario = .textOnly
        var root: URL?
        var rowCount = 100_000
        var scrollPages = Int.max
        var menuRuns = 5

        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            switch argument {
            case "--mode":
                mode = try BenchmarkMode(rawValue: Self.value(after: argument, in: arguments, at: &index))
                    .unwrap(or: BenchmarkError.invalidValue(argument))
            case "--scenario":
                scenario = try BenchmarkScenario.parse(Self.value(after: argument, in: arguments, at: &index))
            case "--root":
                root = URL(
                    fileURLWithPath: try Self.value(after: argument, in: arguments, at: &index)
                )
            case "--rows":
                rowCount = try Self.parsePositiveInt(
                    Self.value(after: argument, in: arguments, at: &index)
                )
            case "--scroll-pages":
                scrollPages = try Self.parsePositiveInt(
                    Self.value(after: argument, in: arguments, at: &index)
                )
            case "--menu-runs":
                menuRuns = try Self.parsePositiveInt(
                    Self.value(after: argument, in: arguments, at: &index)
                )
            case "--help":
                throw BenchmarkError.invalidValue(
                    "usage: --mode seed|measure --scenario text-only|mixed-images|huge-payload "
                        + "--root PATH [--rows N] [--scroll-pages N] [--menu-runs N]"
                )
            default:
                throw BenchmarkError.unsupportedArgument(argument)
            }
            index += 1
        }

        guard let root else {
            throw BenchmarkError.missingValue("--root")
        }
        self.mode = mode
        self.scenario = scenario
        self.root = root
        self.rowCount = rowCount
        self.scrollPages = scrollPages
        self.menuRuns = menuRuns
    }

    private static func value(after option: String, in arguments: [String], at index: inout Int) throws -> String {
        let valueIndex = index + 1
        guard arguments.indices.contains(valueIndex) else {
            throw BenchmarkError.missingValue(option)
        }
        index = valueIndex
        return arguments[valueIndex]
    }

    private static func parsePositiveInt(_ rawValue: String) throws -> Int {
        guard let value = Int(rawValue), value > 0 else {
            throw BenchmarkError.invalidValue(rawValue)
        }
        return value
    }
}

private extension Optional {
    func unwrap(or error: @autoclosure () -> Error) throws -> Wrapped {
        guard let value = self else { throw error() }
        return value
    }
}

private enum BenchmarkClock {
    static func now() -> UInt64 {
        DispatchTime.now().uptimeNanoseconds
    }

    static func milliseconds(from start: UInt64, to end: UInt64) -> Double {
        Double(end - start) / 1_000_000.0
    }
}

private enum ProcessRSS {
    static func currentBytes() -> UInt64 {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(
            MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size
        )
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { rebound in
                task_info(
                    mach_task_self_,
                    task_flavor_t(MACH_TASK_BASIC_INFO),
                    rebound,
                    &count
                )
            }
        }
        guard result == KERN_SUCCESS else { return 0 }
        return UInt64(info.resident_size)
    }
}

private final class RSSSampler {
    private let lock = NSLock()
    private var peakBytes: UInt64 = 0
    private var timer: DispatchSourceTimer?

    func start() {
        record()
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
        timer.schedule(deadline: .now(), repeating: .milliseconds(5))
        timer.setEventHandler { [weak self] in
            self?.record()
        }
        timer.resume()
        self.timer = timer
    }

    func stop() -> UInt64 {
        record()
        timer?.cancel()
        timer = nil
        lock.lock()
        defer { lock.unlock() }
        return peakBytes
    }

    private func record() {
        let bytes = ProcessRSS.currentBytes()
        lock.lock()
        peakBytes = max(peakBytes, bytes)
        lock.unlock()
    }
}

private struct LatencySummary {
    let count: Int
    let p50: Double
    let p95: Double
    let p99: Double
    let maximum: Double

    init(_ values: [Double]) {
        let sorted = values.sorted()
        count = sorted.count
        guard !sorted.isEmpty else {
            p50 = 0
            p95 = 0
            p99 = 0
            maximum = 0
            return
        }
        p50 = sorted[Self.percentileIndex(count: sorted.count, percentile: 0.50)]
        p95 = sorted[Self.percentileIndex(count: sorted.count, percentile: 0.95)]
        p99 = sorted[Self.percentileIndex(count: sorted.count, percentile: 0.99)]
        maximum = sorted[sorted.count - 1]
    }

    private static func percentileIndex(count: Int, percentile: Double) -> Int {
        min(count - 1, max(0, Int(ceil(Double(count) * percentile)) - 1))
    }
}

private final class BenchmarkDrawProbeView: NSView {
    private var armed = false
    private var didDraw = false
    private var callback: (() -> Void)?

    func arm(_ callback: @escaping () -> Void) {
        self.callback = callback
        armed = true
        didDraw = false
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard armed, !didDraw else { return }
        didDraw = true
        armed = false
        callback?()
    }
}

private final class BenchmarkHistoryController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let configuration = HistoryPanelConfiguration.standard
    private let storeClient: HistoryStoreClient
    let panel: HistoryPanel
    let firstResponder: NSResponder
    private let tableView = HistoryTableView()
    private let searchField = NSSearchField()
    private weak var historyClipView: NSClipView?
    private let drawProbe = BenchmarkDrawProbeView()
    private var pageWindow = HistoryPageWindow(maximumCount: 200)
    private let imagePreviewCache: NSCache<NSNumber, NSImage> = {
        let cache = NSCache<NSNumber, NSImage>()
        cache.countLimit = 64
        cache.totalCostLimit = 4 * 1024 * 1024
        return cache
    }()
    private var pendingImagePreviews: Set<Int64> = []

    private(set) var decodedImagePreviewCount = 0
    private(set) var decodedImagePreviewBytes = 0

    var rows: [ClipSummaryDto] { pageWindow.rows }
    var hasMoreOlder: Bool { pageWindow.hasMoreOlder }
    var olderCursor: HistoryCursorDto? { pageWindow.olderAnchor }

    init(storeClient: HistoryStoreClient) {
        self.storeClient = storeClient
        panel = HistoryPanel(configuration: configuration)
        firstResponder = searchField
        super.init()
        configurePanel()
    }

    func loadInitialPage(completion: @escaping (Result<(HistoryPageDto, Double), Error>) -> Void) {
        let started = BenchmarkClock.now()
        storeClient.recentPage(limit: 50) { [weak self] result in
            let elapsed = BenchmarkClock.milliseconds(from: started, to: BenchmarkClock.now())
            guard let self else { return }
            switch result {
            case let .success(page):
                self.pageWindow.reset(with: page)
                self.tableView.reloadData()
                completion(.success((page, elapsed)))
            case let .failure(error):
                completion(.failure(error))
            }
        }
    }

    func fetchNextOlder(
        completion: @escaping (Result<(HistoryPageDto, Double), Error>) -> Void
    ) {
        guard pageWindow.hasMoreOlder, let cursor = pageWindow.olderAnchor else {
            completion(.failure(BenchmarkError.missingPageCursor))
            return
        }
        let started = BenchmarkClock.now()
        storeClient.recentPage(
            cursor: cursor,
            direction: .older,
            limit: 50
        ) { [weak self] result in
            let elapsed = BenchmarkClock.milliseconds(from: started, to: BenchmarkClock.now())
            guard let self else { return }
            switch result {
            case let .success(page):
                self.pageWindow.appendOlder(page)
                self.tableView.reloadData()
                self.scrollToNewestLoadedRows()
                completion(.success((page, elapsed)))
            case let .failure(error):
                completion(.failure(error))
            }
        }
    }

    func armFirstDraw(_ callback: @escaping () -> Void) {
        panel.orderOut(nil)
        drawProbe.arm(callback)
    }

    func displayPanelIfNeeded() {
        panel.displayIfNeeded()
        panel.contentView?.displayIfNeeded()
        tableView.displayIfNeeded()
    }

    func scrollDisplayIfNeeded() {
        panel.contentView?.layoutSubtreeIfNeeded()
        tableView.layoutSubtreeIfNeeded()
        scrollToNewestLoadedRows()
        panel.displayIfNeeded()
        panel.contentView?.displayIfNeeded()
        tableView.displayIfNeeded()
    }

    func showPanel(relativeTo button: NSButton) {
        if !panel.isVisible {
            panel.present(relativeTo: button, firstResponder: firstResponder)
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    func hidePanel() {
        panel.dismiss()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        rows[row].hasImagePreview
            ? configuration.rows.imageHeight
            : configuration.rows.textHeight
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("HistoryCell")
        let cell: HistoryCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? HistoryCellView {
            cell = reused
        } else {
            cell = HistoryCellView(configuration: configuration)
        }

        let summary = rows[row]
        cell.clipId = summary.id
        cell.configureForImagePreview(summary.hasImagePreview)
        cell.previewLabel.stringValue = (summary.preview ?? "[\(summary.kind)]")
            .replacingOccurrences(of: "\n", with: " ")
        cell.previewLabel.isHidden = summary.hasImagePreview && summary.preview == nil
        cell.metadataLabel.stringValue = "\(summary.kind) · "
            + ByteCountFormatter.string(fromByteCount: Int64(summary.payloadSize), countStyle: .file)
        cell.thumbnailImageView.image = placeholderImage(for: summary.kind)

        if summary.hasImagePreview {
            let key = NSNumber(value: summary.id)
            if let image = imagePreviewCache.object(forKey: key) {
                cell.thumbnailImageView.image = image
            } else {
                loadImagePreview(id: summary.id)
            }
        }
        cell.toolTip = "\(summary.payloadSize) bytes · copied \(summary.copyCount)回"
        return cell
    }

    private func configurePanel() {
        let panelContent = TranslucentPanelContentView(
            frame: panel.contentRect(forFrameRect: panel.frame),
            configuration: configuration
        )
        panel.contentView = panelContent

        searchField.placeholderString = "入力して検索"
        searchField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let searchModeControl = NSSegmentedControl(
            labels: ["完全", "前方", "部分"],
            trackingMode: .selectOne,
            target: nil,
            action: nil
        )
        searchModeControl.selectedSegment = 2
        searchModeControl.setContentHuggingPriority(.required, for: .horizontal)
        let appNameLabel = NSTextField(labelWithString: "Clipboard")
        appNameLabel.font = .systemFont(
            ofSize: configuration.typography.appNameSize,
            weight: configuration.typography.appNameWeight
        )
        appNameLabel.textColor = .tertiaryLabelColor
        let searchBar = NSStackView(views: [appNameLabel, searchField, searchModeControl])
        searchBar.orientation = .horizontal
        searchBar.spacing = configuration.content.searchItemSpacing

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history"))
        column.title = "履歴"
        column.resizingMask = .autoresizingMask
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.rowHeight = configuration.rows.textHeight
        tableView.intercellSpacing = NSSize(width: 0, height: configuration.rows.intercellSpacing)
        tableView.backgroundColor = .clear
        tableView.usesAlternatingRowBackgroundColors = false
        tableView.dataSource = self
        tableView.delegate = self

        let scrollView = NSScrollView()
        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.borderType = .noBorder
        scrollView.drawsBackground = false
        scrollView.scrollerStyle = .overlay
        historyClipView = scrollView.contentView

        let stack = NSStackView(views: [searchBar, scrollView])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = configuration.content.sectionSpacing
        stack.translatesAutoresizingMaskIntoConstraints = false
        panelContent.foregroundView.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(
                equalTo: panelContent.foregroundView.leadingAnchor,
                constant: configuration.content.leadingInset
            ),
            stack.trailingAnchor.constraint(
                equalTo: panelContent.foregroundView.trailingAnchor,
                constant: -configuration.content.trailingInset
            ),
            stack.topAnchor.constraint(
                equalTo: panelContent.foregroundView.topAnchor,
                constant: configuration.content.topInset
            ),
            stack.bottomAnchor.constraint(
                equalTo: panelContent.foregroundView.bottomAnchor,
                constant: -configuration.content.bottomInset
            ),
            searchBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.heightAnchor.constraint(
                greaterThanOrEqualToConstant: configuration.content.minimumHistoryHeight
            ),
        ])

        drawProbe.translatesAutoresizingMaskIntoConstraints = false
        drawProbe.isHidden = false
        panelContent.foregroundView.addSubview(drawProbe)
        NSLayoutConstraint.activate([
            drawProbe.leadingAnchor.constraint(equalTo: panelContent.foregroundView.leadingAnchor),
            drawProbe.trailingAnchor.constraint(equalTo: panelContent.foregroundView.trailingAnchor),
            drawProbe.topAnchor.constraint(equalTo: panelContent.foregroundView.topAnchor),
            drawProbe.bottomAnchor.constraint(equalTo: panelContent.foregroundView.bottomAnchor),
        ])
    }

    private func scrollToNewestLoadedRows() {
        guard let historyClipView else { return }
        tableView.layoutSubtreeIfNeeded()
        let documentHeight = tableView.frame.height
        let viewportHeight = historyClipView.bounds.height
        let y = max(0, documentHeight - viewportHeight)
        historyClipView.setBoundsOrigin(NSPoint(x: 0, y: y))
        historyClipView.enclosingScrollView?.reflectScrolledClipView(historyClipView)
    }

    private func placeholderImage(for kind: String) -> NSImage? {
        let symbolName: String
        switch kind {
        case "image": symbolName = "photo"
        case "file": symbolName = "doc"
        case "mixed": symbolName = "square.stack.3d.up"
        default: symbolName = "doc.text"
        }
        return NSImage(systemSymbolName: symbolName, accessibilityDescription: kind)
    }

    private func loadImagePreview(id: Int64) {
        guard pendingImagePreviews.insert(id).inserted else { return }
        storeClient.imagePreview(id: id) { [weak self] result in
            guard let self else { return }
            self.pendingImagePreviews.remove(id)
            guard
                case let .success(preview?) = result,
                let image = NSImage(data: preview.bytes)
            else {
                return
            }
            self.decodedImagePreviewCount += 1
            self.decodedImagePreviewBytes += preview.bytes.count
            self.imagePreviewCache.setObject(
                image,
                forKey: NSNumber(value: id),
                cost: 96 * 96 * 4
            )
            guard let row = self.rows.firstIndex(where: { $0.id == id }) else { return }
            self.tableView.reloadData(
                forRowIndexes: IndexSet(integer: row),
                columnIndexes: IndexSet(integer: 0)
            )
        }
    }
}

private final class BenchmarkApplicationDelegate: NSObject, NSApplicationDelegate {
    private let arguments: BenchmarkArguments
    private var storeClient: HistoryStoreClient?
    private var controller: BenchmarkHistoryController?
    private var menuButton: NSButton?
    private var menuHostWindow: NSWindow?
    private var rssSampler: RSSSampler?
    private var isFinishing = false
    private var didStart = false
    private(set) var exitCode: Int32?

    init(arguments: BenchmarkArguments) {
        self.arguments = arguments
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        start()
    }

    func start() {
        guard !didStart else { return }
        didStart = true
        if arguments.mode == .seed {
            startSeeding()
        } else {
            startMeasurement()
        }
    }

    private func startSeeding() {
        do {
            let summary = try seedDatabase(arguments)
            emit("mode=seed")
            emit("scenario=\(summary.scenario)")
            emit("rows=\(summary.rows)")
            emit("storage_bytes=\(summary.storageBytes)")
            finish(exitCode: 0)
        } catch {
            fputs("seed failed: \(error)\n", stderr)
            finish(exitCode: 1)
        }
    }

    private func startMeasurement() {
        let sampler = RSSSampler()
        sampler.start()
        rssSampler = sampler
        do {
            let client = try HistoryStoreClient(
                databasePath: arguments.databasePath,
                payloadDirectory: arguments.payloadDirectory
            )
            storeClient = client
            configureBenchmarkUI(client: client)
        } catch {
            fputs("measure open failed: \(error)\n", stderr)
            finish(exitCode: 1)
        }
    }

    private func configureBenchmarkUI(client: HistoryStoreClient) {
        let controller = BenchmarkHistoryController(storeClient: client)
        self.controller = controller

        let hostFrame = NSRect(x: 0, y: 0, width: 28, height: 28)
        let hostWindow = NSWindow(
            contentRect: hostFrame,
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        hostWindow.isOpaque = false
        hostWindow.backgroundColor = .clear
        hostWindow.ignoresMouseEvents = true
        hostWindow.level = .statusBar
        hostWindow.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let button = NSButton(frame: hostFrame)
        button.title = "Clipboard benchmark"
        button.image = NSImage(
            systemSymbolName: "clipboard",
            accessibilityDescription: "Clipboard History benchmark"
        )
        button.target = self
        button.action = #selector(menuItemClicked)
        button.isBordered = false
        hostWindow.contentView = button
        let visibleFrame = NSScreen.screens.first?.visibleFrame ?? NSRect(x: 0, y: 0, width: 800, height: 600)
        hostWindow.setFrameOrigin(
            NSPoint(x: visibleFrame.midX - hostFrame.width / 2, y: visibleFrame.maxY - hostFrame.height)
        )
        hostWindow.orderFrontRegardless()
        menuHostWindow = hostWindow
        menuButton = button

        controller.loadInitialPage { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success((page, latency)):
                self.runMenuMeasurements(
                    initialPage: page,
                    initialPageLatencyMs: latency,
                    samples: [],
                    index: 0
                )
            case let .failure(error):
                fputs("initial page failed: \(error)\n", stderr)
                self.finish(exitCode: 1)
            }
        }
    }

    @objc private func menuItemClicked() {
        guard let controller, let button = menuButton else { return }
        controller.panel.toggle(relativeTo: button, firstResponder: controller.firstResponder)
        if controller.panel.isVisible {
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    private func runMenuMeasurements(
        initialPage: HistoryPageDto,
        initialPageLatencyMs: Double,
        samples: [Double],
        index: Int
    ) {
        guard let controller, let button = menuButton else {
            finish(exitCode: 1)
            return
        }
        if index >= arguments.menuRuns {
            startScrolling(
                initialPage: initialPage,
                initialPageLatencyMs: initialPageLatencyMs,
                menuSamples: samples
            )
            return
        }

        var drawAt: UInt64?
        controller.armFirstDraw {
            drawAt = BenchmarkClock.now()
        }
        let started = BenchmarkClock.now()
        button.performClick(nil)
        controller.displayPanelIfNeeded()
        let finished = drawAt ?? BenchmarkClock.now()
        let elapsed = BenchmarkClock.milliseconds(from: started, to: finished)
        var nextSamples = samples
        nextSamples.append(elapsed)
        controller.hidePanel()
        DispatchQueue.main.async { [weak self] in
            self?.runMenuMeasurements(
                initialPage: initialPage,
                initialPageLatencyMs: initialPageLatencyMs,
                samples: nextSamples,
                index: index + 1
            )
        }
    }

    private func startScrolling(
        initialPage: HistoryPageDto,
        initialPageLatencyMs: Double,
        menuSamples: [Double]
    ) {
        guard let controller, let button = menuButton else {
            finish(exitCode: 1)
            return
        }
        controller.showPanel(relativeTo: button)
        controller.displayPanelIfNeeded()
        let rssBeforeScroll = ProcessRSS.currentBytes()
        let started = BenchmarkClock.now()
        continueScrolling(
            initialPage: initialPage,
            initialPageLatencyMs: initialPageLatencyMs,
            menuSamples: menuSamples,
            pageLatencies: [],
            rowsSeen: initialPage.items.count,
            lastSeen: initialPage.items.last.map(cursor(for:)),
            orderViolations: 0,
            pageCount: 0,
            rssBeforeScroll: rssBeforeScroll,
            scrollStarted: started
        )
    }

    private func continueScrolling(
        initialPage: HistoryPageDto,
        initialPageLatencyMs: Double,
        menuSamples: [Double],
        pageLatencies: [Double],
        rowsSeen: Int,
        lastSeen: HistoryCursorDto?,
        orderViolations: Int,
        pageCount: Int,
        rssBeforeScroll: UInt64,
        scrollStarted: UInt64
    ) {
        guard let controller else {
            finish(exitCode: 1)
            return
        }
        if pageCount >= arguments.scrollPages || !controller.hasMoreOlder {
            let scrollElapsed = BenchmarkClock.milliseconds(
                from: scrollStarted,
                to: BenchmarkClock.now()
            )
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
                self?.finishMeasurement(
                    initialPage: initialPage,
                    initialPageLatencyMs: initialPageLatencyMs,
                    menuSamples: menuSamples,
                    pageLatencies: pageLatencies,
                    rowsSeen: rowsSeen,
                    lastSeen: lastSeen,
                    orderViolations: orderViolations,
                    pageCount: pageCount,
                    rssBeforeScroll: rssBeforeScroll,
                    scrollElapsed: scrollElapsed
                )
            }
            return
        }

        controller.fetchNextOlder { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success((page, latency)):
                var nextRowsSeen = rowsSeen
                var nextLastSeen = lastSeen
                var nextViolations = orderViolations
                for item in page.items {
                    let current = cursor(for: item)
                    if let previous = nextLastSeen,
                       !isStrictlyOlder(current, than: previous) {
                        nextViolations += 1
                    }
                    nextLastSeen = current
                    nextRowsSeen += 1
                }
                var nextLatencies = pageLatencies
                nextLatencies.append(latency)
                controller.scrollDisplayIfNeeded()
                DispatchQueue.main.async { [weak self] in
                    self?.continueScrolling(
                        initialPage: initialPage,
                        initialPageLatencyMs: initialPageLatencyMs,
                        menuSamples: menuSamples,
                        pageLatencies: nextLatencies,
                        rowsSeen: nextRowsSeen,
                        lastSeen: nextLastSeen,
                        orderViolations: nextViolations,
                        pageCount: pageCount + 1,
                        rssBeforeScroll: rssBeforeScroll,
                        scrollStarted: scrollStarted
                    )
                }
            case let .failure(error):
                fputs("scroll page failed: \(error)\n", stderr)
                self.finish(exitCode: 1)
            }
        }
    }

    private func finishMeasurement(
        initialPage: HistoryPageDto,
        initialPageLatencyMs: Double,
        menuSamples: [Double],
        pageLatencies: [Double],
        rowsSeen: Int,
        lastSeen: HistoryCursorDto?,
        orderViolations: Int,
        pageCount: Int,
        rssBeforeScroll: UInt64,
        scrollElapsed: Double
    ) {
        guard let controller else {
            finish(exitCode: 1)
            return
        }
        let rssAfterScroll = ProcessRSS.currentBytes()
        let peakRSS = rssSampler?.stop() ?? rssAfterScroll
        let storageBytesBeforeShutdown = storageBytes(at: arguments.root)
        do {
            try storeClient?.shutdown()
        } catch {
            fputs("clean shutdown failed: \(error)\n", stderr)
            finish(exitCode: 1)
            return
        }
        let storageBytes = storageBytes(at: arguments.root)
        let menu = LatencySummary(menuSamples)
        let pages = LatencySummary(pageLatencies)
        emit("mode=measure")
        emit("scenario=\(arguments.scenario.rawValue)")
        emit("rows=\(arguments.rowCount)")
        emit("initial_page_rows=\(initialPage.items.count)")
        emit("initial_page_fetch_ms=\(format(initialPageLatencyMs))")
        emit("menu_click_samples=\(menu.count)")
        emit("menu_click_first_draw_p50_ms=\(format(menu.p50))")
        emit("menu_click_first_draw_p95_ms=\(format(menu.p95))")
        emit("menu_click_first_draw_max_ms=\(format(menu.maximum))")
        emit("scroll_page_count=\(pageCount)")
        emit("scroll_rows_seen=\(rowsSeen)")
        emit("scroll_page_fetch_p50_ms=\(format(pages.p50))")
        emit("scroll_page_fetch_p95_ms=\(format(pages.p95))")
        emit("scroll_page_fetch_p99_ms=\(format(pages.p99))")
        emit("scroll_page_fetch_max_ms=\(format(pages.maximum))")
        emit("scroll_elapsed_ms=\(format(scrollElapsed))")
        emit("scroll_order_violations=\(orderViolations)")
        emit("final_last_seen_id=\(lastSeen?.id ?? -1)")
        emit("rss_before_scroll_bytes=\(rssBeforeScroll)")
        emit("rss_after_scroll_bytes=\(rssAfterScroll)")
        emit("rss_peak_bytes=\(peakRSS)")
        emit("rss_scroll_delta_bytes=\(peakRSS >= rssBeforeScroll ? peakRSS - rssBeforeScroll : 0)")
        emit("decoded_image_preview_count=\(controller.decodedImagePreviewCount)")
        emit("decoded_image_preview_source_bytes=\(controller.decodedImagePreviewBytes)")
        emit("decoded_image_cache_count_limit=64")
        emit("decoded_image_cache_cost_limit_bytes=4194304")
        emit("storage_bytes_before_shutdown=\(storageBytesBeforeShutdown)")
        emit("storage_bytes=\(storageBytes)")
        finish(exitCode: 0)
    }

    private func finish(exitCode: Int32) {
        guard !isFinishing else { return }
        isFinishing = true
        _ = rssSampler?.stop()
        self.exitCode = exitCode
    }
}

private struct SeedSummary {
    let scenario: String
    let rows: Int
    let storageBytes: UInt64
}

private func seedDatabase(_ arguments: BenchmarkArguments) throws -> SeedSummary {
    let fileManager = FileManager.default
    if fileManager.fileExists(atPath: arguments.root.path) {
        try fileManager.removeItem(at: arguments.root)
    }
    try fileManager.createDirectory(at: arguments.root, withIntermediateDirectories: true)
    try fileManager.createDirectory(
        atPath: arguments.payloadDirectory,
        withIntermediateDirectories: true
    )

    let engine = try ClipboardEngine.open(
        databasePath: arguments.databasePath,
        payloadDirectory: arguments.payloadDirectory
    )
    let image = try makeImageFixture()
    let imagePreview = ImagePreviewGenerator.makePreview(from: [image])
    let started = BenchmarkClock.now()

    for index in 0 ..< arguments.rowCount {
        try autoreleasepool {
            let text = RepresentationDto(
                uti: "public.utf8-plain-text",
                bytes: Data(textBytes(for: index, scenario: arguments.scenario))
            )
            let representations: [RepresentationDto]
            let preview: RepresentationDto?
            switch arguments.scenario {
            case .textOnly:
                representations = [text]
                preview = nil
            case .mixedImages:
                if index % 10 == 0 {
                    representations = [text, image]
                    preview = imagePreview
                } else {
                    representations = [text]
                    preview = nil
                }
            case .hugePayload:
                if index % 100 == 0 {
                    let richText = RepresentationDto(
                        uti: "public.rtf",
                        bytes: hugeRichTextBytes(for: index)
                    )
                    representations = [text, richText]
                } else {
                    representations = [text]
                }
                preview = nil
            }
            _ = try engine.capture(
                representations: representations,
                imagePreview: preview,
                copiedAtMs: Int64(arguments.rowCount - index)
            )
        }
        if (index + 1) % 10_000 == 0 {
            let elapsed = BenchmarkClock.milliseconds(from: started, to: BenchmarkClock.now())
            fputs("seeded=\(index + 1) elapsed_ms=\(format(elapsed))\n", stderr)
        }
    }

    try engine.shutdown()
    return SeedSummary(
        scenario: arguments.scenario.rawValue,
        rows: arguments.rowCount,
        storageBytes: storageBytes(at: arguments.root)
    )
}

private func makeImageFixture() throws -> RepresentationDto {
    let width = 96
    let height = 64
    guard
        let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: width,
            pixelsHigh: height,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bitmapFormat: [],
            bytesPerRow: 0,
            bitsPerPixel: 0
        ),
        let bytes = bitmap.bitmapData
    else {
        throw BenchmarkError.noImageFixture
    }

    for y in 0 ..< height {
        for x in 0 ..< width {
            let offset = y * bitmap.bytesPerRow + x * 4
            bytes[offset] = UInt8((x * 3) % 255)
            bytes[offset + 1] = UInt8((y * 4) % 255)
            bytes[offset + 2] = UInt8(((x + y) * 2) % 255)
            bytes[offset + 3] = 255
        }
    }
    guard let encoded = bitmap.representation(using: .png, properties: [:]) else {
        throw BenchmarkError.noImageFixture
    }
    return RepresentationDto(uti: "public.png", bytes: encoded)
}

private func textBytes(for index: Int, scenario: BenchmarkScenario) -> [UInt8] {
    let category: String
    switch index % 5 {
    case 0: category = "project alpha architecture"
    case 1: category = "release checklist and review"
    case 2: category = "meeting notes and follow up"
    case 3: category = "rust sqlite performance experiment"
    default: category = "日本語のクリップボード履歴と検索"
    }
    let value = "clipboard \(scenario.rawValue) item \(String(format: "%08d", index)) "
        + "\(category) — deterministic application benchmark"
    return Array(value.utf8)
}

private func hugeRichTextBytes(for index: Int) -> Data {
    let size = 256 * 1024
    var bytes = Data(repeating: UInt8(index % 251), count: size)
    let header = Data("{\\rtf1\\ansi benchmark payload \(index) ".utf8)
    bytes.replaceSubrange(0 ..< header.count, with: header)
    return bytes
}

private func cursor(for item: ClipSummaryDto) -> HistoryCursorDto {
    HistoryCursorDto(lastUsedAtMs: item.lastUsedAtMs, id: item.id)
}

private func isStrictlyOlder(_ current: HistoryCursorDto, than previous: HistoryCursorDto) -> Bool {
    current.lastUsedAtMs < previous.lastUsedAtMs
        || (current.lastUsedAtMs == previous.lastUsedAtMs && current.id < previous.id)
}

private func storageBytes(at root: URL) -> UInt64 {
    guard
        let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey],
            options: [.skipsHiddenFiles]
        )
    else {
        return 0
    }
    var total: UInt64 = 0
    for case let url as URL in enumerator {
        guard
            let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
            values.isRegularFile == true,
            let size = values.fileSize
        else {
            continue
        }
        total += UInt64(size)
    }
    return total
}

private func format(_ value: Double) -> String {
    String(format: "%.3f", value)
}

private func emit(_ value: String) {
    fputs("\(value)\n", stdout)
    fflush(stdout)
}

private let benchmarkArguments: BenchmarkArguments
do {
    benchmarkArguments = try BenchmarkArguments(arguments: CommandLine.arguments)
} catch {
    fputs("benchmark argument error: \(error.localizedDescription)\n", stderr)
    exit(2)
}

private let application = NSApplication.shared
private let delegate = BenchmarkApplicationDelegate(arguments: benchmarkArguments)
application.delegate = delegate
application.setActivationPolicy(.accessory)
delegate.start()
while delegate.exitCode == nil {
    _ = autoreleasepool {
        RunLoop.main.run(
            mode: .default,
            before: Date(timeIntervalSinceNow: 0.01)
        )
    }
}
exit(delegate.exitCode ?? 1)
