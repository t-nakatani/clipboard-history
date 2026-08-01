import AppKit
import Foundation

final class AppDelegate: NSObject, NSApplicationDelegate, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private let uiConfiguration = HistoryPanelConfiguration.standard
    private var statusItem: NSStatusItem?
    private var monitor: PasteboardMonitor?
    private var storeClient: HistoryStoreClient?
    private var panel: HistoryPanel?
    private var pageWindow = HistoryPageWindow(maximumCount: 200)
    private var historyRows: [ClipSummaryDto] { pageWindow.rows }
    private let imagePreviewCache: NSCache<NSNumber, NSImage> = {
        let cache = NSCache<NSNumber, NSImage>()
        cache.countLimit = 64
        cache.totalCostLimit = 4 * 1024 * 1024
        return cache
    }()
    private var pendingImagePreviews: Set<Int64> = []
    private let tableView = HistoryTableView()
    private weak var historyClipView: NSClipView?
    private let pageSize: UInt32 = 50
    private var isLoadingPage = false
    private var activeSearchQuery: String?
    private var activeSearchMode: SearchModeDto = .substring
    private let searchField = NSSearchField()
    private let searchModeControl = NSSegmentedControl(
        labels: ["完全", "前方", "部分"],
        trackingMode: .selectOne,
        target: nil,
        action: nil
    )
    private var searchTimer: Timer?
    private var searchGeneration = 0
    private let statusLabel = NSTextField(labelWithString: "コピー待機中")
    private let detailLabel = NSTextField(wrappingLabelWithString: "型一覧の検査後、許可されたpayloadだけを読み取ります。")

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        configureStatusItem()
        configurePanel()
        statusLabel.stringValue = "ストレージを準備中…"
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result { try HistoryStoreClient() }
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case let .success(client):
                    self.startMonitoring(storeClient: client)
                case let .failure(error):
                    self.show(error: error)
                }
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        searchTimer?.invalidate()
        monitor?.stop()
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

    @objc private func terminate() {
        NSApplication.shared.terminate(nil)
    }

    @objc private func restoreSelected() {
        let row = tableView.selectedRow
        guard historyRows.indices.contains(row), let storeClient else { return }
        let summary = historyRows[row]
        statusLabel.stringValue = "復元中…"
        storeClient.select(id: summary.id) { [weak self] result in
            do {
                let representations = try result.get()
                try PasteboardWriter.restore(representations: representations)
                self?.statusLabel.stringValue = "Pasteboardへ復元"
                self?.detailLabel.stringValue = summary.preview ?? summary.kind
                self?.panel?.dismiss()
            } catch {
                self?.show(error: error)
            }
        }
    }

    @objc private func deleteSelected() {
        let row = tableView.selectedRow
        guard historyRows.indices.contains(row), let storeClient else { return }
        let id = historyRows[row].id
        storeClient.delete(id: id) { [weak self] result in
            switch result {
            case .success(true):
                self?.statusLabel.stringValue = "履歴を削除"
                self?.reloadHistory()
            case .success(false):
                self?.reloadHistory()
            case let .failure(error):
                self?.show(error: error)
            }
        }
    }

    @objc private func searchModeChanged() {
        scheduleSearch()
    }

    private func startMonitoring(storeClient: HistoryStoreClient) {
        self.storeClient = storeClient
        monitor = PasteboardMonitor(
            onCapture: { [weak self] candidate in self?.persist(candidate: candidate) },
            onRejection: { [weak self] decision in self?.show(rejection: decision) }
        )
        monitor?.start()
        statusLabel.stringValue = "コピー待機中"
        reloadHistory()
    }

    private func configureStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "clipboard", accessibilityDescription: "Clipboard History")
        item.button?.target = self
        item.button?.action = #selector(performStatusItemClick)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        statusItem = item
    }

    private func makeStatusItemContextMenu() -> NSMenu {
        let menu = NSMenu()
        let quitItem = NSMenuItem(title: "終了", action: #selector(terminate), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)
        return menu
    }

    private func togglePanel() {
        guard let panel, let button = statusItem?.button else { return }
        panel.toggle(relativeTo: button, firstResponder: searchField)
        if panel.isVisible {
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    private func configurePanel() {
        let panel = HistoryPanel(configuration: uiConfiguration)
        panel.visibilityDidChange = { [weak self] visible in
            self?.statusItem?.button?.highlight(visible)
        }

        let panelContent = TranslucentPanelContentView(
            frame: panel.contentRect(forFrameRect: panel.frame),
            configuration: uiConfiguration
        )
        panel.contentView = panelContent

        statusLabel.font = .systemFont(ofSize: uiConfiguration.typography.statusSize, weight: .semibold)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.font = .systemFont(ofSize: uiConfiguration.typography.detailSize)
        detailLabel.maximumNumberOfLines = 2

        searchField.placeholderString = "入力して検索"
        searchField.delegate = self
        searchModeControl.selectedSegment = 2
        searchModeControl.target = self
        searchModeControl.action = #selector(searchModeChanged)
        let appNameLabel = NSTextField(labelWithString: "Clipboard")
        appNameLabel.font = .systemFont(
            ofSize: uiConfiguration.typography.appNameSize,
            weight: uiConfiguration.typography.appNameWeight
        )
        appNameLabel.textColor = .tertiaryLabelColor
        let searchBar = NSStackView(views: [appNameLabel, searchField, searchModeControl])
        searchBar.orientation = .horizontal
        searchBar.spacing = uiConfiguration.content.searchItemSpacing
        searchField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        searchModeControl.setContentHuggingPriority(.required, for: .horizontal)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history"))
        column.title = "履歴"
        column.resizingMask = .autoresizingMask
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.rowHeight = uiConfiguration.rows.textHeight
        tableView.intercellSpacing = NSSize(
            width: 0,
            height: uiConfiguration.rows.intercellSpacing
        )
        tableView.backgroundColor = .clear
        tableView.usesAlternatingRowBackgroundColors = false
        tableView.dataSource = self
        tableView.delegate = self
        tableView.target = self
        tableView.doubleAction = #selector(restoreSelected)
        tableView.confirmSelection = { [weak self] in
            self?.restoreSelected()
        }
        tableView.deleteSelection = { [weak self] in
            self?.deleteSelected()
        }

        let scrollView = NSScrollView()
        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.borderType = .noBorder
        scrollView.drawsBackground = false
        scrollView.scrollerStyle = .overlay
        scrollView.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(historyBoundsDidChange),
            name: NSView.boundsDidChangeNotification,
            object: scrollView.contentView
        )
        historyClipView = scrollView.contentView

        let stack = NSStackView(views: [searchBar, scrollView])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = uiConfiguration.content.sectionSpacing
        stack.translatesAutoresizingMaskIntoConstraints = false
        panelContent.foregroundView.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(
                equalTo: panelContent.foregroundView.leadingAnchor,
                constant: uiConfiguration.content.leadingInset
            ),
            stack.trailingAnchor.constraint(
                equalTo: panelContent.foregroundView.trailingAnchor,
                constant: -uiConfiguration.content.trailingInset
            ),
            stack.topAnchor.constraint(
                equalTo: panelContent.foregroundView.topAnchor,
                constant: uiConfiguration.content.topInset
            ),
            stack.bottomAnchor.constraint(
                equalTo: panelContent.foregroundView.bottomAnchor,
                constant: -uiConfiguration.content.bottomInset
            ),
            searchBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.heightAnchor.constraint(
                greaterThanOrEqualToConstant: uiConfiguration.content.minimumHistoryHeight
            ),
        ])
        self.panel = panel
    }

    private func persist(candidate: CapturedClipboardCandidate) {
        statusLabel.stringValue = "保存中…"
        let shortIdentity = String(candidate.identity.prefix(12))
        detailLabel.stringValue = "\(candidate.representationTypes.joined(separator: ", "))\n\(candidate.payloadBytes) bytes · hash \(shortIdentity)…"
        let copiedAtMs = Int64(Date().timeIntervalSince1970 * 1_000)
        storeClient?.capture(
            representations: candidate.representations,
            copiedAtMs: copiedAtMs
        ) { [weak self] result in
            switch result {
            case let .success(capture):
                self?.statusLabel.stringValue = capture.result.inserted ? "履歴へ保存" : "既存履歴を先頭へ移動"
                let count = capture.recentPage.items.count
                self?.detailLabel.stringValue = "clip #\(capture.result.id) · 最近\(count)件をメモリ保持"
                if self?.searchField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == true {
                    self?.searchGeneration += 1
                    self?.activeSearchQuery = nil
                    self?.isLoadingPage = false
                    if self?.panel?.isVisible == true, self?.isViewingAwayFromNewest == true {
                        self?.pageWindow.markNewerAvailable()
                        self?.statusLabel.stringValue = "新しい履歴あり"
                    } else {
                        self?.apply(page: capture.recentPage, reset: true)
                    }
                } else {
                    self?.performSearch()
                }
            case let .failure(error):
                self?.show(error: error)
            }
        }
    }

    private func show(rejection: CaptureFilterDecisionDto) {
        statusLabel.stringValue = "保存対象外"
        switch rejection {
        case .rejectConcealed:
            detailLabel.stringValue = "concealed markerを型一覧から検知しました。payload bytesは読み取っていません。"
        case .rejectTransient:
            detailLabel.stringValue = "transient markerを型一覧から検知しました。payload bytesは読み取っていません。"
        case .accept:
            break
        }
    }

    private func show(error: Error) {
        statusLabel.stringValue = "ストレージエラー"
        detailLabel.stringValue = error.localizedDescription
    }

    private func reloadHistory() {
        if !searchField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            performSearch()
            return
        }
        guard let storeClient else { return }
        searchGeneration += 1
        let generation = searchGeneration
        activeSearchQuery = nil
        isLoadingPage = true
        storeClient.recentPage(limit: pageSize) { [weak self] result in
            guard let self, generation == self.searchGeneration else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: true)
                self.statusLabel.stringValue = "履歴 \(page.items.count)件を読み込み済み"
                if let newest = page.items.first {
                    self.detailLabel.stringValue = newest.preview ?? newest.kind
                }
            case let .failure(error):
                self.show(error: error)
            }
        }
    }

    func controlTextDidChange(_ notification: Notification) {
        scheduleSearch()
    }

    private func scheduleSearch() {
        searchTimer?.invalidate()
        searchTimer = Timer.scheduledTimer(withTimeInterval: 0.12, repeats: false) { [weak self] _ in
            self?.performSearch()
        }
    }

    private func performSearch() {
        guard let storeClient else { return }
        searchTimer?.invalidate()
        searchTimer = nil
        searchGeneration += 1
        let generation = searchGeneration
        let query = searchField.stringValue
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            reloadHistory()
            return
        }
        let mode: SearchModeDto
        switch searchModeControl.selectedSegment {
        case 0: mode = .exact
        case 1: mode = .prefix
        default: mode = .substring
        }
        activeSearchQuery = query
        activeSearchMode = mode
        isLoadingPage = true
        statusLabel.stringValue = "検索中…"
        storeClient.searchPage(query: query, mode: mode, limit: pageSize) { [weak self] result in
            guard let self, generation == self.searchGeneration else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: true)
                self.statusLabel.stringValue = "検索結果 \(page.items.count)件"
                self.detailLabel.stringValue = "完全一致・前方一致・正確な部分一致のみ"
            case let .failure(error):
                self.show(error: error)
            }
        }
    }

    private func apply(
        page: HistoryPageDto,
        reset: Bool,
        direction: PageDirectionDto = .older
    ) {
        let anchor = reset ? nil : visibleAnchor()
        let selectedId = historyRows.indices.contains(tableView.selectedRow)
            ? historyRows[tableView.selectedRow].id
            : nil
        if reset {
            pageWindow.reset(with: page)
        } else {
            switch direction {
            case .older: pageWindow.appendOlder(page)
            case .newer: pageWindow.prependNewer(page)
            }
        }
        tableView.reloadData()
        if let selectedId, let selectedRow = historyRows.firstIndex(where: { $0.id == selectedId }) {
            tableView.selectRowIndexes(IndexSet(integer: selectedRow), byExtendingSelection: false)
        } else if reset {
            tableView.deselectAll(nil)
        }
        if let anchor {
            restoreVisibleAnchor(anchor)
        } else if !historyRows.isEmpty, tableView.selectedRow < 0 {
            tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        }
    }

    @objc private func historyBoundsDidChange() {
        let visibleRows = tableView.rows(in: tableView.visibleRect)
        guard visibleRows.location != NSNotFound else { return }
        if visibleRows.location <= 10, pageWindow.hasMoreNewer {
            loadPage(.newer)
        } else if NSMaxRange(visibleRows) >= max(0, historyRows.count - 10), pageWindow.hasMoreOlder {
            loadPage(.older)
        }
    }

    private func loadPage(_ direction: PageDirectionDto) {
        guard !isLoadingPage, let storeClient else { return }
        let cursor: HistoryCursorDto?
        switch direction {
        case .older:
            guard pageWindow.hasMoreOlder else { return }
            cursor = pageWindow.olderAnchor
        case .newer:
            guard pageWindow.hasMoreNewer else { return }
            cursor = pageWindow.newerAnchor
        }
        guard let cursor else { return }
        isLoadingPage = true
        let generation = searchGeneration
        let completion: HistoryStoreClient.Completion<HistoryPageDto> = { [weak self] result in
            guard let self, generation == self.searchGeneration else { return }
            self.isLoadingPage = false
            switch result {
            case let .success(page):
                self.apply(page: page, reset: false, direction: direction)
            case let .failure(error):
                self.show(error: error)
            }
        }
        if let query = activeSearchQuery {
            storeClient.searchPage(
                query: query,
                mode: activeSearchMode,
                cursor: cursor,
                direction: direction,
                limit: pageSize,
                completion: completion
            )
        } else {
            storeClient.recentPage(
                cursor: cursor,
                direction: direction,
                limit: pageSize,
                completion: completion
            )
        }
    }

    private var isViewingAwayFromNewest: Bool {
        if pageWindow.hasMoreNewer { return true }
        let visibleRows = tableView.rows(in: tableView.visibleRect)
        return visibleRows.location != NSNotFound && visibleRows.location > 10
    }

    private func visibleAnchor() -> (id: Int64, offset: CGFloat)? {
        guard let historyClipView else { return nil }
        let row = tableView.row(at: NSPoint(x: 1, y: tableView.visibleRect.minY + 1))
        guard historyRows.indices.contains(row) else { return nil }
        return (
            historyRows[row].id,
            historyClipView.bounds.minY - tableView.rect(ofRow: row).minY
        )
    }

    private func restoreVisibleAnchor(_ anchor: (id: Int64, offset: CGFloat)) {
        guard
            let historyClipView,
            let row = historyRows.firstIndex(where: { $0.id == anchor.id })
        else { return }
        var origin = historyClipView.bounds.origin
        origin.y = max(0, tableView.rect(ofRow: row).minY + anchor.offset)
        historyClipView.setBoundsOrigin(origin)
        historyClipView.enclosingScrollView?.reflectScrolledClipView(historyClipView)
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        historyRows.count
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        historyRows[row].hasImagePreview
            ? uiConfiguration.rows.imageHeight
            : uiConfiguration.rows.textHeight
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("HistoryCell")
        let cell: HistoryCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? HistoryCellView {
            cell = reused
        } else {
            cell = HistoryCellView(configuration: uiConfiguration)
        }
        let summary = historyRows[row]
        cell.clipId = summary.id
        cell.configureForImagePreview(summary.hasImagePreview)
        let preview = (summary.preview ?? "[\(summary.kind)]")
            .replacingOccurrences(of: "\n", with: " ")
        cell.previewLabel.stringValue = preview
        cell.previewLabel.isHidden = summary.hasImagePreview && summary.preview == nil
        cell.metadataLabel.stringValue = "\(summary.kind) · \(ByteCountFormatter.string(fromByteCount: Int64(summary.payloadSize), countStyle: .file))"
        cell.thumbnailImageView.image = placeholderImage(for: summary.kind)
        if summary.hasImagePreview {
            let cacheKey = NSNumber(value: summary.id)
            if let image = imagePreviewCache.object(forKey: cacheKey) {
                cell.thumbnailImageView.image = image
            } else {
                loadImagePreview(id: summary.id)
            }
        }
        cell.toolTip = "\(summary.payloadSize) bytes · copied \(summary.copyCount)回"
        return cell
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
        guard pendingImagePreviews.insert(id).inserted, let storeClient else { return }
        storeClient.imagePreview(id: id) { [weak self] result in
            guard let self else { return }
            self.pendingImagePreviews.remove(id)
            guard
                case let .success(preview?) = result,
                let image = NSImage(data: preview.bytes)
            else {
                return
            }
            self.imagePreviewCache.setObject(image, forKey: NSNumber(value: id), cost: 96 * 96 * 4)
            guard let row = self.historyRows.firstIndex(where: { $0.id == id }) else { return }
            self.tableView.reloadData(
                forRowIndexes: IndexSet(integer: row),
                columnIndexes: IndexSet(integer: 0)
            )
        }
    }
}
