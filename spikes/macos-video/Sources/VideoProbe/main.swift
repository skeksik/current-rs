// Probes what AVFoundation does with each file or URL: opens it, lists tracks, decodes audio to PCM, and plays a few seconds
// through AVPlayer into an AVPlayerItemVideoOutput to see whether frames come out.
// Usage: VideoProbe [--caps] <file or URL>...
import AVFoundation
import CoreMedia
import VideoToolbox

func fourCC(_ code: FourCharCode) -> String {
    let bytes = [24, 16, 8, 0].map { UInt8((code >> $0) & 0xff) }
    return bytes.allSatisfy { $0 >= 32 && $0 < 127 } ? String(decoding: bytes, as: UTF8.self) : String(format: "0x%08x", code)
}

@MainActor
func capabilities() {
    print("== capabilities")
    let mime = Set(AVURLAsset.audiovisualMIMETypes())
    for type in ["video/mp4", "video/quicktime", "video/x-matroska", "video/webm", "video/mp2t", "application/vnd.apple.mpegurl", "video/x-msvideo"] {
        print("MIME \(type): \(mime.contains(type))")
    }
    let extended = [
        #"video/mp4; codecs="avc1.640028""#, #"video/mp4; codecs="hvc1.2.4.L150.B0""#, #"video/mp4; codecs="hev1.2.4.L150.B0""#,
        #"video/mp4; codecs="dvh1.08.06""#, #"video/mp4; codecs="dvh1.05.06""#, #"video/mp4; codecs="av01.0.08M.10""#,
        #"video/mp4; codecs="vp09.00.40.08""#, #"audio/mp4; codecs="ac-3""#, #"audio/mp4; codecs="ec-3""#,
        #"audio/mp4; codecs="mlpa""#, #"audio/mp4; codecs="dtsc""#, #"audio/mp4; codecs="dtsx""#, #"audio/mp4; codecs="fLaC""#,
        #"audio/mp4; codecs="Opus""#, #"audio/mp4; codecs="alac""#, #"video/mp4; codecs="wvtt""#, #"video/mp4; codecs="stpp""#,
    ]
    for type in extended { print("playable \(type): \(AVURLAsset.isPlayableExtendedMIMEType(type))") }
    let codecs: [(String, CMVideoCodecType)] = [
        ("h264", kCMVideoCodecType_H264), ("hevc", kCMVideoCodecType_HEVC), ("av1", kCMVideoCodecType_AV1),
        ("vp9", kCMVideoCodecType_VP9), ("prores4444", kCMVideoCodecType_AppleProRes4444),
    ]
    for (name, codec) in codecs { print("VT hardware decode \(name): \(VTIsHardwareDecodeSupported(codec))") }
    print("eligibleForHDRPlayback: \(AVPlayer.eligibleForHDRPlayback)")
}

@MainActor
func probe(_ argument: String) async {
    let url = argument.contains("://") ? URL(string: argument)! : URL(fileURLWithPath: argument)
    print("== \(url.isFileURL ? url.lastPathComponent : "remote " + url.path)")
    let asset = AVURLAsset(url: url)
    do {
        let (playable, readable) = try await asset.load(.isPlayable, .isReadable)
        print("asset playable=\(playable) readable=\(readable)")
        for track in try await asset.load(.tracks) {
            let (descriptions, trackPlayable, decodable) = try await track.load(.formatDescriptions, .isPlayable, .isDecodable)
            var line = "track \(track.trackID) \(track.mediaType.rawValue) \(descriptions.map { fourCC(CMFormatDescriptionGetMediaSubType($0)) })"
            line += " playable=\(trackPlayable) decodable=\(decodable)"
            if track.mediaType == .video, let desc = descriptions.first {
                let ext = CMFormatDescriptionGetExtensions(desc) as? [String: Any] ?? [:]
                line += " transfer=\(ext[kCVImageBufferTransferFunctionKey as String] as? String ?? "-")"
                line += " HDR=\(try await track.load(.mediaCharacteristics).contains(.containsHDRVideo))"
            }
            print(line)
        }
        for characteristic in [AVMediaCharacteristic.audible, .legible] {
            if let group = try await asset.loadMediaSelectionGroup(for: characteristic) {
                print("selection \(characteristic.rawValue): \(group.options.map { "\($0.displayName)[\($0.mediaSubTypes.map { fourCC($0.uint32Value) }.joined(separator: ","))]" })")
            }
        }
        if url.isFileURL { await decodeAudio(asset) }
    } catch {
        let e = error as NSError
        print("asset load failed: \(e.domain) \(e.code) \(e.localizedDescription) \(e.localizedFailureReason ?? "")")
    }
    await play(url)
}

/// Decodes the first buffer of each audio track to LPCM: tells whether the OS has a decoder for it.
@MainActor
func decodeAudio(_ asset: AVURLAsset) async {
    guard let tracks = try? await asset.loadTracks(withMediaType: .audio) else { return }
    for track in tracks {
        do {
            let reader = try AVAssetReader(asset: asset)
            let output = AVAssetReaderTrackOutput(track: track, outputSettings: [AVFormatIDKey: kAudioFormatLinearPCM])
            reader.add(output)
            reader.startReading()
            let buffer = output.copyNextSampleBuffer()
            let ok = buffer != nil && reader.status != .failed
            print("audio track \(track.trackID) decode to PCM: \(ok ? "ok" : "FAILED \(reader.error.map { ($0 as NSError).code } ?? 0)")")
            reader.cancelReading()
        } catch {
            print("audio track \(track.trackID) reader failed: \((error as NSError).code)")
        }
    }
}

/// Plays up to 6 s muted; reports item status, how far time moved and whether decoded frames arrived.
@MainActor
func play(_ url: URL) async {
    let item = AVPlayerItem(url: url)
    let output = AVPlayerItemVideoOutput(pixelBufferAttributes: nil)
    item.add(output)
    let player = AVPlayer(playerItem: item)
    player.isMuted = true
    player.play()
    var frames = 0
    var format = "-"
    let start = Date()
    while Date().timeIntervalSince(start) < 6, item.status != .failed {
        let time = output.itemTime(forHostTime: CACurrentMediaTime())
        if output.hasNewPixelBuffer(forItemTime: time), let buffer = output.copyPixelBuffer(forItemTime: time, itemTimeForDisplay: nil) {
            frames += 1
            format = "\(fourCC(CVPixelBufferGetPixelFormatType(buffer))) \(CVPixelBufferGetWidth(buffer))x\(CVPixelBufferGetHeight(buffer))"
        }
        try? await Task.sleep(for: .milliseconds(20))
    }
    let status = ["unknown", "readyToPlay", "failed"][item.status.rawValue]
    var line = "play status=\(status) advanced=\(String(format: "%.1f", player.currentTime().seconds)) s frames=\(frames) format=\(format)"
    if let e = item.error as NSError? {
        let underlying = (e.userInfo[NSUnderlyingErrorKey] as? NSError).map { " (\($0.domain) \($0.code))" } ?? ""
        line += " error=\(e.domain) \(e.code) \(e.localizedDescription)\(underlying)"
    }
    if let events = item.errorLog()?.events, let last = events.last {
        line += " errorlog=\(last.errorStatusCode) \(last.errorComment ?? "")"
    }
    let enabled = item.tracks.filter(\.isEnabled).compactMap { $0.assetTrack?.mediaType.rawValue }
    line += " enabledTracks=\(enabled)"
    print(line)
    player.pause()
}

@MainActor
func run() async {
    var arguments = Array(CommandLine.arguments.dropFirst())
    if arguments.first == "--supplemental" {
        // Opt in to the system's supplemental decoders (VP9 on macOS) before probing.
        arguments.removeFirst()
        VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_VP9)
        print("registered supplemental VP9; VT hardware decode vp9: \(VTIsHardwareDecodeSupported(kCMVideoCodecType_VP9))")
    }
    if arguments.first == "--caps" {
        arguments.removeFirst()
        capabilities()
    }
    for argument in arguments { await probe(argument) }
}

await run()
