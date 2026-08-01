import AppKit

/// Central tuning surface for the menu-bar history UI.
///
/// Change the values in `standard` to iterate on layout without hunting
/// through AppDelegate, HistoryPanel, and cell implementation details.
struct HistoryPanelConfiguration {
    struct Window {
        let width: CGFloat
        let initialHeight: CGFloat
        let minimumHeight: CGFloat
        let screenHeightFraction: CGFloat
        let anchorGap: CGFloat
    }

    struct Content {
        let topInset: CGFloat
        let leadingInset: CGFloat
        let bottomInset: CGFloat
        let trailingInset: CGFloat
        let sectionSpacing: CGFloat
        let searchItemSpacing: CGFloat
        let minimumHistoryHeight: CGFloat
    }

    struct Rows {
        let textHeight: CGFloat
        let imageHeight: CGFloat
        let intercellSpacing: CGFloat
        let leadingInset: CGFloat
        let trailingInset: CGFloat
        let imageVerticalInset: CGFloat
        let imageWidth: CGFloat
        let imageToTextSpacing: CGFloat
        let textLeadingSpacing: CGFloat
        let textToMetadataSpacing: CGFloat
        let metadataMaximumWidth: CGFloat
    }

    struct Typography {
        let textSize: CGFloat
        let textWeight: NSFont.Weight
        let metadataSize: CGFloat
        let appNameSize: CGFloat
        let appNameWeight: NSFont.Weight
        let statusSize: CGFloat
        let detailSize: CGFloat
    }

    struct Appearance {
        let cornerRadius: CGFloat
        let borderWidth: CGFloat
        let borderColor: NSColor
        let blurAlpha: CGFloat
        let tintColor: NSColor
        let thumbnailCornerRadius: CGFloat
        let thumbnailBackgroundColor: NSColor
    }

    let window: Window
    let content: Content
    let rows: Rows
    let typography: Typography
    let appearance: Appearance

    // MARK: - Edit UI values here

    static let standard = HistoryPanelConfiguration(
        window: Window(
            width: 700,
            initialHeight: 440,
            minimumHeight: 440,
            screenHeightFraction: 0.92,
            anchorGap: 6
        ),
        content: Content(
            topInset: 6,
            leadingInset: 2,
            bottomInset: 8,
            trailingInset: 3,
            sectionSpacing: 3,
            searchItemSpacing: 10,
            minimumHistoryHeight: 380
        ),
        rows: Rows(
            textHeight: 22,
            imageHeight: 60,
            intercellSpacing: 0,
            leadingInset: 1,
            trailingInset: 8,
            imageVerticalInset: 4,
            imageWidth: 80,
            imageToTextSpacing: 10,
            textLeadingSpacing: 1,
            textToMetadataSpacing: 10,
            metadataMaximumWidth: 116
        ),
        typography: Typography(
            textSize: 12,
            textWeight: .regular,
            metadataSize: 11.5,
            appNameSize: 13,
            appNameWeight: .semibold,
            statusSize: 16,
            detailSize: 12
        ),
        appearance: Appearance(
            cornerRadius: 12,
            borderWidth: 0.75,
            borderColor: NSColor.white.withAlphaComponent(0.20),
            blurAlpha: 0.76,
            tintColor: NSColor(
                calibratedRed: 0.10,
                green: 0.12,
                blue: 0.20,
                alpha: 0.18
            ),
            thumbnailCornerRadius: 6,
            thumbnailBackgroundColor: NSColor.white.withAlphaComponent(0.07)
        )
    )
}
