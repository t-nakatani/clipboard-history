import AppKit
import ImageIO

enum ImagePreviewGenerator {
    private static let sourceTypes = ["public.png", "public.tiff"]
    private static let maximumPixelSize = 96
    private static let maximumEncodedBytes = 64 * 1024

    /// Produces a small derived PNG while the original clipboard bytes are
    /// already resident. The original is never decoded again for list display.
    static func makePreview(from representations: [RepresentationDto]) -> RepresentationDto? {
        guard let sourceRepresentation = sourceTypes.lazy.compactMap({ uti in
            representations.first(where: { $0.uti == uti })
        }).first else {
            return nil
        }

        guard let source = CGImageSourceCreateWithData(sourceRepresentation.bytes as CFData, nil) else {
            return nil
        }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maximumPixelSize,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
            return nil
        }

        let bitmap = NSBitmapImageRep(cgImage: thumbnail)
        guard
            let encoded = bitmap.representation(
                using: .jpeg,
                properties: [.compressionFactor: 0.58]
            ),
            !encoded.isEmpty,
            encoded.count <= maximumEncodedBytes
        else {
            return nil
        }
        return RepresentationDto(uti: "public.jpeg", bytes: encoded)
    }
}
