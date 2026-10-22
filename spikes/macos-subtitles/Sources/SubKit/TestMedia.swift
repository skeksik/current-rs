import CoreGraphics
import CoreText
import Foundation

/// Frame counters for the sync measurement. The video carries frame N as 16 blocks in its top row (make-media.sh, FFmpeg geq);
/// the subtitle under test draws the frame number it believes is current as 16 blocks in a row below. A screenshot of the
/// video rect then shows both, and their difference is the subtitle's offset in frames.
public enum Barcode {
    public static let bits = 16
    /// Rows start this many block heights down, clear of the window's title bar (which overlays the video).
    public static let topOffset = 4
    /// Block `bit` of `row` (0: video, 1: subtitle; one block height apart) in a frame of `width`×`height`, top-left origin.
    public static func block(bit: Int, row: Int, width: Double, height: Double) -> CGRect {
        let bw = width / Double(bits), bh = height / 27
        return CGRect(x: Double(bit) * bw, y: (Double(row) * 2 + Double(topOffset)) * bh, width: bw, height: bh)
    }

    static func cs(_ t: Double) -> String {  // ASS time, centiseconds
        let c = Int((t * 100).rounded())
        return String(format: "%d:%02d:%02d.%02d", c / 360_000, c / 6000 % 60, c / 100 % 60, c % 100)
    }

    /// One ASS event per frame drawing the frame number (vector drawing, \p1), on a dark row that lasts the whole file.
    public static func assScript(width: Int, height: Int, fps: Int, seconds: Int) -> String {
        var s = """
        [Script Info]
        ScriptType: v4.00+
        PlayResX: \(width)
        PlayResY: \(height)
        ScaledBorderAndShadow: yes

        [V4+ Styles]
        Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
        Style: Bar,Helvetica,20,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1

        [Events]
        Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text

        """
        let row = block(bit: 0, row: 1, width: Double(width), height: Double(height))
        s += "Dialogue: 0,\(cs(0)),\(cs(Double(seconds))),Bar,,0,0,0,,{\\pos(0,0)\\p1\\1c&H404040&}"
            + "m 0 \(Int(row.minY)) l \(width) \(Int(row.minY)) \(width) \(Int(row.maxY)) 0 \(Int(row.maxY)){\\p0}\n"
        for f in 0..<(fps * seconds) {
            var path = ""
            for bit in 0..<bits where (f >> bit) & 1 == 1 {
                let r = block(bit: bit, row: 1, width: Double(width), height: Double(height)).integral
                path += "m \(Int(r.minX)) \(Int(r.minY)) l \(Int(r.maxX)) \(Int(r.minY)) \(Int(r.maxX)) \(Int(r.maxY)) \(Int(r.minX)) \(Int(r.maxY)) "
            }
            guard !path.isEmpty else { continue }
            let t0 = Double(f) / Double(fps), t1 = Double(f + 1) / Double(fps)
            s += "Dialogue: 1,\(cs(t0)),\(cs(t1)),Bar,,0,0,0,,{\\pos(0,0)\\p1}\(path){\\p0}\n"
        }
        return s
    }

    /// One PGS display set per frame with the frame number as a 16-block row.
    public static func pgs(width: Int, height: Int, fps: Int, seconds: Int) -> Data {
        var writer = PGS.Writer(width: width, height: height)
        let b0 = block(bit: 0, row: 1, width: Double(width), height: Double(height)).integral
        let row = CGRect(x: 0, y: b0.minY, width: Double(width), height: b0.height)
        let palette: [(UInt8, UInt8, UInt8, UInt8)] = [(0, 0, 0, 0), (64, 64, 64, 255), (255, 255, 255, 255)]
        for f in 0..<(fps * seconds) {
            var px = [UInt8](repeating: 1, count: Int(row.width) * Int(row.height))
            for bit in 0..<bits where (f >> bit) & 1 == 1 {
                let r = block(bit: bit, row: 1, width: Double(width), height: Double(height)).integral
                for y in 0..<Int(row.height) { for x in Int(r.minX)..<min(Int(r.maxX), Int(row.width)) { px[y * Int(row.width) + x] = 2 } }
            }
            writer.show(pts: Double(f) / Double(fps),
                        objects: [PGS.Object(x: 0, y: Int(row.minY), w: Int(row.width), h: Int(row.height), indices: px)],
                        palette: palette)
        }
        writer.clear(pts: Double(seconds))
        return writer.data
    }

    /// Reads both rows from a screenshot of the video rect. Returns nil when a row is unreadable.
    public static func read(_ image: CGImage) -> (video: Int, subtitle: Int)? {
        let w = image.width, h = image.height
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        guard let data = ctx.data?.assumingMemoryBound(to: UInt8.self) else { return nil }
        func value(row: Int) -> Int {
            var v = 0
            for bit in 0..<bits {
                let r = block(bit: bit, row: row, width: Double(w), height: Double(h))
                let x = Int(r.midX), y = Int(r.midY)  // CGContext memory is top row first
                let p = data + (y * w + x) * 4
                let luma = 0.2126 * Double(p[0]) + 0.7152 * Double(p[1]) + 0.0722 * Double(p[2])
                if luma > 150 { v |= 1 << bit }
            }
            return v
        }
        return (value(row: 0), value(row: 1))
    }
}

/// Text drawn with CoreText into palette indices for PGS: 8 fill levels × 8 outline levels, index 0 transparent.
public enum TextBitmap {
    public struct Style {
        public var fontName = "Helvetica-Bold"
        public var size: CGFloat = 56
        public var fill: (UInt8, UInt8, UInt8) = (255, 255, 255)
        public var fillAlpha: UInt8 = 255
        public var outline: (UInt8, UInt8, UInt8) = (0, 0, 0)
        public var outlineWidth: CGFloat = 4
        public init() {}
    }

    public static func palette(_ style: Style) -> [(UInt8, UInt8, UInt8, UInt8)] {
        var p: [(UInt8, UInt8, UInt8, UInt8)] = [(0, 0, 0, 0)]
        for qf in 0..<8 {
            for qo in 0..<8 {
                let fa = Double(qf) / 7 * Double(style.fillAlpha) / 255, oa = Double(qo) / 7
                let a = fa + oa * (1 - fa)
                func mix(_ f: UInt8, _ o: UInt8) -> UInt8 {
                    a == 0 ? 0 : UInt8(((Double(f) * fa + Double(o) * oa * (1 - fa)) / a).rounded())
                }
                p.append((mix(style.fill.0, style.outline.0), mix(style.fill.1, style.outline.1), mix(style.fill.2, style.outline.2),
                          UInt8((a * 255).rounded())))
            }
        }
        return p
    }

    /// Renders `text` and returns its tight object (size and indices).
    public static func render(_ text: String, style: Style) -> (w: Int, h: Int, indices: [UInt8]) {
        let font = CTFontCreateWithName(style.fontName as CFString, style.size, nil)
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [.init(kCTFontAttributeName as String): font,
            .init(kCTForegroundColorFromContextAttributeName as String): true]))
        let bounds = CTLineGetImageBounds(line, nil).insetBy(dx: -style.outlineWidth - 2, dy: -style.outlineWidth - 2).integral
        let w = Int(bounds.width), h = Int(bounds.height)
        func mask(stroke: Bool) -> [UInt8] {
            let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w, space: CGColorSpaceCreateDeviceGray(),
                                bitmapInfo: CGImageAlphaInfo.none.rawValue)!
            ctx.setFillColor(gray: 1, alpha: 1)
            ctx.setStrokeColor(gray: 1, alpha: 1)
            ctx.setLineWidth(style.outlineWidth * 2)
            ctx.setLineJoin(.round)
            ctx.textPosition = CGPoint(x: -bounds.minX, y: -bounds.minY)
            ctx.setTextDrawingMode(stroke ? .fillStroke : .fill)
            CTLineDraw(line, ctx)
            let p = ctx.data!.assumingMemoryBound(to: UInt8.self)
            return (0..<(w * h)).map { p[$0] }  // first row in memory is the top row
        }
        let fill = mask(stroke: false), outline = mask(stroke: true)
        let indices = (0..<(w * h)).map { i -> UInt8 in
            let qf = Int((Double(fill[i]) / 255 * 7).rounded()), qo = Int((Double(outline[i]) / 255 * 7).rounded())
            return qf == 0 && qo == 0 ? 0 : UInt8(1 + qf * 8 + qo)
        }
        return (w, h, indices)
    }
}
