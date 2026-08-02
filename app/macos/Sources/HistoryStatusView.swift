import AppKit

/// The panel's footer: a headline and a detail line describing what the app just
/// did.
///
/// Both labels existed before this type, fully configured and written to from a
/// dozen places, but never added to any view hierarchy — every message went
/// nowhere. Owning them here is what puts them on screen, and gives the one
/// place that decides whether a message may be replaced.
final class HistoryStatusView: NSStackView {
    private let headlineLabel = NSTextField(labelWithString: "コピー待機中")
    private let detailLabel = NSTextField(labelWithString: "コピーすると自動で履歴に追加されます。")

    /// Set when an important status was raised while the panel was closed, so
    /// the routine chatter that follows cannot bury it before it is ever seen.
    private var hasUnseenImportantStatus = false
    /// Whether the row is currently in front of the user. Without it, every
    /// important status would stick until the panel happened to open.
    var isOnScreen: (() -> Bool)?

    init(configuration: HistoryPanelConfiguration) {
        super.init(frame: .zero)

        headlineLabel.font = .systemFont(
            ofSize: configuration.typography.statusSize,
            weight: .semibold
        )
        headlineLabel.lineBreakMode = .byTruncatingTail
        headlineLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        headlineLabel.setContentHuggingPriority(.required, for: .horizontal)

        detailLabel.textColor = .secondaryLabelColor
        detailLabel.font = .systemFont(ofSize: configuration.typography.detailSize)
        // One line only: the row has a fixed height so the panel keeps its
        // history area. Long detail (paths, error text) stays in the tooltip.
        detailLabel.maximumNumberOfLines = 1
        detailLabel.lineBreakMode = .byTruncatingTail
        detailLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        setViews([headlineLabel, detailLabel], in: .leading)
        orientation = .horizontal
        alignment = .firstBaseline
        spacing = configuration.content.searchItemSpacing
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// What is on the row right now. Read by the self-test, which has no panel
    /// to look at.
    var displayedHeadline: String { headlineLabel.stringValue }
    var displayedDetail: String { detailLabel.stringValue }

    /// Presents a status, unless an important one is still waiting to be read.
    func show(_ status: HistoryStatus) {
        if status.priority == .routine, hasUnseenImportantStatus { return }
        if status.priority == .important {
            hasUnseenImportantStatus = !(isOnScreen?() ?? false)
        }
        if let headline = status.headline {
            headlineLabel.stringValue = headline
        }
        if let detail = status.detail {
            detailLabel.stringValue = detail
            detailLabel.toolTip = detail
        }
    }

    /// Called when the panel becomes visible: whatever is on the row has now
    /// been delivered, so routine updates may take it back over.
    func markSeen() {
        hasUnseenImportantStatus = false
    }
}
