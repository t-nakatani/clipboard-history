import AppKit

/// Builds and drives the panel's contents: search controls, the history table,
/// and the scroll bookkeeping that keeps the view steady while pages arrive.
///
/// Owns no storage state of its own — rows, the live query and every store call
/// belong to `HistoryFeedModel`.
final class HistoryListController: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    /// How close to either edge of the resident rows the viewport has to get
    /// before the next page is requested.
    private static let pagingRowMargin = 10

    let contentView: TranslucentPanelContentView
    let searchField = NSSearchField()

    /// Reports what the list is doing so the owner can present it. Restoring is
    /// driven from here, so its outcome does not reach the feed.
    var statusDidChange: ((HistoryStatus) -> Void)?
    /// A restored clip is the panel's cue to go away. Whoever owns the panel
    /// decides how that happens.
    var dismissPanel: (() -> Void)?

    private let configuration: HistoryPanelConfiguration
    private let feed: HistoryFeedModel
    private let previewLoader: ImagePreviewLoader
    private let statusView: HistoryStatusView
    private let tableView = HistoryTableView()
    private let searchModeControl = NSSegmentedControl(
        labels: ["完全", "前方", "部分"],
        trackingMode: .selectOne,
        target: nil,
        action: nil
    )
    private weak var historyClipView: NSClipView?
    /// Set while the list scrolls itself — keyboard selection, or the anchor
    /// put back after a page arrives — so that scrolling does not hand the
    /// selection to whatever row happens to slide under the pointer.
    private var isScrollingProgrammatically = false

    init(
        configuration: HistoryPanelConfiguration,
        feed: HistoryFeedModel,
        previewLoader: ImagePreviewLoader,
        statusView: HistoryStatusView,
        contentFrame: NSRect
    ) {
        self.configuration = configuration
        self.feed = feed
        self.previewLoader = previewLoader
        self.statusView = statusView
        contentView = TranslucentPanelContentView(
            frame: contentFrame,
            configuration: configuration
        )
        super.init()
        buildContents()
        previewLoader.didLoad = { [weak self] id in self?.redrawRow(id: id) }
        feed.updateRows = { [weak self] reset, apply in
            guard let self else {
                apply()
                return
            }
            self.updateRows(reset: reset, apply: apply)
        }
        feed.shouldHoldNewestUpdate = { [weak self] in self?.shouldHoldNewestUpdate ?? false }
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    /// True while a freshly captured clip would not be visible anyway: either
    /// the panel is off screen, or the reader has scrolled away from the newest
    /// rows and should not have them yanked out from under them.
    private var shouldHoldNewestUpdate: Bool {
        guard contentView.window?.isVisible == true else { return false }
        if feed.hasMoreNewer { return true }
        let visibleRows = tableView.rows(in: tableView.visibleRect)
        return visibleRows.location != NSNotFound
            && visibleRows.location > Self.pagingRowMargin
    }

    private var selectedSearchMode: SearchModeDto {
        switch searchModeControl.selectedSegment {
        case 0: return .exact
        case 1: return .prefix
        default: return .substring
        }
    }

    // MARK: - View construction

    private func buildContents() {
        searchField.placeholderString = "入力して検索"
        searchField.delegate = self
        searchModeControl.selectedSegment = 2
        searchModeControl.target = self
        searchModeControl.action = #selector(searchModeChanged)

        let appNameLabel = NSTextField(labelWithString: "Clipboard")
        appNameLabel.font = .systemFont(
            ofSize: configuration.typography.appNameSize,
            weight: configuration.typography.appNameWeight
        )
        appNameLabel.textColor = .tertiaryLabelColor
        let searchBar = NSStackView(views: [appNameLabel, searchField, searchModeControl])
        searchBar.orientation = .horizontal
        searchBar.spacing = configuration.content.searchItemSpacing
        searchField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        searchModeControl.setContentHuggingPriority(.required, for: .horizontal)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("history"))
        column.title = "履歴"
        column.resizingMask = .autoresizingMask
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.rowHeight = configuration.rows.textHeight
        tableView.intercellSpacing = NSSize(
            width: 0,
            height: configuration.rows.intercellSpacing
        )
        // The list always keeps a row selected: it is what Return restores, and
        // a click below the last row would otherwise clear it.
        tableView.allowsEmptySelection = false
        tableView.backgroundColor = .clear
        tableView.usesAlternatingRowBackgroundColors = false
        tableView.dataSource = self
        tableView.delegate = self
        tableView.target = self
        tableView.action = #selector(restoreClicked)

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

        let stack = NSStackView(views: [searchBar, scrollView, statusView])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = configuration.content.sectionSpacing
        stack.translatesAutoresizingMaskIntoConstraints = false
        contentView.foregroundView.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(
                equalTo: contentView.foregroundView.leadingAnchor,
                constant: configuration.content.leadingInset
            ),
            stack.trailingAnchor.constraint(
                equalTo: contentView.foregroundView.trailingAnchor,
                constant: -configuration.content.trailingInset
            ),
            stack.topAnchor.constraint(
                equalTo: contentView.foregroundView.topAnchor,
                constant: configuration.content.topInset
            ),
            stack.bottomAnchor.constraint(
                equalTo: contentView.foregroundView.bottomAnchor,
                constant: -configuration.content.bottomInset
            ),
            searchBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scrollView.heightAnchor.constraint(
                greaterThanOrEqualToConstant: configuration.content.minimumHistoryHeight
            ),
            statusView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            statusView.heightAnchor.constraint(
                equalToConstant: configuration.content.statusRowHeight
            ),
        ])
    }

    // MARK: - Actions

    @objc private func searchModeChanged() {
        feed.scheduleSearch(text: searchField.stringValue, mode: selectedSearchMode)
    }

    func controlTextDidChange(_ notification: Notification) {
        feed.scheduleSearch(text: searchField.stringValue, mode: selectedSearchMode)
    }

    /// The table refuses first responder, so every key arrives here and the
    /// ones the list needs are routed on. Backspace only reaches the list once
    /// the query is empty; until then it belongs to the text being typed, and
    /// Escape is left alone so it still clears the query before closing.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)):
            moveSelection(by: -1)
        case #selector(NSResponder.moveDown(_:)):
            moveSelection(by: 1)
        case #selector(NSResponder.scrollPageUp(_:)), #selector(NSResponder.pageUp(_:)):
            moveSelection(by: -visibleRowCount)
        case #selector(NSResponder.scrollPageDown(_:)), #selector(NSResponder.pageDown(_:)):
            moveSelection(by: visibleRowCount)
        case #selector(NSResponder.moveToBeginningOfDocument(_:)),
             #selector(NSResponder.scrollToBeginningOfDocument(_:)):
            select(row: 0)
        case #selector(NSResponder.moveToEndOfDocument(_:)),
             #selector(NSResponder.scrollToEndOfDocument(_:)):
            select(row: feed.rows.count - 1)
        case #selector(NSResponder.insertNewline(_:)):
            restoreSelected()
        case #selector(NSResponder.deleteBackward(_:)), #selector(NSResponder.deleteForward(_:)):
            guard searchField.stringValue.isEmpty else { return false }
            deleteSelected()
        default:
            return false
        }
        return true
    }

    private var visibleRowCount: Int {
        let rows = tableView.rows(in: tableView.visibleRect)
        return rows.location == NSNotFound ? 1 : max(1, rows.length - 1)
    }

    private func moveSelection(by offset: Int) {
        let current = tableView.selectedRow
        select(row: current < 0 ? 0 : current + offset)
    }

    /// Puts the selection on `row`, clamped to the rows on hand, while the
    /// pointer stays put — so the hover sync has to stand down until the row is
    /// scrolled into view.
    private func select(row: Int) {
        guard !feed.rows.isEmpty else { return }
        let target = min(max(row, 0), feed.rows.count - 1)
        withProgrammaticScroll {
            tableView.selectRowIndexes(IndexSet(integer: target), byExtendingSelection: false)
            tableView.scrollRowToVisible(target)
        }
    }

    /// Holds the hover sync down for the scroll `body` causes. The release is
    /// deferred by a turn because the bounds change AppKit posts for that scroll
    /// does not always land inside `body`.
    private func withProgrammaticScroll(_ body: () -> Void) {
        isScrollingProgrammatically = true
        body()
        DispatchQueue.main.async { [weak self] in
            self?.isScrollingProgrammatically = false
        }
    }

    /// A single click restores the row it landed on. Clicks in the empty area
    /// below the last row report no row, and the second click of a double click
    /// would otherwise restore the same clip twice.
    @objc private func restoreClicked() {
        guard tableView.lastClickCount == 1 else { return }
        let row = tableView.clickedRow
        guard feed.rows.indices.contains(row) else { return }
        restore(summary: feed.rows[row])
    }

    @objc private func restoreSelected() {
        guard let summary = selectedSummary else { return }
        restore(summary: summary)
    }

    private func restore(summary: ClipSummaryDto) {
        statusDidChange?(.restoring)
        feed.representations(for: summary.id) { [weak self] result in
            do {
                try PasteboardWriter.restore(representations: try result.get())
                self?.statusDidChange?(.restored(preview: summary.preview ?? summary.kind))
                self?.dismissPanel?()
            } catch {
                self?.statusDidChange?(.failed(error))
            }
        }
    }

    @objc private func deleteSelected() {
        guard let summary = selectedSummary else { return }
        feed.delete(id: summary.id)
    }

    private var selectedSummary: ClipSummaryDto? {
        let row = tableView.selectedRow
        guard feed.rows.indices.contains(row) else { return nil }
        return feed.rows[row]
    }

    @objc private func historyBoundsDidChange() {
        feed.markActivity()
        if !isScrollingProgrammatically {
            tableView.selectRowUnderPointer()
        }
        let visibleRows = tableView.rows(in: tableView.visibleRect)
        guard visibleRows.location != NSNotFound else { return }
        if visibleRows.location <= Self.pagingRowMargin, feed.hasMoreNewer {
            feed.loadPage(.newer)
        } else if
            NSMaxRange(visibleRows) >= max(0, feed.rows.count - Self.pagingRowMargin),
            feed.hasMoreOlder
        {
            feed.loadPage(.older)
        }
    }

    // MARK: - Row updates

    /// Keeps the reader's place across a row change: where the viewport sits and
    /// what is selected are read before the mutation and put back after it.
    private func updateRows(reset: Bool, apply: () -> Void) {
        let anchor = reset ? nil : visibleAnchor()
        let selectedId = selectedSummary?.id

        apply()

        withProgrammaticScroll {
            tableView.reloadData()
            if let selectedId, let row = feed.rows.firstIndex(where: { $0.id == selectedId }) {
                tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            } else if reset {
                tableView.deselectAll(nil)
            }
            if let anchor {
                restoreVisibleAnchor(anchor)
            } else if !feed.rows.isEmpty, tableView.selectedRow < 0 {
                tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
            }
        }
    }

    private func redrawRow(id: Int64) {
        guard let row = feed.rows.firstIndex(where: { $0.id == id }) else { return }
        tableView.reloadData(
            forRowIndexes: IndexSet(integer: row),
            columnIndexes: IndexSet(integer: 0)
        )
    }

    private func visibleAnchor() -> (id: Int64, offset: CGFloat)? {
        guard let historyClipView else { return nil }
        let row = tableView.row(at: NSPoint(x: 1, y: tableView.visibleRect.minY + 1))
        guard feed.rows.indices.contains(row) else { return nil }
        return (
            feed.rows[row].id,
            historyClipView.bounds.minY - tableView.rect(ofRow: row).minY
        )
    }

    private func restoreVisibleAnchor(_ anchor: (id: Int64, offset: CGFloat)) {
        guard
            let historyClipView,
            let row = feed.rows.firstIndex(where: { $0.id == anchor.id })
        else { return }
        var origin = historyClipView.bounds.origin
        origin.y = max(0, tableView.rect(ofRow: row).minY + anchor.offset)
        historyClipView.setBoundsOrigin(origin)
        historyClipView.enclosingScrollView?.reflectScrolledClipView(historyClipView)
    }

    // MARK: - Table data source and delegate

    func numberOfRows(in tableView: NSTableView) -> Int {
        feed.rows.count
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        feed.rows[row].hasImagePreview
            ? configuration.rows.imageHeight
            : configuration.rows.textHeight
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let identifier = NSUserInterfaceItemIdentifier("HistoryRow")
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? HistoryRowView {
            return reused
        }
        return HistoryRowView()
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("HistoryCell")
        let cell: HistoryCellView
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? HistoryCellView {
            cell = reused
        } else {
            cell = HistoryCellView(configuration: configuration)
        }
        let summary = feed.rows[row]
        cell.clipId = summary.id
        cell.configureForImagePreview(summary.hasImagePreview)
        let preview = (summary.preview ?? "[\(summary.kind)]")
            .replacingOccurrences(of: "\n", with: " ")
        cell.previewLabel.stringValue = preview
        cell.previewLabel.isHidden = summary.hasImagePreview && summary.preview == nil
        cell.metadataLabel.stringValue = "\(summary.kind) · \(ByteCountFormatter.string(fromByteCount: Int64(summary.payloadSize), countStyle: .file))"
        cell.thumbnailImageView.image = Self.placeholderImage(for: summary.kind)
        if summary.hasImagePreview {
            if let image = previewLoader.cachedImage(for: summary.id) {
                cell.thumbnailImageView.image = image
            } else {
                previewLoader.load(id: summary.id)
            }
        }
        cell.toolTip = "\(summary.payloadSize) bytes · copied \(summary.copyCount)回"
        return cell
    }

    private static func placeholderImage(for kind: String) -> NSImage? {
        let symbolName: String
        switch kind {
        case "image": symbolName = "photo"
        case "file": symbolName = "doc"
        case "mixed": symbolName = "square.stack.3d.up"
        default: symbolName = "doc.text"
        }
        return NSImage(systemSymbolName: symbolName, accessibilityDescription: kind)
    }
}
