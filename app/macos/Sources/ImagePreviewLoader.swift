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
///
/// This pair is not unique on its own. `last_used_at` is `copied_at_ms`, which
/// `HistoryFeedModel` reads off the wall clock at millisecond resolution, and
/// the store enforces neither monotonicity nor uniqueness on it. A capture that
/// inherits a deleted clip's id in the same millisecond the deleted clip last
/// carried therefore lands on the same identity. `ImagePreviewLoader.invalidate`
/// is what closes that window; the pair is what keeps every other path — recopy,
/// prune, a row that changed while a fetch was in flight — from needing one.
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
    /// Bumped by `invalidate`. A fetch carries the generation it started in, so
    /// a read that was already under way when a clip was removed cannot put the
    /// removed clip's bytes back into a cache that was just emptied.
    private var generation = 0
    private let fetch: Fetch

    var didLoad: ((Int64) -> Void)?

    init(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    func cachedImage(for summary: ClipSummaryDto) -> NSImage? {
        cache.object(forKey: PreviewIdentity(summary).cacheKey)
    }

    /// Drops every resident preview. Called when a clip leaves the store,
    /// because that is what returns its id to SQLite for the next capture to
    /// take, and a reused id can arrive carrying the same `last_used_at` the
    /// deleted row had. Nothing here is keyed on which clip went away, so it
    /// costs no assumption about which id gets reused or which rows the removal
    /// touched.
    ///
    /// Deletes are single keystrokes and already reload the feed, so the price
    /// is one bounded refetch for each image row still on screen.
    func invalidate() {
        generation &+= 1
        // Requests already recorded belong to the previous generation and will
        // be dropped on arrival, so the rows they were for have to be free to
        // ask again.
        inFlight.removeAll()
        cache.removeAllObjects()
    }

    func load(_ summary: ClipSummaryDto) {
        let identity = PreviewIdentity(summary)
        guard inFlight.insert(identity).inserted else { return }
        let generation = generation
        fetch(identity.id) { [weak self] preview in
            guard let self, generation == self.generation else { return }
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
