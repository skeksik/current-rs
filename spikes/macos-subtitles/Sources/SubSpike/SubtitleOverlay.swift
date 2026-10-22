import AVFoundation
import AppKit
import QuartzCore
import SubKit

/// One loaded subtitle file and how it is drawn.
enum SubtitleSource {
    case ass(AssRenderer)
    case bitmap(BitmapTrack)

    var label: String {
        switch self {
        case .ass: "libass"
        case .bitmap(let track): "\(track.decoder) \(track.canvasWidth)x\(track.canvasHeight)"
        }
    }
}

struct SubtitleTrackOption: Identifiable {
    let id: Int
    let name: String
    let source: SubtitleSource
}

/// Our subtitle layer: a sibling above the AVPlayerLayer, covering exactly the layer's videoRect, top-left origin.
/// Owns no clock: the model calls `draw(at:)` from the clock under test.
@MainActor
final class SubtitleOverlay {
    let root = CALayer()  // frame = videoRect, geometry flipped
    private let content = CALayer()  // the current subtitle image
    var source: SubtitleSource? { didSet { lastKey = nil; content.contents = nil } }
    var bottomInsetPoints: CGFloat = 0  // distance our control bar reaches up from the view's bottom
    var whiteLevel = 1.0 {
        didSet {
            if case .ass(let r) = source { r.whiteLevel = whiteLevel }
            lastKey = nil
            // Above 1.0 the image holds extended-range values; the layer must ask for high dynamic range to show them.
            content.preferredDynamicRange = whiteLevel > 1 ? .high : .standard
            content.contentsHeadroom = whiteLevel > 1 ? whiteLevel : 0  // CALayer.h: untagged contents need it
        }
    }
    private var videoRect = CGRect.zero
    private var scale: CGFloat = 2
    private var lastKey: String?
    private(set) var lastTime = 0.0
    // Stats since the last report.
    var draws = 0, uploads = 0
    var drawSeconds = 0.0, maxDrawSeconds = 0.0

    init() {
        root.isGeometryFlipped = true
        root.masksToBounds = true
        content.magnificationFilter = .linear
        content.minificationFilter = .trilinear
        content.contentsGravity = .resize
        root.addSublayer(content)
        root.actions = ["position": NSNull(), "bounds": NSNull(), "contents": NSNull(), "hidden": NSNull()]
        content.actions = root.actions
    }

    func layout(videoRect: CGRect, scale: CGFloat) {
        guard videoRect != self.videoRect || scale != self.scale else { return }
        self.videoRect = videoRect
        self.scale = scale
        root.frame = videoRect
        lastKey = nil
    }

    /// The bar's overlap with the video, in points from the video's bottom edge.
    private var insetInVideo: CGFloat { max(0, bottomInsetPoints - videoRect.minY) }

    func draw(at seconds: Double) {
        lastTime = seconds
        guard let source, videoRect.width > 1 else { content.contents = nil; return }
        let started = CFAbsoluteTimeGetCurrent()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer {
            CATransaction.commit()
            let d = CFAbsoluteTimeGetCurrent() - started
            draws += 1
            drawSeconds += d
            maxDrawSeconds = max(maxDrawSeconds, d)
        }
        switch source {
        case .ass(let renderer):
            // libass renders at the on-screen pixel size of the video, so text is sharp at any window size.
            let w = Int((videoRect.width * scale).rounded()), h = Int((videoRect.height * scale).rounded())
            let frame = renderer.render(milliseconds: Int64((seconds * 1000).rounded()), width: w, height: h,
                                        bottomInset: Int(insetInVideo * scale))
            let key = frame.image.map { "\(ObjectIdentifier($0))" } ?? "none"
            guard key != lastKey else { return }
            lastKey = key
            uploads += 1
            content.contents = frame.image
            content.frame = CGRect(x: frame.rect.minX / scale, y: frame.rect.minY / scale,
                                   width: frame.rect.width / scale, height: frame.rect.height / scale)
        case .bitmap(let track):
            let index = track.event(at: seconds)
            let inset = insetInVideo
            let key = "\(index ?? -1)/\(inset)/\(whiteLevel)"
            guard key != lastKey else { return }
            lastKey = key
            uploads += 1
            guard let index else { content.contents = nil; return }
            let frame = track.compose(index)
            // Canvas pixels to video points; aspect differences (720x576 on 16:9) stretch like the video itself.
            let sx = videoRect.width / CGFloat(track.canvasWidth), sy = videoRect.height / CGFloat(track.canvasHeight)
            var rect = CGRect(x: frame.rect.minX * sx, y: frame.rect.minY * sy, width: frame.rect.width * sx, height: frame.rect.height * sy)
            // Bitmaps in the lower half move up just enough to clear the bar; signs higher up stay where they are.
            if rect.midY > videoRect.height / 2 { rect.origin.y -= max(0, rect.maxY - (videoRect.height - inset)) }
            content.contents = whiteLevel == 1 ? frame.image : dimmed(frame.image)
            content.frame = rect
        }
    }

    private func dimmed(_ image: CGImage?) -> CGImage? {
        guard let image, let data = image.dataProvider?.data as Data? else { return image }
        var canvas = Canvas(width: image.width, height: image.height)
        canvas.pixels = [UInt8](data)
        return canvas.makeImage(whiteLevel: whiteLevel)
    }

    func setHidden(_ hidden: Bool) { root.isHidden = hidden }
}

/// Hosts the AVPlayerLayer and our subtitle layer above it, and drives the display link.
final class LayerHostView: NSView {
    var playerLayer: AVPlayerLayer?
    var overlay: SubtitleOverlay?
    var onFrame: ((CADisplayLink) -> Void)?
    var onLayout: ((LayerHostView) -> Void)?
    private var link: CADisplayLink?

    func attach(playerLayer: AVPlayerLayer, overlay: SubtitleOverlay) {
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        self.playerLayer = playerLayer
        self.overlay = overlay
        layer?.addSublayer(playerLayer)
        layer?.addSublayer(overlay.root)
        playerLayer.addObserver(self, forKeyPath: "videoRect", options: [.new], context: nil)
    }

    override func observeValue(forKeyPath keyPath: String?, of object: Any?, change: [NSKeyValueChangeKey: Any]?,
                               context: UnsafeMutableRawPointer?) {
        DispatchQueue.main.async { self.relayout() }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        link?.invalidate()
        guard window != nil else { return }
        // NSView.displayLink (macOS 14+) follows the screen the view is on.
        let link = displayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func tick(_ link: CADisplayLink) { onFrame?(link) }

    override func layout() {
        super.layout()
        relayout()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        relayout()
    }

    func relayout() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        playerLayer?.frame = bounds
        if let playerLayer, let overlay {
            overlay.layout(videoRect: playerLayer.videoRect, scale: window?.backingScaleFactor ?? 2)
        }
        CATransaction.commit()
        onLayout?(self)
    }

    /// The video rect in global screen coordinates with a top-left origin (what `screencapture -R` takes).
    func videoRectOnScreen() -> CGRect? {
        guard let window, let playerLayer, let primary = NSScreen.screens.first else { return nil }
        let inWindow = convert(playerLayer.videoRect, to: nil)
        let onScreen = window.convertToScreen(inWindow)
        return CGRect(x: onScreen.minX, y: primary.frame.maxY - onScreen.maxY, width: onScreen.width, height: onScreen.height)
    }

    override var mouseDownCanMoveWindow: Bool { false }
}
