import AppKit

/// One history row: an optional thumbnail, the preview text, and the metadata
/// trailing it. Image rows hide the metadata to leave the preview more width.
final class HistoryCellView: NSTableCellView {
    let thumbnailImageView = NSImageView()
    let previewLabel = NSTextField(labelWithString: "")
    let metadataLabel = NSTextField(labelWithString: "")
    var clipId: Int64?
    private var thumbnailWidthConstraint: NSLayoutConstraint!
    private var thumbnailGapConstraint: NSLayoutConstraint!
    private let configuration: HistoryPanelConfiguration

    init(frame frameRect: NSRect = .zero, configuration: HistoryPanelConfiguration) {
        self.configuration = configuration
        super.init(frame: frameRect)
        identifier = NSUserInterfaceItemIdentifier("HistoryCell")

        thumbnailImageView.imageScaling = .scaleProportionallyUpOrDown
        thumbnailImageView.imageAlignment = .alignCenter
        thumbnailImageView.contentTintColor = .secondaryLabelColor
        thumbnailImageView.wantsLayer = true
        thumbnailImageView.layer?.cornerRadius = configuration.appearance.thumbnailCornerRadius
        thumbnailImageView.layer?.cornerCurve = .continuous
        thumbnailImageView.layer?.masksToBounds = true
        thumbnailImageView.layer?.backgroundColor = configuration.appearance.thumbnailBackgroundColor.cgColor
        thumbnailImageView.translatesAutoresizingMaskIntoConstraints = false

        previewLabel.lineBreakMode = .byTruncatingTail
        previewLabel.font = .systemFont(
            ofSize: configuration.typography.textSize,
            weight: configuration.typography.textWeight
        )
        previewLabel.textColor = .labelColor
        previewLabel.translatesAutoresizingMaskIntoConstraints = false
        textField = previewLabel

        metadataLabel.lineBreakMode = .byTruncatingTail
        metadataLabel.font = .systemFont(ofSize: configuration.typography.metadataSize)
        metadataLabel.textColor = .tertiaryLabelColor
        metadataLabel.alignment = .right
        metadataLabel.translatesAutoresizingMaskIntoConstraints = false

        addSubview(thumbnailImageView)
        addSubview(previewLabel)
        addSubview(metadataLabel)
        thumbnailWidthConstraint = thumbnailImageView.widthAnchor.constraint(equalToConstant: 0)
        thumbnailGapConstraint = previewLabel.leadingAnchor.constraint(
            equalTo: thumbnailImageView.trailingAnchor,
            constant: configuration.rows.textLeadingSpacing
        )
        NSLayoutConstraint.activate([
            thumbnailImageView.leadingAnchor.constraint(
                equalTo: leadingAnchor,
                constant: configuration.rows.leadingInset
            ),
            thumbnailImageView.topAnchor.constraint(
                equalTo: topAnchor,
                constant: configuration.rows.imageVerticalInset
            ),
            thumbnailImageView.bottomAnchor.constraint(
                equalTo: bottomAnchor,
                constant: -configuration.rows.imageVerticalInset
            ),
            thumbnailWidthConstraint,
            thumbnailGapConstraint,
            previewLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            metadataLabel.leadingAnchor.constraint(
                greaterThanOrEqualTo: previewLabel.trailingAnchor,
                constant: configuration.rows.textToMetadataSpacing
            ),
            metadataLabel.trailingAnchor.constraint(
                equalTo: trailingAnchor,
                constant: -configuration.rows.trailingInset
            ),
            metadataLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            metadataLabel.widthAnchor.constraint(
                lessThanOrEqualToConstant: configuration.rows.metadataMaximumWidth
            ),
        ])
    }

    /// `previewLabel` is the cell's `textField` and follows the background on
    /// its own. The metadata is ours to keep readable on the accent fill.
    override var backgroundStyle: NSView.BackgroundStyle {
        didSet {
            metadataLabel.textColor = backgroundStyle == .emphasized
                ? NSColor.alternateSelectedControlTextColor.withAlphaComponent(0.75)
                : .tertiaryLabelColor
        }
    }

    func configureForImagePreview(_ showsImagePreview: Bool) {
        thumbnailImageView.isHidden = !showsImagePreview
        thumbnailWidthConstraint.constant = showsImagePreview ? configuration.rows.imageWidth : 0
        thumbnailGapConstraint.constant = showsImagePreview
            ? configuration.rows.imageToTextSpacing
            : configuration.rows.textLeadingSpacing
        metadataLabel.isHidden = showsImagePreview
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}
