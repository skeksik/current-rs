import Foundation

/// A small PGS (HDMV presentation graphics, .sup) reader and writer, to compare with libavcodec's decoder and to make test
/// files (FFmpeg has no PGS encoder). Covers what Blu-ray authoring tools emit for subtitles: PCS, WDS, PDS, ODS, END.
public enum PGS {
    struct Segment {
        var pts: Double
        var type: UInt8
        var payload: ArraySlice<UInt8>
    }

    static func segments(_ bytes: [UInt8]) throws -> [Segment] {
        var out: [Segment] = []
        var p = 0
        while p + 13 <= bytes.count {
            guard bytes[p] == 0x50, bytes[p + 1] == 0x47 else { throw ParseError.badMagic(p) }  // "PG"
            let pts = UInt32(bytes[p + 2]) << 24 | UInt32(bytes[p + 3]) << 16 | UInt32(bytes[p + 4]) << 8 | UInt32(bytes[p + 5])
            let type = bytes[p + 10]
            let size = Int(bytes[p + 11]) << 8 | Int(bytes[p + 12])
            guard p + 13 + size <= bytes.count else { throw ParseError.truncated(p) }
            out.append(Segment(pts: Double(pts) / 90_000, type: type, payload: bytes[(p + 13)..<(p + 13 + size)]))
            p += 13 + size
        }
        return out
    }

    public enum ParseError: Error { case badMagic(Int), truncated(Int) }

    // MARK: Reading

    /// Parses a whole .sup into events. Each display set with composition objects shows them until the next display set.
    public static func read(_ data: Data) throws -> BitmapTrack {
        let bytes = [UInt8](data)
        var palettes: [UInt8: [UInt32]] = [:]  // id -> 256 premultiplied BGRA entries packed as UInt32 (B | G<<8 | R<<16 | A<<24)
        var objects: [UInt16: (w: Int, h: Int, rle: [UInt8])] = [:]
        var width = 0, height = 0
        var events: [BitmapEvent] = []
        var pending: (pts: Double, paletteId: UInt8, placements: [(id: UInt16, x: Int, y: Int)])?

        for seg in try segments(bytes) {
            let s = Array(seg.payload)
            func u16(_ i: Int) -> Int { Int(s[i]) << 8 | Int(s[i + 1]) }
            switch seg.type {
            case 0x16:  // PCS
                width = u16(0)
                height = u16(2)
                let state = s[7]
                if state & 0x80 != 0 { objects.removeAll() }  // epoch start
                var placements: [(UInt16, Int, Int)] = []
                var o = 11
                for _ in 0..<Int(s[10]) {
                    let id = UInt16(u16(o))
                    let cropped = s[o + 3] & 0x40 != 0
                    placements.append((id, u16(o + 4), u16(o + 6)))
                    o += cropped ? 16 : 8
                }
                pending = (seg.pts, s[9], placements)
            case 0x14:  // PDS
                var table = palettes[s[0]] ?? [UInt32](repeating: 0, count: 256)
                var o = 2
                while o + 5 <= s.count {
                    table[Int(s[o])] = ycrcbaToBGRA(y: s[o + 1], cr: s[o + 2], cb: s[o + 3], a: s[o + 4], hd: height > 576)
                    o += 5
                }
                palettes[s[0]] = table
            case 0x15:  // ODS
                let id = UInt16(u16(0))
                let sequence = s[3]
                if sequence & 0x80 != 0 {
                    objects[id] = (u16(7), u16(9), Array(s[11...]))
                } else if var object = objects[id] {
                    object.rle += s[4...]
                    objects[id] = object
                }
            case 0x80:  // END: the display set is complete
                guard let set = pending else { continue }
                pending = nil
                let table = palettes[set.paletteId] ?? [UInt32](repeating: 0, count: 256)
                var rects: [BitmapRect] = []
                for placement in set.placements {
                    guard let object = objects[placement.id] else { continue }
                    let indices = decodeRLE(object.rle, w: object.w, h: object.h)
                    var bgra = [UInt8](repeating: 0, count: object.w * object.h * 4)
                    for (i, index) in indices.enumerated() {
                        let c = table[Int(index)]
                        bgra[i * 4] = UInt8(c & 0xff)
                        bgra[i * 4 + 1] = UInt8((c >> 8) & 0xff)
                        bgra[i * 4 + 2] = UInt8((c >> 16) & 0xff)
                        bgra[i * 4 + 3] = UInt8(c >> 24)
                    }
                    rects.append(BitmapRect(x: placement.x, y: placement.y, w: object.w, h: object.h, bgra: bgra))
                }
                if let lastIndex = events.indices.last, events[lastIndex].end == .infinity { events[lastIndex].end = set.pts }
                events.append(BitmapEvent(start: set.pts, end: .infinity, rects: rects))
            default:  // WDS (0x17) and anything else: windows don't change what is drawn here
                break
            }
        }
        if let lastIndex = events.indices.last, events[lastIndex].end == .infinity { events[lastIndex].end = events[lastIndex].start + 10 }
        return BitmapTrack(canvasWidth: width, canvasHeight: height, events: events, decoder: "swift-pgs")
    }

    static func decodeRLE(_ rle: [UInt8], w: Int, h: Int) -> [UInt8] {
        var out = [UInt8](repeating: 0, count: w * h)
        var p = 0, x = 0, y = 0
        func put(_ colour: UInt8, _ n: Int) {
            guard y < h else { return }
            let count = min(n, w - x)
            if count > 0, colour != 0 { for i in 0..<count { out[y * w + x + i] = colour } }
            x += max(count, 0)
        }
        while p < rle.count, y < h {
            let b = rle[p]; p += 1
            if b != 0 { put(b, 1); continue }
            guard p < rle.count else { break }
            let b2 = rle[p]; p += 1
            if b2 == 0 { x = 0; y += 1; continue }  // end of line
            var length = Int(b2 & 0x3f)
            if b2 & 0x40 != 0, p < rle.count { length = length << 8 | Int(rle[p]); p += 1 }
            var colour: UInt8 = 0
            if b2 & 0x80 != 0, p < rle.count { colour = rle[p]; p += 1 }
            put(colour, length)
        }
        return out
    }

    /// Limited-range YCbCr to premultiplied BGRA; BT.709 for HD canvases, BT.601 otherwise (as libavcodec's decoder does).
    static func ycrcbaToBGRA(y: UInt8, cr: UInt8, cb: UInt8, a: UInt8, hd: Bool) -> UInt32 {
        let yy = (Double(y) - 16) * 255 / 219, pb = (Double(cb) - 128) * 255 / 224, pr = (Double(cr) - 128) * 255 / 224
        let (kr, kb) = hd ? (0.2126, 0.0722) : (0.299, 0.114)
        let kg = 1 - kr - kb
        let r = yy + 2 * (1 - kr) * pr
        let b = yy + 2 * (1 - kb) * pb
        let g = (yy - kr * r - kb * b) / kg
        func c(_ v: Double) -> UInt32 { UInt32(max(0, min(255, (v * Double(a) / 255).rounded()))) }
        return c(b) | c(g) << 8 | c(r) << 16 | UInt32(a) << 24
    }

    static func rgbaToYCrCbA(r: UInt8, g: UInt8, b: UInt8, a: UInt8, hd: Bool) -> [UInt8] {
        let (kr, kb) = hd ? (0.2126, 0.0722) : (0.299, 0.114)
        let kg = 1 - kr - kb
        let (rf, gf, bf) = (Double(r), Double(g), Double(b))
        let y = kr * rf + kg * gf + kb * bf
        let pb = (bf - y) / (2 * (1 - kb)), pr = (rf - y) / (2 * (1 - kr))
        func clamp(_ v: Double) -> UInt8 { UInt8(max(0, min(255, v.rounded()))) }
        return [clamp(16 + y * 219 / 255), clamp(128 + pr * 224 / 255), clamp(128 + pb * 224 / 255), a]
    }

    // MARK: Writing

    public struct Object {
        public var x: Int, y: Int, w: Int, h: Int
        public var indices: [UInt8]  // palette indices, 0 = transparent
        public init(x: Int, y: Int, w: Int, h: Int, indices: [UInt8]) {
            self.x = x; self.y = y; self.w = w; self.h = h; self.indices = indices
        }
    }

    /// Writes display sets. Every set is an epoch start, so each one stands alone (simple and valid, if not the smallest).
    public struct Writer {
        public private(set) var data = Data()
        let width: Int, height: Int
        var composition = 0
        var lastWindows: [(Int, Int, Int, Int)] = []

        public init(width: Int, height: Int) { self.width = width; self.height = height }

        mutating func segment(pts: Double, type: UInt8, _ payload: [UInt8]) {
            let t = UInt32((pts * 90_000).rounded())
            data.append(contentsOf: [0x50, 0x47, UInt8(t >> 24), UInt8(t >> 16 & 0xff), UInt8(t >> 8 & 0xff), UInt8(t & 0xff), 0, 0, 0, 0,
                                     type, UInt8(payload.count >> 8), UInt8(payload.count & 0xff)])
            data.append(contentsOf: payload)
        }

        static func be16(_ v: Int) -> [UInt8] { [UInt8(v >> 8 & 0xff), UInt8(v & 0xff)] }

        /// Shows up to two objects with a palette of up to 256 straight-alpha RGBA colours.
        public mutating func show(pts: Double, objects: [Object], palette: [(UInt8, UInt8, UInt8, UInt8)]) {
            precondition(objects.count <= 2, "PGS allows two objects per composition")
            composition += 1
            var pcs = Self.be16(width) + Self.be16(height) + [0x10] + Self.be16(composition) + [0x80, 0, 0, UInt8(objects.count)]
            for (i, o) in objects.enumerated() { pcs += Self.be16(i) + [UInt8(i), 0] + Self.be16(o.x) + Self.be16(o.y) }
            segment(pts: pts, type: 0x16, pcs)
            var wds: [UInt8] = [UInt8(objects.count)]
            for (i, o) in objects.enumerated() { wds += [UInt8(i)] + Self.be16(o.x) + Self.be16(o.y) + Self.be16(o.w) + Self.be16(o.h) }
            lastWindows = objects.map { ($0.x, $0.y, $0.w, $0.h) }
            segment(pts: pts, type: 0x17, wds)
            var pds: [UInt8] = [0, 0]
            for (i, c) in palette.enumerated() where i > 0 {
                pds += [UInt8(i)] + PGS.rgbaToYCrCbA(r: c.0, g: c.1, b: c.2, a: c.3, hd: height > 576)
            }
            segment(pts: pts, type: 0x14, pds)
            for (i, o) in objects.enumerated() {
                let rle = Self.encodeRLE(o.indices, w: o.w, h: o.h)
                let head = Self.be16(i) + [0]
                let first = rle.count + 4
                var chunks: [ArraySlice<UInt8>] = []
                var p = 0
                let firstRoom = 65_535 - 11
                chunks.append(rle[0..<min(rle.count, firstRoom)])
                p = min(rle.count, firstRoom)
                while p < rle.count { chunks.append(rle[p..<min(rle.count, p + 65_535 - 4)]); p += 65_535 - 4 }
                for (n, chunk) in chunks.enumerated() {
                    var flags: UInt8 = 0
                    if n == 0 { flags |= 0x80 }
                    if n == chunks.count - 1 { flags |= 0x40 }
                    var payload = head + [flags]
                    if n == 0 { payload += [UInt8(first >> 16 & 0xff), UInt8(first >> 8 & 0xff), UInt8(first & 0xff)] + Self.be16(o.w) + Self.be16(o.h) }
                    payload += chunk
                    segment(pts: pts, type: 0x15, payload)
                }
            }
            segment(pts: pts, type: 0x80, [])
        }

        /// Clears the screen: a composition with no objects over the last windows.
        public mutating func clear(pts: Double) {
            composition += 1
            segment(pts: pts, type: 0x16, Self.be16(width) + Self.be16(height) + [0x10] + Self.be16(composition) + [0, 0, 0, 0])
            var wds: [UInt8] = [UInt8(lastWindows.count)]
            for (i, w) in lastWindows.enumerated() { wds += [UInt8(i)] + Self.be16(w.0) + Self.be16(w.1) + Self.be16(w.2) + Self.be16(w.3) }
            segment(pts: pts, type: 0x17, wds)
            segment(pts: pts, type: 0x80, [])
        }

        static func encodeRLE(_ px: [UInt8], w: Int, h: Int) -> [UInt8] {
            var out: [UInt8] = []
            for y in 0..<h {
                var x = 0
                while x < w {
                    let c = px[y * w + x]
                    var n = 1
                    while x + n < w, px[y * w + x + n] == c, n < 16_383 { n += 1 }
                    if c != 0, n <= 2 {
                        out += [UInt8](repeating: c, count: n)
                    } else if c == 0 {
                        out += n < 64 ? [0, UInt8(n)] : [0, 0x40 | UInt8(n >> 8), UInt8(n & 0xff)]
                    } else {
                        out += n < 64 ? [0, 0x80 | UInt8(n), c] : [0, 0xC0 | UInt8(n >> 8), UInt8(n & 0xff), c]
                    }
                    x += n
                }
                out += [0, 0]
            }
            return out
        }
    }
}
