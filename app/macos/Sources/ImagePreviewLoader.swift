import AppKit

/// Decodes row thumbnails once and keeps a bounded number of them resident.
///
/// Table views ask for the same row repeatedly while scrolling, so requests are
/// coalesced per clip id; `didLoad` tells the view which row became drawable.
final class ImagePreviewLoader {
    typealias Fetch = (Int64, @escaping (RepresentationDto?) -> Void) -> Void

    /// Previews are bounded to 96px by `ImagePreviewGenerator`, so this is the
    /// worst-case cost of one decoded bitmap.
    private static let decodedPreviewCost = 96 * 96 * 4

    private let cache: NSCache<NSNumber, NSImage> = {
        let cache = NSCache<NSNumber, NSImage>()
        cache.countLimit = 64
        cache.totalCostLimit = 4 * 1024 * 1024
        return cache
    }()
    private var inFlight: Set<Int64> = []
    private let fetch: Fetch

    var didLoad: ((Int64) -> Void)?

    init(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    func cachedImage(for id: Int64) -> NSImage? {
        cache.object(forKey: NSNumber(value: id))
    }

    func load(id: Int64) {
        guard inFlight.insert(id).inserted else { return }
        fetch(id) { [weak self] preview in
            guard let self else { return }
            self.inFlight.remove(id)
            guard let preview, let image = NSImage(data: preview.bytes) else { return }
            self.cache.setObject(
                image,
                forKey: NSNumber(value: id),
                cost: Self.decodedPreviewCost
            )
            self.didLoad?(id)
        }
    }
}
