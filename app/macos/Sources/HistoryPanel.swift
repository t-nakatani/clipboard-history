import AppKit

final class TranslucentPanelContentView: NSView {
    let foregroundView = NSView()

    init(frame frameRect: NSRect, configuration: HistoryPanelConfiguration) {
        super.init(frame: frameRect)

        appearance = NSAppearance(named: .darkAqua)
        wantsLayer = true
        layer?.cornerRadius = configuration.appearance.cornerRadius
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true
        layer?.borderWidth = configuration.appearance.borderWidth
        layer?.borderColor = configuration.appearance.borderColor.cgColor

        let blurView = NSVisualEffectView()
        blurView.material = .hudWindow
        blurView.blendingMode = .behindWindow
        blurView.state = .active
        // Fade only the backdrop. Text and controls live in foregroundView and
        // therefore retain full contrast while the desktop remains visible.
        blurView.alphaValue = configuration.appearance.blurAlpha

        let tintView = NSView()
        tintView.wantsLayer = true
        tintView.layer?.backgroundColor = configuration.appearance.tintColor.cgColor

        for view in [blurView, tintView, foregroundView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
            NSLayoutConstraint.activate([
                view.leadingAnchor.constraint(equalTo: leadingAnchor),
                view.trailingAnchor.constraint(equalTo: trailingAnchor),
                view.topAnchor.constraint(equalTo: topAnchor),
                view.bottomAnchor.constraint(equalTo: bottomAnchor),
            ])
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

/// Rows of the history list highlight like a menu: the selection follows the
/// pointer and stays accent-coloured even though the search field, not the
/// table, holds first responder.
final class HistoryRowView: NSTableRowView {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        identifier = NSUserInterfaceItemIdentifier("HistoryRow")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override var isEmphasized: Bool {
        get { true }
        set {}
    }
}

final class HistoryTableView: NSTableView {
    /// The click count of the mouse-down that led to the current action, read
    /// instead of `NSApp.currentEvent` so the action never misreads the event
    /// it was sent for.
    private(set) var lastClickCount = 0

    private var hoverTrackingArea: NSTrackingArea?

    /// Every key belongs to the search field, which routes what the list needs
    /// back to it. Letting the table take focus would silently redirect typing
    /// into its own type-select.
    override var acceptsFirstResponder: Bool { false }

    override func mouseDown(with event: NSEvent) {
        lastClickCount = event.clickCount
        super.mouseDown(with: event)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverTrackingArea {
            removeTrackingArea(hoverTrackingArea)
        }
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        hoverTrackingArea = area
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        selectRow(at: convert(event.locationInWindow, from: nil))
    }

    /// Re-runs the hover selection after the rows moved under a stationary
    /// pointer, which mouse-moved events alone would not report.
    func selectRowUnderPointer() {
        guard let window, window.isVisible else { return }
        let point = convert(window.convertPoint(fromScreen: NSEvent.mouseLocation), from: nil)
        guard visibleRect.contains(point) else { return }
        selectRow(at: point)
    }

    /// Moves the selection to whatever row sits under the pointer. Points
    /// outside the rows keep the current selection so the panel never ends up
    /// with nothing to restore.
    private func selectRow(at point: NSPoint) {
        let row = self.row(at: point)
        guard row >= 0, row != selectedRow else { return }
        selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    }
}

/// A menu-like panel anchored to the menu bar status item.
///
/// The panel remains a real key window so controls such as NSSearchField and
/// NSTableView keep their normal AppKit keyboard behaviour. Presentation and
/// dismissal are owned here to keep menu-bar geometry out of AppDelegate.
final class HistoryPanel: NSPanel {
    private let configuration: HistoryPanelConfiguration
    var visibilityDidChange: ((Bool) -> Void)?

    init(configuration: HistoryPanelConfiguration) {
        self.configuration = configuration
        super.init(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: configuration.window.width,
                height: configuration.window.initialHeight
            ),
            styleMask: [.nonactivatingPanel, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = false
        level = .popUpMenu
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isMovable = false
        isMovableByWindowBackground = false
        hidesOnDeactivate = true
        // The history list highlights the row under the pointer, so the panel
        // takes mouse-moved events rather than dropping them.
        acceptsMouseMovedEvents = true
        animationBehavior = .utilityWindow
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    func toggle(relativeTo statusButton: NSStatusBarButton, firstResponder: NSResponder?) {
        if isVisible {
            dismiss()
        } else {
            present(relativeTo: statusButton, firstResponder: firstResponder)
        }
    }

    func present(relativeTo statusButton: NSStatusBarButton, firstResponder: NSResponder?) {
        position(relativeTo: statusButton)
        makeKeyAndOrderFront(nil)
        if let firstResponder {
            makeFirstResponder(firstResponder)
        }
        visibilityDidChange?(true)
    }

    func dismiss() {
        guard isVisible else { return }
        orderOut(nil)
        visibilityDidChange?(false)
    }

    override func resignKey() {
        super.resignKey()
        dismiss()
    }

    override func cancelOperation(_ sender: Any?) {
        dismiss()
    }

    private func position(relativeTo statusButton: NSStatusBarButton) {
        guard let buttonWindow = statusButton.window else {
            center()
            return
        }

        let buttonRectInWindow = statusButton.convert(statusButton.bounds, to: nil)
        let anchorRect = buttonWindow.convertToScreen(buttonRectInWindow)
        let targetScreen = buttonWindow.screen ?? NSScreen.screens.first
        guard let targetScreen else {
            center()
            return
        }

        let visibleFrame = targetScreen.visibleFrame
        let preferredHeight = max(
            configuration.window.minimumHeight,
            floor(visibleFrame.height * configuration.window.screenHeightFraction)
        )
        setContentSize(NSSize(width: frame.width, height: min(preferredHeight, visibleFrame.height)))
        let maximumX = max(visibleFrame.minX, visibleFrame.maxX - frame.width)
        let centeredX = anchorRect.midX - frame.width / 2
        let x = min(max(centeredX, visibleFrame.minX), maximumX)

        let maximumY = max(visibleFrame.minY, visibleFrame.maxY - frame.height)
        let belowMenuBarY = anchorRect.minY - frame.height - configuration.window.anchorGap
        let y = min(max(belowMenuBarY, visibleFrame.minY), maximumY)
        setFrameOrigin(NSPoint(x: x.rounded(), y: y.rounded()))
    }
}
