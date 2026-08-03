import AppKit

/// Identifies the bytes behind a row's thumbnail.
///
/// A clip's id alone is not that identity. `clips.id` is a plain
/// `INTEGER PRIMARY KEY`, so SQLite hands the id of a deleted newest clip
/// straight to the next capture, and anything keyed on the id alone answers for
/// the new clip with the deleted clip's picture. `last_used_at` moves on every
/// capture and every recopy, so pairing the two tells a reused id apart from the
/// row it replaced.
///
/// The cost is a refetch after a recopy of an image, which is one bounded read
/// of at most 64KiB off the main thread.
struct PreviewIdentity: Hashable {
    let id: Int64
    let lastUsedAtMs: Int64

    init(_ summary: ClipSummaryDto) {
        id = summary.id
        lastUsedAtMs = summary.lastUsedAtMs
    }

    var cacheKey: NSString { "\(id):\(lastUsedAtMs)" as NSString }
}

/// Decodes row thumbnails once and keeps a bounded number of them resident.
///
/// Table views ask for the same row repeatedly while scrolling, so requests are
/// coalesced per clip, and `didLoad` tells the view which row became drawable.
final class ImagePreviewLoader {
    typealias Fetch = (Int64, @escaping (RepresentationDto?) -> Void) -> Void

    /// Previews are bounded to 96px by `ImagePreviewGenerator`, so this is the
    /// worst-case cost of one decoded bitmap.
    private static let decodedPreviewCost = 96 * 96 * 4

    private let cache: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>()
        cache.countLimit = 64
        cache.totalCostLimit = 4 * 1024 * 1024
        return cache
    }()
    private var inFlight: Set<PreviewIdentity> = []
    private let fetch: Fetch

    var didLoad: ((Int64) -> Void)?

    init(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    func cachedImage(for summary: ClipSummaryDto) -> NSImage? {
        cache.object(forKey: PreviewIdentity(summary).cacheKey)
    }

    func load(_ summary: ClipSummaryDto) {
        let identity = PreviewIdentity(summary)
        guard inFlight.insert(identity).inserted else { return }
        fetch(identity.id) { [weak self] preview in
            guard let self else { return }
            self.inFlight.remove(identity)
            guard let preview, let image = NSImage(data: preview.bytes) else { return }
            // Stored under the identity the request was made for. A capture that
            // lands while this is in flight moves the row to a new identity, and
            // that row asks for its own preview rather than reading this one.
            self.cache.setObject(
                image,
                forKey: identity.cacheKey,
                cost: Self.decodedPreviewCost
            )
            self.didLoad?(identity.id)
        }
    }
}
