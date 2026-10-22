import CoreGraphics
import Foundation

/// A premultiplied BGRA (little-endian, alpha first) pixel buffer: the format CoreAnimation takes without conversion.
public struct Canvas {
    public let width: Int
    public let height: Int
    public var pixels: [UInt8]

    public init(width: Int, height: Int) {
        self.width = max(width, 1)
        self.height = max(height, 1)
        pixels = [UInt8](repeating: 0, count: self.width * self.height * 4)
    }

    /// Blends an 8-bit coverage mask in one colour (straight RGB, `alpha` 0...255) at (x, y): source-over.
    public mutating func blendMask(
        _ mask: UnsafePointer<UInt8>, w: Int, h: Int, stride: Int, x: Int, y: Int, r: UInt8, g: UInt8, b: UInt8, alpha: UInt8
    ) {
        guard alpha > 0 else { return }
        let x0 = max(0, x), y0 = max(0, y), x1 = min(width, x + w), y1 = min(height, y + h)
        guard x0 < x1, y0 < y1 else { return }
        let a = UInt32(alpha), cr = UInt32(r), cg = UInt32(g), cb = UInt32(b)
        let rowBytes = width * 4
        pixels.withUnsafeMutableBufferPointer { dst in
            for row in y0..<y1 {
                let src = mask + (row - y) * stride + (x0 - x)
                var d = dst.baseAddress! + row * rowBytes + x0 * 4
                for i in 0..<(x1 - x0) {
                    let k = UInt32(src[i])
                    if k != 0 {
                        let sa = (k * a + 127) / 255  // effective source alpha
                        let inv = 255 - sa
                        d[0] = UInt8((cb * sa + UInt32(d[0]) * inv + 127) / 255)
                        d[1] = UInt8((cg * sa + UInt32(d[1]) * inv + 127) / 255)
                        d[2] = UInt8((cr * sa + UInt32(d[2]) * inv + 127) / 255)
                        d[3] = UInt8(sa + (UInt32(d[3]) * inv + 127) / 255)
                    }
                    d += 4
                }
            }
        }
    }

    /// Copies premultiplied BGRA rows in at (x, y), replacing what is there.
    public mutating func copyBGRA(_ src: UnsafePointer<UInt8>, w: Int, h: Int, stride: Int, x: Int, y: Int) {
        let x0 = max(0, x), y0 = max(0, y), x1 = min(width, x + w), y1 = min(height, y + h)
        guard x0 < x1, y0 < y1 else { return }
        pixels.withUnsafeMutableBufferPointer { dst in
            for row in y0..<y1 {
                memcpy(dst.baseAddress! + (row * width + x0) * 4, src + (row - y) * stride + (x0 - x) * 4, (x1 - x0) * 4)
            }
        }
    }

    /// An sRGB image at SDR reference white (1.0), optionally scaled by `whiteLevel` (< 1 dims the colours, keeping alpha).
    public func makeImage(whiteLevel: Double = 1) -> CGImage? {
        var data = pixels
        if whiteLevel != 1, whiteLevel < 1 {
            let f = UInt32(whiteLevel * 256)
            for i in Swift.stride(from: 0, to: data.count, by: 4) {
                data[i] = UInt8(UInt32(data[i]) * f >> 8)
                data[i + 1] = UInt8(UInt32(data[i + 1]) * f >> 8)
                data[i + 2] = UInt8(UInt32(data[i + 2]) * f >> 8)
            }
        }
        if whiteLevel > 1 { return makeExtendedImage(gain: whiteLevel, from: data) }
        guard let provider = CGDataProvider(data: Data(data) as CFData) else { return nil }
        return CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// Half-float extended linear sRGB with colours multiplied by `gain` (> 1 is brighter than SDR white on an EDR display,
    /// when the layer asks for high dynamic range). Only for the HDR brightness comparison.
    private func makeExtendedImage(gain: Double, from data: [UInt8]) -> CGImage? {
        var out = [Float16](repeating: 0, count: width * height * 4)
        var lut = [Float](repeating: 0, count: 256)
        for i in 0..<256 {
            let c = Float(i) / 255
            lut[i] = c <= 0.04045 ? c / 12.92 : powf((c + 0.055) / 1.055, 2.4)
        }
        for p in 0..<(width * height) {
            let a = Float(data[p * 4 + 3]) / 255
            guard a > 0 else { continue }
            // Un-premultiply, linearise, scale, premultiply again.
            for (o, s) in [(0, 2), (1, 1), (2, 0)] {
                let straight = min(255, Int((Float(data[p * 4 + s]) / a).rounded()))
                out[p * 4 + o] = Float16(lut[straight] * Float(gain) * a)
            }
            out[p * 4 + 3] = Float16(a)
        }
        let bytes = out.withUnsafeBufferPointer { Data(buffer: $0) }
        guard let provider = CGDataProvider(data: bytes as CFData) else { return nil }
        return CGImage(
            width: width, height: height, bitsPerComponent: 16, bitsPerPixel: 64, bytesPerRow: width * 8,
            space: CGColorSpace(name: CGColorSpace.extendedLinearSRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.floatComponents.rawValue
                | CGBitmapInfo.byteOrder16Little.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}

/// A rendered subtitle frame: an image covering `rect` (pixels, top-left origin) of the render frame.
public struct SubtitleFrame: @unchecked Sendable {
    public var image: CGImage?
    public var rect: CGRect
    public init(image: CGImage?, rect: CGRect) {
        self.image = image
        self.rect = rect
    }
    public static let empty = SubtitleFrame(image: nil, rect: .zero)
}
