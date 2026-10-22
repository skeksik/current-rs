import CFFmpeg
import CoreGraphics
import Foundation

/// One bitmap of a display set, in canvas pixels (top-left origin), premultiplied BGRA.
public struct BitmapRect {
    public var x: Int, y: Int, w: Int, h: Int
    public var bgra: [UInt8]
}

/// What is shown from `start` until `end` (seconds on the subtitle stream's timeline). An empty `rects` clears the screen.
public struct BitmapEvent {
    public var start: Double
    public var end: Double
    public var rects: [BitmapRect]
}

/// A whole bitmap subtitle stream (PGS .sup, or VobSub in .mks as Jellyfin extracts it), decoded up front.
public struct BitmapTrack {
    public var canvasWidth: Int
    public var canvasHeight: Int
    public var events: [BitmapEvent]
    public var decoder: String

    /// Index of the event shown at `time`, if any. Events are sorted by start.
    public func event(at time: Double) -> Int? {
        var lo = 0, hi = events.count - 1, found: Int?
        while lo <= hi {
            let mid = (lo + hi) / 2
            if events[mid].start <= time { found = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        guard let i = found, time < events[i].end, !events[i].rects.isEmpty else { return nil }
        return i
    }

    /// The event's bitmaps composed into one image covering their union, in canvas pixels.
    public func compose(_ index: Int) -> SubtitleFrame {
        let rects = events[index].rects
        var bounds = CGRect.null
        for r in rects { bounds = bounds.union(CGRect(x: r.x, y: r.y, width: r.w, height: r.h)) }
        guard !bounds.isNull else { return .empty }
        var canvas = Canvas(width: Int(bounds.width), height: Int(bounds.height))
        for r in rects {
            r.bgra.withUnsafeBufferPointer {
                canvas.copyBGRA($0.baseAddress!, w: r.w, h: r.h, stride: r.w * 4, x: r.x - Int(bounds.minX), y: r.y - Int(bounds.minY))
            }
        }
        return SubtitleFrame(image: canvas.makeImage(), rect: bounds)
    }
}

public enum LibavSubtitleDecoder {
    public enum Failure: Error { case open(Int32), noStream, noDecoder, decoder(Int32) }

    /// Demuxes and decodes every subtitle packet of the first subtitle stream with libavformat and libavcodec.
    public static func decode(path: String) throws -> BitmapTrack {
        av_log_set_level(AV_LOG_ERROR)
        var fmt: UnsafeMutablePointer<AVFormatContext>?
        var err = avformat_open_input(&fmt, path, nil, nil)
        guard err >= 0, let fmt else { throw Failure.open(err) }
        defer { var f: UnsafeMutablePointer<AVFormatContext>? = fmt; avformat_close_input(&f) }
        avformat_find_stream_info(fmt, nil)
        let streamIndex = av_find_best_stream(fmt, AVMEDIA_TYPE_SUBTITLE, -1, -1, nil, 0)
        guard streamIndex >= 0, let stream = fmt.pointee.streams[Int(streamIndex)] else { throw Failure.noStream }
        guard let codec = avcodec_find_decoder(stream.pointee.codecpar.pointee.codec_id) else { throw Failure.noDecoder }
        var ctx = avcodec_alloc_context3(codec)
        defer { avcodec_free_context(&ctx) }
        avcodec_parameters_to_context(ctx, stream.pointee.codecpar)
        ctx!.pointee.pkt_timebase = stream.pointee.time_base
        err = avcodec_open2(ctx, codec, nil)
        guard err >= 0 else { throw Failure.decoder(err) }

        let tb = stream.pointee.time_base
        var events: [BitmapEvent] = []
        var packet = av_packet_alloc()
        defer { av_packet_free(&packet) }
        while av_read_frame(fmt, packet) >= 0 {
            defer { av_packet_unref(packet) }
            guard packet!.pointee.stream_index == streamIndex else { continue }
            var sub = AVSubtitle()
            var got: Int32 = 0
            guard avcodec_decode_subtitle2(ctx, &sub, &got, packet) >= 0, got != 0 else { continue }
            defer { avsubtitle_free(&sub) }
            let base = sub.pts != Int64.min
                ? Double(sub.pts) / Double(AV_TIME_BASE)
                : Double(packet!.pointee.pts) * av_q2d(tb)
            let start = base + Double(sub.start_display_time) / 1000
            var end = Double.infinity
            if sub.end_display_time != UInt32.max, sub.end_display_time > sub.start_display_time {
                end = base + Double(sub.end_display_time) / 1000
            }
            if packet!.pointee.duration > 0, end == .infinity {
                end = Double(packet!.pointee.pts + packet!.pointee.duration) * av_q2d(tb)
            }
            var rects: [BitmapRect] = []
            for i in 0..<Int(sub.num_rects) {
                guard let rect = sub.rects[i]?.pointee, rect.type == SUBTITLE_BITMAP, rect.w > 0, rect.h > 0,
                    let indices = rect.data.0, let palette = rect.data.1
                else { continue }
                // Palette: uint32 ARGB in native order (little endian: B, G, R, A bytes), straight alpha.
                let colours = UnsafeRawPointer(palette).assumingMemoryBound(to: UInt32.self)
                var bgra = [UInt8](repeating: 0, count: Int(rect.w * rect.h) * 4)
                for row in 0..<Int(rect.h) {
                    for col in 0..<Int(rect.w) {
                        let c = colours[Int(indices[row * Int(rect.linesize.0) + col])]
                        let a = c >> 24
                        let o = (row * Int(rect.w) + col) * 4
                        bgra[o] = UInt8((c & 0xff) * a / 255)
                        bgra[o + 1] = UInt8(((c >> 8) & 0xff) * a / 255)
                        bgra[o + 2] = UInt8(((c >> 16) & 0xff) * a / 255)
                        bgra[o + 3] = UInt8(a)
                    }
                }
                rects.append(BitmapRect(x: Int(rect.x), y: Int(rect.y), w: Int(rect.w), h: Int(rect.h), bgra: bgra))
            }
            events.append(BitmapEvent(start: start, end: end, rects: rects))
        }
        events.sort { $0.start < $1.start }
        // An event without its own end lasts until the next one (PGS clears with an empty display set).
        for i in events.indices where events[i].end == .infinity {
            events[i].end = i + 1 < events.count ? events[i + 1].start : events[i].start + 10
        }
        let w = Int(ctx!.pointee.width > 0 ? ctx!.pointee.width : stream.pointee.codecpar.pointee.width)
        let h = Int(ctx!.pointee.height > 0 ? ctx!.pointee.height : stream.pointee.codecpar.pointee.height)
        return BitmapTrack(canvasWidth: w, canvasHeight: h, events: events, decoder: String(cString: codec.pointee.name))
    }
}
