import AVFoundation
import AVKit
import AppKit
import Observation
import SubKit

/// Which clock drives the subtitle layer (the D7 comparison).
enum ClockMode: String, CaseIterable {
    case periodic  // addPeriodicTimeObserver every 1/60 s, on the main queue
    case link  // display link callback, AVPlayer.currentTime()
    case output  // display link callback, AVPlayerItemVideoOutput.itemTime(forHostTime: targetTimestamp)
}

@MainActor @Observable
final class PlayerModel: NSObject {
    let player = AVPlayer()
    let playerLayer = AVPlayerLayer()
    @ObservationIgnored let overlay = SubtitleOverlay()
    @ObservationIgnored weak var host: LayerHostView?

    var title = "No media"
    var isPlaying = false
    var rate: Float = 1
    var currentTime: Double = 0
    var duration: Double = 0
    var isScrubbing = false
    var pipPossible = false
    var pipActive = false
    var isFullScreen = false
    var controlsVisible = true
    var pinControls = false
    var showInfo = false
    var clock: ClockMode = .periodic
    var whiteLevel = 1.0
    var subtitleOptions: [SubtitleTrackOption] = []
    var selectedSubtitle: Int?
    var statsLine = ""
    var log: [String] = []

    /// Points from the window's bottom edge to just above our control bar.
    static let barReach: CGFloat = 122

    @ObservationIgnored private var frameRate = 0.0
    @ObservationIgnored private var periodicToken: Any?
    @ObservationIgnored private var uiToken: Any?
    @ObservationIgnored private var videoOutput: AVPlayerItemVideoOutput?
    @ObservationIgnored private var observations: [NSKeyValueObservation] = []
    @ObservationIgnored private var pip: AVPictureInPictureController?
    @ObservationIgnored private var hideTask: Task<Void, Never>?
    @ObservationIgnored private var statsTask: Task<Void, Never>?

    override init() {
        super.init()
        setvbuf(stdout, nil, _IOLBF, 0)
        playerLayer.player = player
        playerLayer.videoGravity = .resizeAspect
        playerLayer.preferredDynamicRange = .automatic
        player.preventsDisplaySleepDuringVideoPlayback = true
        observations.append(player.observe(\.timeControlStatus, options: [.initial, .new]) { @Sendable [weak self] _, _ in
            Task { @MainActor in self?.syncPlayState() }
        })
        uiToken = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 4), queue: .main) { @Sendable [weak self] t in
            MainActor.assumeIsolated {
                guard let self, !self.isScrubbing else { return }
                self.currentTime = t.seconds.isFinite ? t.seconds : 0
            }
        }
        AssRenderer.log = { line in Task { @MainActor in PlayerModel.shared?.note(line) } }
        PlayerModel.shared = self
        setUpPictureInPicture()
        setClock(.periodic)
        startStats()
        let center = NotificationCenter.default
        for (name, value) in [(NSWindow.didEnterFullScreenNotification, true), (NSWindow.didExitFullScreenNotification, false)] {
            _ = center.addObserver(forName: name, object: nil, queue: .main) { @Sendable [weak self] _ in
                MainActor.assumeIsolated { self?.isFullScreen = value }
            }
        }
    }

    @ObservationIgnored static weak var shared: PlayerModel?

    // MARK: Loading

    func open(video: URL, subtitles: [URL], fonts: [URL], provider: AssRenderer.FontProvider, pgsParser: String) {
        title = video.lastPathComponent
        note("load \(video.isFileURL ? video.lastPathComponent : video.absoluteString)")
        let item = AVPlayerItem(url: video)
        let output = AVPlayerItemVideoOutput(pixelBufferAttributes: nil)
        item.add(output)
        videoOutput = output
        player.replaceCurrentItem(with: item)
        Task {
            if let d = try? await item.asset.load(.duration), d.seconds.isFinite { duration = d.seconds }
            let track = try? await item.asset.loadTracks(withMediaType: .video).first
            let size = (try? await track?.load(.naturalSize)) ?? CGSize(width: 1920, height: 1080)
            frameRate = Double((try? await track?.load(.nominalFrameRate)) ?? 0)
            note("video \(Int(size.width))x\(Int(size.height)) at \(frameRate) fps")
            loadSubtitles(subtitles, fonts: fonts, provider: provider, pgsParser: pgsParser, storage: size)
        }
        player.play()
    }

    private func loadSubtitles(_ urls: [URL], fonts: [URL], provider: AssRenderer.FontProvider, pgsParser: String, storage: CGSize) {
        let attachments = fonts.compactMap { url in (try? Data(contentsOf: url)).map { (name: url.lastPathComponent, data: $0) } }
        var options: [SubtitleTrackOption] = []
        for (i, url) in urls.enumerated() {
            let started = CFAbsoluteTimeGetCurrent()
            let source: SubtitleSource?
            switch url.pathExtension.lowercased() {
            case "ass", "ssa":
                source = (try? Data(contentsOf: url)).flatMap {
                    AssRenderer(script: $0, attachments: attachments, provider: provider, storageSize: storage)
                }.map { .ass($0) }
            case "sup" where pgsParser == "swift":
                source = (try? PGS.read(Data(contentsOf: url))).map { .bitmap($0) }
            default:
                do { source = .bitmap(try LibavSubtitleDecoder.decode(path: url.path)) } catch {
                    note("decode failed: \(error)")
                    source = nil
                }
            }
            guard let source else { note("could not load \(url.lastPathComponent)"); continue }
            note(String(format: "loaded %@ with %@ in %.1f ms", url.lastPathComponent, source.label, (CFAbsoluteTimeGetCurrent() - started) * 1000))
            options.append(SubtitleTrackOption(id: i, name: url.lastPathComponent, source: source))
        }
        subtitleOptions = options
        selectSubtitle(options.first?.id)
    }

    func selectSubtitle(_ id: Int?) {
        selectedSubtitle = id
        overlay.source = subtitleOptions.first { $0.id == id }?.source
        overlay.whiteLevel = whiteLevel
        overlay.draw(at: player.currentTime().seconds)
        note("subtitle: \(subtitleOptions.first { $0.id == id }?.name ?? "off")")
    }

    // MARK: Clocks

    func setClock(_ mode: ClockMode) {
        clock = mode
        if let periodicToken { player.removeTimeObserver(periodicToken) }
        periodicToken = nil
        if mode == .periodic {
            periodicToken = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 60), queue: .main) {
                @Sendable [weak self] t in MainActor.assumeIsolated { self?.overlay.draw(at: t.seconds) }
            }
        }
        note("clock: \(mode.rawValue)")
    }

    func cycleClock() {
        let all = ClockMode.allCases
        setClock(all[(all.firstIndex(of: clock)! + 1) % all.count])
    }

    /// Display link callback (every refresh of the view's screen).
    func displayLinkFired(_ link: CADisplayLink) {
        switch clock {
        case .periodic:
            break
        case .link:
            overlay.draw(at: player.currentTime().seconds)
        case .output:
            // While paused, itemTime(forHostTime:) read one frame behind the frame on screen (measured); currentTime() matches.
            guard let videoOutput, player.rate != 0 else { overlay.draw(at: player.currentTime().seconds); return }
            let t = videoOutput.itemTime(forHostTime: link.targetTimestamp)
            guard t.isValid else { return }
            // Snap to the video's frame grid: the picture changes only per video frame, so libass need not render at the
            // display's refresh rate (60 Hz here) for 24/25 fps video.
            overlay.draw(at: frameRate > 0 ? (t.seconds * frameRate + 0.001).rounded(.down) / frameRate : t.seconds)
        }
    }

    // MARK: Transport

    func togglePlay() {
        if player.timeControlStatus == .paused { player.rate = rate } else { player.pause() }
        poke()
    }

    func skip(_ seconds: Double) {
        seek(to: max(0, min(duration > 0 ? duration : .greatestFiniteMagnitude, player.currentTime().seconds + seconds)))
        poke()
    }

    func seek(to seconds: Double, exact: Bool = true) {
        currentTime = seconds
        let tolerance = exact ? CMTime.zero : CMTime.positiveInfinity
        player.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: tolerance, toleranceAfter: tolerance)
    }

    func changeRate(up: Bool) {
        let rates: [Float] = [0.5, 1, 1.5, 2]
        let i = rates.firstIndex(of: rate) ?? 1
        rate = rates[max(0, min(rates.count - 1, i + (up ? 1 : -1)))]
        if player.timeControlStatus != .paused { player.rate = rate }
        note("rate \(rate)")
    }

    private func syncPlayState() {
        isPlaying = player.timeControlStatus != .paused
        poke()
    }

    func cycleWhiteLevel() {
        let levels = [1.0, 0.8, 2.0]
        whiteLevel = levels[((levels.firstIndex(of: whiteLevel) ?? 0) + 1) % levels.count]
        overlay.whiteLevel = whiteLevel
        overlay.draw(at: overlay.lastTime)
        note(String(format: "subtitle white level %.1f × SDR reference white%@", whiteLevel, whiteLevel > 1 ? " (EDR, layer .high)" : ""))
    }

    // MARK: Picture in Picture

    private func setUpPictureInPicture() {
        guard AVPictureInPictureController.isPictureInPictureSupported() else { return }
        let controller = AVPictureInPictureController(contentSource: .init(playerLayer: playerLayer))
        controller.delegate = self
        observations.append(controller.observe(\.isPictureInPicturePossible, options: [.initial, .new]) { @Sendable [weak self] c, _ in
            let possible = c.isPictureInPicturePossible
            Task { @MainActor in self?.pipPossible = possible }
        })
        observations.append(controller.observe(\.isPictureInPictureActive, options: [.initial, .new]) { @Sendable [weak self] c, _ in
            let active = c.isPictureInPictureActive
            Task { @MainActor in
                guard let self else { return }
                self.pipActive = active
                // Our layer is not part of the PiP window; over the system's placeholder it would only be noise.
                self.overlay.setHidden(active)
                self.note("pip active=\(active)")
            }
        })
        pip = controller
    }

    func togglePictureInPicture() {
        guard let pip else { return }
        if pip.isPictureInPictureActive { pip.stopPictureInPicture() } else { pip.startPictureInPicture() }
    }

    // MARK: Window and controls

    func toggleFullScreen() { (NSApp.keyWindow ?? NSApp.mainWindow)?.toggleFullScreen(nil) }

    func exitFullScreen() -> Bool {
        guard let window = NSApp.keyWindow, window.styleMask.contains(.fullScreen) else { return false }
        window.toggleFullScreen(nil)
        return true
    }

    func poke() {
        setControls(visible: true)
        hideTask?.cancel()
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled, let self, self.isPlaying, !self.isScrubbing, !self.pinControls else { return }
            self.setControls(visible: false)
            NSCursor.setHiddenUntilMouseMoves(true)
        }
    }

    private func setControls(visible: Bool) {
        guard visible != controlsVisible || overlay.bottomInsetPoints == 0 else { return }
        controlsVisible = visible
        overlay.bottomInsetPoints = visible ? Self.barReach : 0
        overlay.draw(at: overlay.lastTime)
    }

    /// Prints the video rect for `screencapture -R` (sync measurement).
    @ObservationIgnored private var lastSyncRect = CGRect.zero
    func printSyncRect() {
        guard let r = host?.videoRectOnScreen(), r != lastSyncRect, r.width > 1 else { return }
        lastSyncRect = r
        note(String(format: "SYNC-RECT %.0f,%.0f,%.0f,%.0f", r.minX, r.minY, r.width, r.height))
    }

    /// Timed actions for unattended sync runs: "seconds:action[=value]", comma separated, relative to now.
    func runActions(_ script: String) {
        for step in script.split(separator: ",") {
            let parts = step.split(separator: ":", maxSplits: 1)
            guard parts.count == 2, let at = Double(parts[0]) else { continue }
            let action = parts[1].split(separator: "=", maxSplits: 1).map(String.init)
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(at))
                guard let self else { return }
                switch action[0] {
                case "seek": self.seek(to: Double(action[1]) ?? 0)
                case "rate": self.rate = Float(action[1]) ?? 1; self.player.rate = self.rate
                case "pause": self.player.pause()
                case "play": self.player.rate = self.rate
                case "clock": if let mode = ClockMode(rawValue: action[1]) { self.setClock(mode) }
                case "sub": self.selectSubtitle(Int(action[1]))
                case "fullscreen": self.toggleFullScreen()
                case "pip": self.togglePictureInPicture()
                case "quit": NSApp.terminate(nil)
                default: break
                }
                self.note(String(format: "ACTION %@ at %.3f", String(step), CFAbsoluteTimeGetCurrent()))
            }
        }
    }

    // MARK: Stats

    private func startStats() {
        statsTask = Task { [weak self] in
            var lastCPU = Self.cpuSeconds(), lastWall = CFAbsoluteTimeGetCurrent()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard let self else { return }
                let cpu = Self.cpuSeconds(), wall = CFAbsoluteTimeGetCurrent()
                let o = self.overlay
                self.statsLine = String(format: "clock %@ | draws %d uploads %d | draw avg %.2f ms max %.2f ms | process CPU %.0f%%",
                                        self.clock.rawValue, o.draws, o.uploads, o.draws > 0 ? o.drawSeconds / Double(o.draws) * 1000 : 0,
                                        o.maxDrawSeconds * 1000, (cpu - lastCPU) / (wall - lastWall) * 100)
                print("[stats] \(self.statsLine)")
                o.draws = 0; o.uploads = 0; o.drawSeconds = 0; o.maxDrawSeconds = 0
                lastCPU = cpu
                lastWall = wall
            }
        }
    }

    nonisolated static func cpuSeconds() -> Double {
        var ts = timespec()
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &ts)
        return Double(ts.tv_sec) + Double(ts.tv_nsec) / 1e9
    }

    func note(_ line: String) {
        let stamp = Date.now.formatted(date: .omitted, time: .standard)
        log.append("\(stamp) \(line)")
        if log.count > 300 { log.removeFirst(log.count - 300) }
        print("[spike] \(line)")
    }
}

extension PlayerModel: AVPictureInPictureControllerDelegate {
    nonisolated func pictureInPictureController(
        _ controller: AVPictureInPictureController,
        restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping @Sendable (Bool) -> Void
    ) {
        Task { @MainActor in
            NSApp.activate()
            NSApp.windows.first { $0.isVisible }?.makeKeyAndOrderFront(nil)
            completionHandler(true)
        }
    }
}
