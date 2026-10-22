import CoreGraphics
import Foundation
import ImageIO
import SubKit

// SubTool: test media, render checks, benchmarks and the sync probe for the D7 spike.

setvbuf(stdout, nil, _IOLBF, 0)
let args = Array(CommandLine.arguments.dropFirst())
func arg(_ i: Int) -> String { i < args.count ? args[i] : { fatalError("missing argument \(i)\n\(usage)") }() }
func int(_ i: Int) -> Int { Int(arg(i))! }
let usage = """
usage:
  subtool ass-sync <out.ass> <w> <h> <fps> <seconds>      frame-number barcode, one ASS event per frame
  subtool sup-sync <out.sup> <w> <h> <fps> <seconds>      the same as PGS
  subtool sup-dialog <out.sup> <w> <h>                    dialog and sign events (PGS)
  subtool ass-busy <out.ass> <seconds>                    a busy typesetting script at 1920x1080 PlayRes
  subtool embed-font <in.ass> <font file> <out.ass>       adds the font as a [Fonts] section
  subtool render-ass <in.ass> <ms> <w> <h> <out.png> [font files...]
  subtool bench-ass <in.ass> <w> <h> <from s> <to s> <fps> [font files...]
  subtool decode <file>                                   bitmap events via libavcodec
  subtool compare-pgs <file.sup>                          libavcodec vs the Swift parser
  subtool probe-sync <png>...                             reads both barcodes from screenshots
"""

func writePNG(_ image: CGImage, _ path: String) {
    let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, "public.png" as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, image, nil)
    CGImageDestinationFinalize(dest)
}

func fonts(from index: Int) -> [(name: String, data: Data)] {
    args.dropFirst(index).map { (URL(fileURLWithPath: $0).lastPathComponent, try! Data(contentsOf: URL(fileURLWithPath: $0))) }
}

AssRenderer.log = { print($0) }

switch args.first ?? "" {
case "ass-sync":
    try! Barcode.assScript(width: int(2), height: int(3), fps: int(4), seconds: int(5)).write(toFile: arg(1), atomically: true, encoding: .utf8)
case "sup-sync":
    try! Barcode.pgs(width: int(2), height: int(3), fps: int(4), seconds: int(5)).write(to: URL(fileURLWithPath: arg(1)))
case "sup-dialog":
    try! Generators.dialogPGS(width: int(2), height: int(3)).write(to: URL(fileURLWithPath: arg(1)))
case "ass-busy":
    try! Generators.busyASS(seconds: int(2)).write(toFile: arg(1), atomically: true, encoding: .utf8)
case "embed-font":
    let script = try! String(contentsOfFile: arg(1), encoding: .utf8)
    let font = try! Data(contentsOf: URL(fileURLWithPath: arg(2)))
    try! Generators.embed(font: font, name: URL(fileURLWithPath: arg(2)).lastPathComponent, in: script)
        .write(toFile: arg(3), atomically: true, encoding: .utf8)
case "render-ass":
    let provider = AssRenderer.FontProvider(rawValue: ProcessInfo.processInfo.environment["SUBTOOL_PROVIDER"] ?? "coretext") ?? .coretext
    let r = AssRenderer(script: try! Data(contentsOf: URL(fileURLWithPath: arg(1))), attachments: fonts(from: 6), provider: provider,
                        storageSize: CGSize(width: int(3), height: int(4)))!
    let frame = r.render(milliseconds: Int64(int(2)), width: int(3), height: int(4))
    var canvas = Canvas(width: int(3), height: int(4))
    if let image = frame.image {  // place the frame's image on a full-size transparent canvas
        let ctx = CGContext(data: &canvas.pixels, width: canvas.width, height: canvas.height, bitsPerComponent: 8, bytesPerRow: canvas.width * 4,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
        ctx.draw(image, in: CGRect(x: frame.rect.minX, y: CGFloat(canvas.height) - frame.rect.maxY, width: frame.rect.width, height: frame.rect.height))
    }
    writePNG(canvas.makeImage()!, arg(5))
    print("rect \(frame.rect)")
case "bench-ass":
    let (w, h) = (int(2), int(3))
    let r = AssRenderer(script: try! Data(contentsOf: URL(fileURLWithPath: arg(1))), attachments: fonts(from: 7),
                        storageSize: CGSize(width: w, height: h))!
    let from = Double(arg(4))!, to = Double(arg(5))!, fps = Double(arg(6))!
    _ = r.render(milliseconds: Int64(from * 1000), width: w, height: h)  // warm caches and fonts
    r.resetStats()
    let started = CFAbsoluteTimeGetCurrent()
    var t = from
    var maxPixels = 0.0
    while t < to {
        let f = r.render(milliseconds: Int64(t * 1000), width: w, height: h)
        maxPixels = max(maxPixels, f.rect.width * f.rect.height)
        t += 1 / fps
    }
    let wall = CFAbsoluteTimeGetCurrent() - started
    let s = r.stats
    print(String(format: "%dx%d frames %d changed %d: render %.2f ms/frame, blend+image %.2f ms/changed frame, max %.2f ms, "
                 + "%.0f bitmaps/changed frame, largest rect %.1f Mpx, wall %.2f s for %.1f s of video (%.0f%% of one core)",
                 w, h, s.renders, s.changed, s.renderSeconds / Double(s.renders) * 1000,
                 s.blendSeconds / Double(max(s.changed, 1)) * 1000, s.maxFrameSeconds * 1000,
                 Double(s.images) / Double(max(s.changed, 1)), maxPixels / 1e6, wall, to - from, wall / (to - from) * 100))
case "decode":
    let track = try! LibavSubtitleDecoder.decode(path: arg(1))
    print("\(track.decoder): canvas \(track.canvasWidth)x\(track.canvasHeight), \(track.events.count) events")
    for e in track.events.prefix(40) {
        print(String(format: "  %8.3f → %8.3f  ", e.start, e.end) + e.rects.map { "[\($0.x),\($0.y) \($0.w)x\($0.h)]" }.joined(separator: " "))
    }
case "compare-pgs":
    var t0 = CFAbsoluteTimeGetCurrent()
    let a = try! LibavSubtitleDecoder.decode(path: arg(1))
    let libavTime = CFAbsoluteTimeGetCurrent() - t0
    t0 = CFAbsoluteTimeGetCurrent()
    let b = try! PGS.read(Data(contentsOf: URL(fileURLWithPath: arg(1))))
    let swiftTime = CFAbsoluteTimeGetCurrent() - t0
    print(String(format: "libavcodec: %d events in %.1f ms; swift: %d events in %.1f ms", a.events.count, libavTime * 1000,
                 b.events.count, swiftTime * 1000))
    print("canvas libav \(a.canvasWidth)x\(a.canvasHeight), swift \(b.canvasWidth)x\(b.canvasHeight)")
    // libavcodec drops empty (clearing) display sets as events of their own; compare the non-empty ones.
    let ea = a.events.filter { !$0.rects.isEmpty }, eb = b.events.filter { !$0.rects.isEmpty }
    var timing = 0.0, maxDiff = 0, geometry = 0, bytes = 0
    for (x, y) in zip(ea, eb) {
        timing = max(timing, abs(x.start - y.start), abs(x.end - y.end))
        for (r1, r2) in zip(x.rects, y.rects) {
            if (r1.x, r1.y, r1.w, r1.h) != (r2.x, r2.y, r2.w, r2.h) { geometry += 1; continue }
            for i in r1.bgra.indices { maxDiff = max(maxDiff, abs(Int(r1.bgra[i]) - Int(r2.bgra[i]))) }
            bytes += r1.bgra.count
        }
    }
    print("non-empty events: libav \(ea.count), swift \(eb.count); max start/end difference \(String(format: "%.4f", timing)) s; "
          + "geometry mismatches \(geometry); max channel difference \(maxDiff) over \(bytes) bytes")
case "probe-sync":
    var diffs: [Int] = []
    for path in args.dropFirst() {
        guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let image = CGImageSourceCreateImageAtIndex(src, 0, nil), let v = Barcode.read(image)
        else { print("\(path): unreadable"); continue }
        diffs.append(v.subtitle - v.video)
        print("\(URL(fileURLWithPath: path).lastPathComponent): video \(v.video) subtitle \(v.subtitle) diff \(v.subtitle - v.video)")
    }
    let histogram = Dictionary(grouping: diffs, by: { $0 }).mapValues(\.count).sorted { $0.key < $1.key }
    print("diff histogram (subtitle - video, frames): " + histogram.map { "\($0.key): \($0.value)" }.joined(separator: ", "))
default:
    print(usage)
}
