import AVFoundation
import AVKit
import AppKit
import MediaPlayer
import Observation

/// One entry of an audio or subtitle menu.
struct TrackOption: Identifiable, Hashable {
    let id: Int
    let name: String
}

/// Owns the AVPlayer and everything the controls render. All state lives on the main actor; AVFoundation callbacks hop here.
@MainActor @Observable
final class PlayerModel: NSObject {
    let player = AVPlayer()
    let playerLayer = AVPlayerLayer()

    var title = "No media"
    var isPlaying = false
    var currentTime: Double = 0
    var duration: Double = 0
    var isScrubbing = false
    var errorText: String?
    var audioOptions: [TrackOption] = []
    var subtitleOptions: [TrackOption] = []
    var selectedAudio: Int?
    var selectedSubtitle: Int?
    var pipPossible = false
    var pipActive = false
    var isFullScreen = false
    var controlsVisible = true
    var showInfo = false
    var dynamicRange: CALayer.DynamicRange = .automatic
    var log: [String] = []
    /// When on, subtitles come from AVPlayerItemLegibleOutput and our SwiftUI view draws them (above the controls);
    /// when off, AVPlayerLayer draws them and our controls can cover them.
    var ownsSubtitleRendering = !CommandLine.arguments.contains("-systemSubtitles")
    var subtitleText: String?

    private(set) var queue: [URL] = []
    private var queueIndex = 0
    private var audioGroup: AVMediaSelectionGroup?
    private var subtitleGroup: AVMediaSelectionGroup?
    private var observations: [NSKeyValueObservation] = []
    private var itemObservations: [NSKeyValueObservation] = []
    private var itemTokens: [NSObjectProtocol] = []
    private var timeToken: Any?
    private var pip: AVPictureInPictureController?
    private var hideTask: Task<Void, Never>?

    override init() {
        super.init()
        setvbuf(stdout, nil, _IOLBF, 0)  // line-buffered log when stdout is a file
        playerLayer.player = player
        playerLayer.videoGravity = .resizeAspect
        playerLayer.backgroundColor = NSColor.black.cgColor
        playerLayer.preferredDynamicRange = dynamicRange
        // Header: "Default is YES on iOS, tvOS and in Mac Catalyst apps. Default is NO on macOS." (AVPlayer.h)
        player.preventsDisplaySleepDuringVideoPlayback = true
        player.allowsExternalPlayback = true

        observations.append(player.observe(\.timeControlStatus, options: [.initial, .new]) { @Sendable [weak self] _, _ in
            Task { @MainActor in self?.syncPlayState() }
        })
        timeToken = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 4), queue: .main) {
            @Sendable [weak self] time in
            MainActor.assumeIsolated {
                guard let self, !self.isScrubbing else { return }
                self.currentTime = time.seconds.isFinite ? time.seconds : 0
            }
        }
        setUpPictureInPicture()
        NowPlaying.shared.attach(to: self)
        observeWindows()
    }

    // MARK: Loading

    func open(_ urls: [URL]) {
        guard !urls.isEmpty else { return }
        queue = urls
        queueIndex = 0
        load(urls[0])
    }

    func next() {
        guard queueIndex + 1 < queue.count else { note("next: end of queue"); return }
        queueIndex += 1
        load(queue[queueIndex])
    }

    func previous() {
        if currentTime > 3 || queueIndex == 0 { seek(to: 0); return }
        queueIndex -= 1
        load(queue[queueIndex])
    }

    private func load(_ url: URL, displayName: String? = nil) {
        itemObservations.removeAll()
        itemTokens.forEach(NotificationCenter.default.removeObserver)
        itemTokens.removeAll()
        errorText = nil
        audioOptions = []
        subtitleOptions = []
        duration = 0
        currentTime = 0
        title = displayName ?? url.lastPathComponent
        note("load \(Redact.url(url))")

        let item = AVPlayerItem(asset: AVURLAsset(url: url))
        subtitleText = nil
        attachLegibleOutput(to: item)
        itemObservations.append(item.observe(\.status, options: [.new]) { @Sendable [weak self] _, _ in
            Task { @MainActor in self?.itemStatusChanged() }
        })
        let center = NotificationCenter.default
        itemTokens.append(center.addObserver(forName: AVPlayerItem.didPlayToEndTimeNotification, object: item, queue: .main) {
            @Sendable [weak self] _ in MainActor.assumeIsolated { self?.note("ended"); self?.next() }
        })
        itemTokens.append(center.addObserver(forName: AVPlayerItem.failedToPlayToEndTimeNotification, object: item, queue: .main) {
            @Sendable [weak self] note in
            let message = (note.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? NSError)?.localizedDescription ?? "?"
            MainActor.assumeIsolated { self?.note("failed to play to end: \(message)") }
        })
        itemTokens.append(center.addObserver(forName: AVPlayerItem.newErrorLogEntryNotification, object: item, queue: .main) {
            @Sendable [weak self] _ in
            MainActor.assumeIsolated {
                guard let event = self?.player.currentItem?.errorLog()?.events.last else { return }
                self?.note("error log: \(event.errorStatusCode) \(event.errorDomain) \(event.errorComment ?? "")")
            }
        })
        player.replaceCurrentItem(with: item)
        player.play()
        NowPlaying.shared.update(from: self)
    }

    func loadRemote(_ url: URL, displayName: String) {
        queue = [url]
        queueIndex = 0
        load(url, displayName: displayName)
    }

    private func itemStatusChanged() {
        guard let item = player.currentItem else { return }
        switch item.status {
        case .readyToPlay:
            note("ready to play")
            Task { await loadDetails(of: item) }
        case .failed:
            let error = item.error as NSError?
            errorText = error?.localizedDescription ?? "Failed"
            note("item failed: \(error?.domain ?? "") \(error?.code ?? 0) \(error?.localizedDescription ?? "") "
                + "\(error?.localizedFailureReason ?? "") \((error?.userInfo[NSUnderlyingErrorKey] as? NSError).map { "\($0.domain) \($0.code)" } ?? "")")
        default:
            break
        }
    }

    private func loadDetails(of item: AVPlayerItem) async {
        let asset = item.asset
        if let d = try? await asset.load(.duration), d.seconds.isFinite { duration = d.seconds }
        audioGroup = try? await asset.loadMediaSelectionGroup(for: .audible)
        subtitleGroup = try? await asset.loadMediaSelectionGroup(for: .legible)
        audioOptions = audioGroup?.options.enumerated().map { TrackOption(id: $0.offset, name: Self.name(of: $0.element)) } ?? []
        subtitleOptions = subtitleGroup?.options.enumerated().map { TrackOption(id: $0.offset, name: Self.name(of: $0.element)) } ?? []
        syncSelection()
        await describeTracks(of: item)
        NowPlaying.shared.update(from: self)
    }

    private static func name(of option: AVMediaSelectionOption) -> String {
        var parts = [option.displayName]
        if option.hasMediaCharacteristic(.containsOnlyForcedSubtitles) { parts.append("(forced)") }
        if option.hasMediaCharacteristic(.describesMusicAndSoundForAccessibility) { parts.append("(SDH)") }
        let types = option.mediaSubTypes.map { FourCC.string($0.uint32Value) }
        if !types.isEmpty { parts.append("[\(types.joined(separator: ","))]") }
        return parts.joined(separator: " ")
    }

    private func describeTracks(of item: AVPlayerItem) async {
        for track in item.tracks {
            guard let assetTrack = track.assetTrack else { continue }
            let descriptions = (try? await assetTrack.load(.formatDescriptions)) ?? []
            let codecs = descriptions.map { FourCC.string(CMFormatDescriptionGetMediaSubType($0)) }.joined(separator: ",")
            var extra = ""
            if assetTrack.mediaType == .video, let desc = descriptions.first {
                let ext = CMFormatDescriptionGetExtensions(desc) as? [String: Any] ?? [:]
                let transfer = ext[kCVImageBufferTransferFunctionKey as String] as? String ?? "-"
                let primaries = ext[kCVImageBufferColorPrimariesKey as String] as? String ?? "-"
                let hdr = (try? await assetTrack.load(.mediaCharacteristics))?.contains(.containsHDRVideo) ?? false
                extra = " transfer=\(transfer) primaries=\(primaries) containsHDRVideo=\(hdr)"
            }
            note("track \(assetTrack.mediaType.rawValue) \(codecs) enabled=\(track.isEnabled)\(extra)")
        }
        note("eligibleForHDRPlayback=\(AVPlayer.eligibleForHDRPlayback)")
    }

    // MARK: Transport

    func togglePlay() {
        if player.timeControlStatus == .paused { player.play() } else { player.pause() }
        poke()
    }

    func skip(_ seconds: Double) {
        seek(to: max(0, min(duration > 0 ? duration : .greatestFiniteMagnitude, currentTime + seconds)))
        poke()
    }

    func seek(to seconds: Double, exact: Bool = true) {
        currentTime = seconds
        let tolerance = exact ? CMTime.zero : CMTime.positiveInfinity
        player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: tolerance, toleranceAfter: tolerance) {
            @Sendable [weak self] _ in Task { @MainActor in if let self { NowPlaying.shared.update(from: self) } }
        }
    }

    private func syncPlayState() {
        let playing = player.timeControlStatus != .paused
        if playing != isPlaying {
            isPlaying = playing
            NowPlaying.shared.update(from: self)
        }
        poke()
    }

    // MARK: Media selection

    func selectAudio(_ id: Int?) {
        guard let group = audioGroup, let id, group.options.indices.contains(id) else { return }
        player.currentItem?.select(group.options[id], in: group)
        syncSelection()
    }

    func selectSubtitle(_ id: Int?) {
        guard let group = subtitleGroup else { return }
        if let id, group.options.indices.contains(id) {
            player.currentItem?.select(group.options[id], in: group)
        } else {
            player.currentItem?.select(nil, in: group)
        }
        syncSelection()
    }

    private func syncSelection() {
        guard let selection = player.currentItem?.currentMediaSelection else { return }
        if let group = audioGroup, let option = selection.selectedMediaOption(in: group) {
            selectedAudio = group.options.firstIndex(of: option)
        }
        if let group = subtitleGroup {
            selectedSubtitle = selection.selectedMediaOption(in: group).flatMap { group.options.firstIndex(of: $0) }
        }
    }

    // MARK: Subtitle rendering

    private func attachLegibleOutput(to item: AVPlayerItem) {
        item.outputs.compactMap { $0 as? AVPlayerItemLegibleOutput }.forEach(item.remove)
        let output = AVPlayerItemLegibleOutput(mediaSubtypesForNativeRepresentation: [])
        output.suppressesPlayerRendering = ownsSubtitleRendering
        output.setDelegate(self, queue: .main)
        item.add(output)
    }

    func setOwnsSubtitleRendering(_ value: Bool) {
        ownsSubtitleRendering = value
        subtitleText = nil
        if let item = player.currentItem { attachLegibleOutput(to: item) }
        note("own subtitle rendering=\(value)")
    }

    // MARK: Picture in Picture

    private func setUpPictureInPicture() {
        guard AVPictureInPictureController.isPictureInPictureSupported() else { note("PiP not supported"); return }
        let controller = AVPictureInPictureController(contentSource: .init(playerLayer: playerLayer))
        controller.delegate = self
        observations.append(controller.observe(\.isPictureInPicturePossible, options: [.initial, .new]) { @Sendable [weak self] c, _ in
            let possible = c.isPictureInPicturePossible
            Task { @MainActor in self?.pipPossible = possible }
        })
        observations.append(controller.observe(\.isPictureInPictureActive, options: [.initial, .new]) { @Sendable [weak self] c, _ in
            let active = c.isPictureInPictureActive
            Task { @MainActor in self?.pipActive = active; self?.note("pip active=\(active)") }
        })
        pip = controller
    }

    func togglePictureInPicture() {
        guard let pip else { return }
        if pip.isPictureInPictureActive { pip.stopPictureInPicture() } else { pip.startPictureInPicture() }
    }

    // MARK: Window and controls

    func toggleFullScreen() {
        (NSApp.keyWindow ?? NSApp.mainWindow)?.toggleFullScreen(nil)
    }

    func exitFullScreen() -> Bool {
        guard let window = NSApp.keyWindow, window.styleMask.contains(.fullScreen) else { return false }
        window.toggleFullScreen(nil)
        return true
    }

    private func observeWindows() {
        let center = NotificationCenter.default
        for (name, value) in [(NSWindow.didEnterFullScreenNotification, true), (NSWindow.didExitFullScreenNotification, false)] {
            _ = center.addObserver(forName: name, object: nil, queue: .main) { @Sendable [weak self] _ in
                MainActor.assumeIsolated { self?.isFullScreen = value }
            }
        }
    }

    /// Shows the controls and restarts the idle timer. Controls stay up while paused.
    func poke() {
        controlsVisible = true
        hideTask?.cancel()
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled, let self, self.isPlaying, !self.isScrubbing else { return }
            self.controlsVisible = false
            NSCursor.setHiddenUntilMouseMoves(true)
        }
    }

    func setDynamicRange(_ range: CALayer.DynamicRange) {
        dynamicRange = range
        playerLayer.preferredDynamicRange = range
        note("preferredDynamicRange=\(range.rawValue)")
    }

    func note(_ line: String) {
        let stamp = Date.now.formatted(date: .omitted, time: .standard)
        log.append("\(stamp) \(line)")
        if log.count > 300 { log.removeFirst(log.count - 300) }
        print("[spike] \(line)")
    }
}

extension PlayerModel: AVPictureInPictureControllerDelegate {
    nonisolated func pictureInPictureControllerWillStartPictureInPicture(_ controller: AVPictureInPictureController) {
        Task { @MainActor in self.note("pip will start") }
    }

    nonisolated func pictureInPictureControllerDidStopPictureInPicture(_ controller: AVPictureInPictureController) {
        Task { @MainActor in self.note("pip did stop") }
    }

    nonisolated func pictureInPictureController(
        _ controller: AVPictureInPictureController, failedToStartPictureInPictureWithError error: any Error
    ) {
        let message = error.localizedDescription
        Task { @MainActor in self.note("pip failed: \(message)") }
    }

    nonisolated func pictureInPictureController(
        _ controller: AVPictureInPictureController,
        restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping @Sendable (Bool) -> Void
    ) {
        Task { @MainActor in
            self.note("pip restore UI requested")
            NSApp.activate()
            NSApp.windows.first { $0.isVisible }?.makeKeyAndOrderFront(nil)
            completionHandler(true)
        }
    }
}

enum FourCC {
    static func string(_ code: UInt32) -> String {
        let bytes = [24, 16, 8, 0].map { UInt8((code >> $0) & 0xff) }
        if bytes.allSatisfy({ $0 >= 32 && $0 < 127 }) { return String(decoding: bytes, as: UTF8.self) }
        return String(format: "0x%08x", code)
    }
}

/// Keeps tokens out of the log: query strings are dropped from remote URLs.
enum Redact {
    static func url(_ url: URL) -> String {
        if url.isFileURL { return url.lastPathComponent }
        return "\(url.scheme ?? "?")://<server>\(url.path)"
    }
}

extension PlayerModel: AVPlayerItemLegibleOutputPushDelegate {
    nonisolated func legibleOutput(
        _ output: AVPlayerItemLegibleOutput, didOutputAttributedStrings strings: [NSAttributedString],
        nativeSampleBuffers nativeSamples: [Any], forItemTime itemTime: CMTime
    ) {
        // Delivered on the main queue (setDelegate(_:queue: .main)). Styling attributes (kCMTextMarkupAttribute_*) are
        // dropped here: the spike draws plain text.
        let text = strings.map(\.string).joined(separator: "\n")
        MainActor.assumeIsolated { self.subtitleText = text.isEmpty ? nil : text }
    }
}
