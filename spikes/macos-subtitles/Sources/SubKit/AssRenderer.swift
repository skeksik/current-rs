import CLibass
import CoreGraphics
import Foundation

/// libass over a whole .ass script, as a server delivers it. Not thread-safe: one caller at a time.
public final class AssRenderer {
    public enum FontProvider: String, Sendable { case coretext, none }

    public struct Stats {
        public var renders = 0
        public var changed = 0
        public var renderSeconds = 0.0  // ass_render_frame
        public var blendSeconds = 0.0  // compositing + CGImage
        public var maxFrameSeconds = 0.0
        public var images = 0
    }

    private let library: OpaquePointer
    private let renderer: OpaquePointer
    private let track: UnsafeMutablePointer<ASS_Track>
    private var frameSize = (w: 0, h: 0)
    private var linePosition = -1.0
    private var last = SubtitleFrame.empty
    public private(set) var stats = Stats()
    public var whiteLevel = 1.0 { didSet { forceRedraw = true } }
    private var forceRedraw = false
    nonisolated(unsafe) public static var log: (@Sendable (String) -> Void)?

    /// - Parameters:
    ///   - script: the .ass file. A `[Fonts]` section is honoured (ass_set_extract_fonts).
    ///   - attachments: fonts as an MKV carries them (Jellyfin's attachments endpoint), added by name.
    ///   - storageSize: the video's size in pixels; libass scales blur and borders with it.
    public init?(script: Data, attachments: [(name: String, data: Data)] = [], provider: FontProvider = .coretext,
                 storageSize: CGSize? = nil) {
        guard let library = ass_library_init() else { return nil }
        self.library = library
        ass_set_message_cb(library, { level, format, args, _ in
            guard level <= 4, let format, let args else { return }  // MSGL_INFO and more severe, which includes fontselect
            var buffer = [CChar](repeating: 0, count: 1024)
            vsnprintf(&buffer, buffer.count, format, args)
            AssRenderer.log?("libass[\(level)] " + String(cString: buffer))
        }, nil)
        ass_set_extract_fonts(library, 1)
        for font in attachments {
            font.data.withUnsafeBytes { raw in
                ass_add_font(library, font.name, raw.baseAddress!.assumingMemoryBound(to: CChar.self), Int32(raw.count))
            }
        }
        guard let renderer = ass_renderer_init(library) else { ass_library_done(library); return nil }
        self.renderer = renderer
        let dfp = provider == .coretext ? ASS_FONTPROVIDER_CORETEXT : ASS_FONTPROVIDER_NONE
        // With no provider, libass can only use the script's and attached fonts; the default font path is then required.
        ass_set_fonts(renderer, provider == .none ? "/System/Library/Fonts/Helvetica.ttc" : nil, "sans-serif",
                      Int32(dfp.rawValue), nil, 0)
        var bytes = [CChar](repeating: 0, count: script.count + 1)
        script.withUnsafeBytes { raw in _ = memcpy(&bytes, raw.baseAddress!, script.count) }
        guard let track = ass_read_memory(library, &bytes, script.count, nil) else {
            ass_renderer_done(renderer)
            ass_library_done(library)
            return nil
        }
        self.track = track
        if let storageSize { ass_set_storage_size(renderer, Int32(storageSize.width), Int32(storageSize.height)) }
        AssRenderer.log?("libass \(String(ass_library_version(), radix: 16)): \(track.pointee.n_events) events, "
            + "\(track.pointee.n_styles) styles, provider \(provider.rawValue), \(attachments.count) attachments")
    }

    deinit {
        ass_free_track(track)
        ass_renderer_done(renderer)
        ass_library_done(library)
    }

    /// Renders the subtitles at `milliseconds` for a frame of `width`×`height` pixels (the video rect on screen).
    /// `bottomInset` (pixels) moves unpositioned lines up, above our control bar; positioned signs stay put.
    public func render(milliseconds: Int64, width: Int, height: Int, bottomInset: Int = 0) -> SubtitleFrame {
        let started = CFAbsoluteTimeGetCurrent()
        if frameSize != (width, height) {
            frameSize = (width, height)
            ass_set_frame_size(renderer, Int32(width), Int32(height))
            forceRedraw = true
        }
        let position = height > 0 ? min(100, Double(bottomInset) / Double(height) * 100) : 0
        if position != linePosition {
            linePosition = position
            ass_set_line_position(renderer, position)
            forceRedraw = true
        }
        var change: Int32 = 0
        let first = ass_render_frame(renderer, track, milliseconds, &change)
        let rendered = CFAbsoluteTimeGetCurrent()
        stats.renders += 1
        stats.renderSeconds += rendered - started
        guard change != 0 || forceRedraw else { return last }
        forceRedraw = false
        stats.changed += 1

        // Union of all bitmaps: the layer covers only that rectangle, so blending and upload stay small.
        var bounds = CGRect.null
        var node = first
        while let image = node {
            let i = image.pointee
            if i.w > 0, i.h > 0 { bounds = bounds.union(CGRect(x: Int(i.dst_x), y: Int(i.dst_y), width: Int(i.w), height: Int(i.h))) }
            node = i.next
        }
        bounds = bounds.intersection(CGRect(x: 0, y: 0, width: width, height: height))
        guard !bounds.isNull, !bounds.isEmpty else {
            last = .empty
            return last
        }
        var canvas = Canvas(width: Int(bounds.width), height: Int(bounds.height))
        var count = 0
        node = first
        while let image = node {
            let i = image.pointee
            if i.w > 0, i.h > 0, let bitmap = i.bitmap {
                // ASS colour is RGBA with inverted alpha (0 = opaque).
                canvas.blendMask(
                    bitmap, w: Int(i.w), h: Int(i.h), stride: Int(i.stride),
                    x: Int(i.dst_x) - Int(bounds.minX), y: Int(i.dst_y) - Int(bounds.minY),
                    r: UInt8(i.color >> 24), g: UInt8((i.color >> 16) & 0xff), b: UInt8((i.color >> 8) & 0xff),
                    alpha: 255 - UInt8(i.color & 0xff))
                count += 1
            }
            node = i.next
        }
        last = SubtitleFrame(image: canvas.makeImage(whiteLevel: whiteLevel), rect: bounds)
        let done = CFAbsoluteTimeGetCurrent()
        stats.blendSeconds += done - rendered
        stats.maxFrameSeconds = max(stats.maxFrameSeconds, done - started)
        stats.images += count
        return last
    }

    public func resetStats() { stats = Stats() }
}
