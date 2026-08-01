import AppKit
import Foundation
import ImageIO

// The production generator only needs these two fields from the UniFFI DTO.
// Defining the same shape here lets this PoC compile the production source
// directly instead of maintaining a second preview implementation.
struct RepresentationDto {
    let uti: String
    let bytes: Data
}

private struct Fixture {
    let name: String
    let source: RepresentationDto
    let expectsAlpha: Bool
}

private struct Metrics: Codable {
    let name: String
    let sourceBytes: Int
    let sourceWidth: Int
    let sourceHeight: Int
    let sourceHasAlpha: Bool
    let previewUti: String
    let previewBytes: Int
    let previewWidth: Int
    let previewHeight: Int
    let previewHasAlpha: Bool
    let generationP50Ms: Double
    let generationP95Ms: Double
    let generationMaxMs: Double
    let psnrDb: Double
    let projected100kBytes: Int64
}

private enum PocError: Error, CustomStringConvertible {
    case usage
    case bitmapCreation
    case sourceEncoding(String)
    case previewGeneration(String)
    case imageDecode(String)

    var description: String {
        switch self {
        case .usage:
            return "usage: image-preview-poc OUTPUT_DIRECTORY [ITERATIONS]"
        case .bitmapCreation:
            return "could not allocate bitmap fixture"
        case let .sourceEncoding(name):
            return "could not encode source fixture: \(name)"
        case let .previewGeneration(name):
            return "production generator rejected fixture: \(name)"
        case let .imageDecode(name):
            return "could not decode image metadata: \(name)"
        }
    }
}

private func percentile(_ sorted: [Double], _ fraction: Double) -> Double {
    let index = min(sorted.count - 1, Int((Double(sorted.count - 1) * fraction).rounded(.up)))
    return sorted[index]
}

private func makeBitmap(
    width: Int,
    height: Int,
    pixel: (_ x: Int, _ y: Int) -> (UInt8, UInt8, UInt8, UInt8)
) throws -> NSBitmapImageRep {
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: width,
        pixelsHigh: height,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bitmapFormat: [],
        bytesPerRow: width * 4,
        bitsPerPixel: 32
    ), let bytes = bitmap.bitmapData else {
        throw PocError.bitmapCreation
    }

    for y in 0 ..< height {
        for x in 0 ..< width {
            let offset = y * bitmap.bytesPerRow + x * 4
            let value = pixel(x, y)
            bytes[offset] = value.0
            bytes[offset + 1] = value.1
            bytes[offset + 2] = value.2
            bytes[offset + 3] = value.3
        }
    }
    return bitmap
}

private func encodePng(_ bitmap: NSBitmapImageRep, name: String) throws -> Data {
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        throw PocError.sourceEncoding(name)
    }
    return data
}

private func makeFixtures() throws -> [Fixture] {
    let width = 1440
    let height = 900

    let photo = try makeBitmap(width: width, height: height) { x, y in
        let seed = UInt32(truncatingIfNeeded: x &* 73_856_093 ^ y &* 19_349_663)
        let noise = Int((seed ^ (seed >> 13) ^ (seed << 7)) & 31) - 15
        let horizon = Double(y) / Double(height)
        let red = max(0, min(255, Int(38 + 165 * horizon) + noise))
        let green = max(0, min(255, Int(105 + 105 * (1 - horizon)) + noise / 2))
        let blue = max(0, min(255, Int(175 + 55 * (1 - horizon)) + noise / 3))
        return (UInt8(red), UInt8(green), UInt8(blue), 255)
    }

    let screenshot = try makeBitmap(width: width, height: height) { x, y in
        if y < 72 { return (38, 42, 48, 255) }
        if x < 230 { return (232, 235, 239, 255) }
        if (y - 110) % 72 < 18, x > 275, x < 1260 {
            let shade = UInt8(62 + ((y / 72) % 4) * 24)
            return (shade, shade, shade, 255)
        }
        if x > 275, x < 1160, y > 115, (x / 11 + y / 7) % 29 == 0 {
            return (41, 112, 210, 255)
        }
        return (248, 248, 246, 255)
    }

    let transparent = try makeBitmap(width: width, height: height) { x, y in
        let dx = Double(x - width / 2)
        let dy = Double(y - height / 2)
        let distance = sqrt(dx * dx + dy * dy)
        if distance < 310 {
            let alpha = UInt8(max(32, min(230, Int(230 - distance / 2))))
            // Store premultiplied RGB because NSBitmapImageRep expects it.
            return (
                UInt8(Int(52) * Int(alpha) / 255),
                UInt8(Int(132) * Int(alpha) / 255),
                UInt8(Int(235) * Int(alpha) / 255),
                alpha
            )
        }
        if abs(x - y) < 24 || abs((width - x) - y) < 24 {
            return (215, 54, 82, 215)
        }
        return (0, 0, 0, 0)
    }

    return [
        Fixture(
            name: "photo-like",
            source: RepresentationDto(uti: "public.png", bytes: try encodePng(photo, name: "photo-like")),
            expectsAlpha: false
        ),
        Fixture(
            name: "screenshot",
            source: RepresentationDto(uti: "public.png", bytes: try encodePng(screenshot, name: "screenshot")),
            expectsAlpha: false
        ),
        Fixture(
            name: "transparent",
            source: RepresentationDto(uti: "public.png", bytes: try encodePng(transparent, name: "transparent")),
            expectsAlpha: true
        ),
    ]
}

private func imageInfo(_ data: Data, name: String) throws -> (Int, Int, Bool) {
    guard
        let source = CGImageSourceCreateWithData(data as CFData, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        throw PocError.imageDecode(name)
    }
    let hasAlpha: Bool
    switch image.alphaInfo {
    case .none, .noneSkipFirst, .noneSkipLast:
        hasAlpha = false
    default:
        hasAlpha = true
    }
    return (image.width, image.height, hasAlpha)
}

private func rgba(_ data: Data, width: Int, height: Int) throws -> [UInt8] {
    guard
        let source = CGImageSourceCreateWithData(data as CFData, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        throw PocError.imageDecode("PSNR input")
    }
    var pixels = [UInt8](repeating: 255, count: width * height * 4)
    let created = pixels.withUnsafeMutableBytes { raw -> Bool in
        guard let base = raw.baseAddress,
              let context = CGContext(
                  data: base,
                  width: width,
                  height: height,
                  bitsPerComponent: 8,
                  bytesPerRow: width * 4,
                  space: CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
              ) else {
            return false
        }
        context.setFillColor(CGColor(gray: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return true
    }
    guard created else { throw PocError.bitmapCreation }
    return pixels
}

private func psnr(reference: Data, preview: Data, width: Int, height: Int) throws -> Double {
    let expected = try rgba(reference, width: width, height: height)
    let actual = try rgba(preview, width: width, height: height)
    var squaredError = 0.0
    for pixel in 0 ..< (width * height) {
        for channel in 0 ..< 3 {
            let offset = pixel * 4 + channel
            let difference = Double(Int(expected[offset]) - Int(actual[offset]))
            squaredError += difference * difference
        }
    }
    let meanSquaredError = squaredError / Double(width * height * 3)
    return meanSquaredError == 0 ? .infinity : 10 * log10(255 * 255 / meanSquaredError)
}

private func run() throws {
    guard CommandLine.arguments.count >= 2 else { throw PocError.usage }
    let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
    let iterations = CommandLine.arguments.count >= 3 ? Int(CommandLine.arguments[2]) ?? 200 : 200
    guard iterations > 0 else { throw PocError.usage }
    try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

    var allMetrics: [Metrics] = []
    for fixture in try makeFixtures() {
        _ = ImagePreviewGenerator.makePreview(from: [fixture.source])
        var durations: [Double] = []
        var lastPreview: RepresentationDto?
        for _ in 0 ..< iterations {
            let start = DispatchTime.now().uptimeNanoseconds
            lastPreview = autoreleasepool {
                ImagePreviewGenerator.makePreview(from: [fixture.source])
            }
            let elapsed = DispatchTime.now().uptimeNanoseconds - start
            durations.append(Double(elapsed) / 1_000_000)
        }
        guard let preview = lastPreview else { throw PocError.previewGeneration(fixture.name) }
        durations.sort()

        let sourceInfo = try imageInfo(fixture.source.bytes, name: fixture.name)
        let previewInfo = try imageInfo(preview.bytes, name: fixture.name)
        let quality = try psnr(
            reference: fixture.source.bytes,
            preview: preview.bytes,
            width: previewInfo.0,
            height: previewInfo.1
        )
        let previewExtension = preview.uti == "public.png" ? "png" : "jpg"
        try fixture.source.bytes.write(to: output.appendingPathComponent("\(fixture.name)-source.png"))
        try preview.bytes.write(to: output.appendingPathComponent("\(fixture.name)-preview.\(previewExtension)"))

        let metrics = Metrics(
            name: fixture.name,
            sourceBytes: fixture.source.bytes.count,
            sourceWidth: sourceInfo.0,
            sourceHeight: sourceInfo.1,
            sourceHasAlpha: fixture.expectsAlpha && sourceInfo.2,
            previewUti: preview.uti,
            previewBytes: preview.bytes.count,
            previewWidth: previewInfo.0,
            previewHeight: previewInfo.1,
            previewHasAlpha: previewInfo.2,
            generationP50Ms: percentile(durations, 0.50),
            generationP95Ms: percentile(durations, 0.95),
            generationMaxMs: durations.last ?? 0,
            psnrDb: quality,
            projected100kBytes: Int64(preview.bytes.count) * 100_000
        )
        allMetrics.append(metrics)
        print(String(format:
            "%@ source=%dB preview=%dB %dx%d p50=%.3fms p95=%.3fms PSNR=%.2fdB alpha=%@",
            fixture.name,
            metrics.sourceBytes,
            metrics.previewBytes,
            metrics.previewWidth,
            metrics.previewHeight,
            metrics.generationP50Ms,
            metrics.generationP95Ms,
            metrics.psnrDb,
            metrics.previewHasAlpha ? "yes" : "no"
        ))
    }

    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(allMetrics).write(to: output.appendingPathComponent("metrics.json"))
}

do {
    try run()
} catch {
    fputs("image preview PoC failed: \(error)\n", stderr)
    exit(1)
}
