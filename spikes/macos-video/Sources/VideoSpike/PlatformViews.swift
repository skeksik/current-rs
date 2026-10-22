import AVKit
import AppKit
import SwiftUI

/// Hosts the model's AVPlayerLayer. The layer is created once and moved between hosts, so PiP keeps its reference.
struct PlayerLayerView: NSViewRepresentable {
    let model: PlayerModel

    func makeNSView(context: Context) -> LayerHostView {
        let view = LayerHostView()
        view.playerLayer = model.playerLayer
        return view
    }

    func updateNSView(_ view: LayerHostView, context: Context) {}

    final class LayerHostView: NSView {
        var playerLayer: AVPlayerLayer? {
            didSet {
                wantsLayer = true
                layer?.backgroundColor = NSColor.black.cgColor
                if let playerLayer { layer?.addSublayer(playerLayer) }
            }
        }

        override func layout() {
            super.layout()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            playerLayer?.frame = bounds
            CATransaction.commit()
        }

        // The window is moved by its title bar only, as in a native player; clicks on the video reach SwiftUI.
        override var mouseDownCanMoveWindow: Bool { false }
    }
}

/// The system AirPlay picker, which presents its own native route menu.
struct RoutePicker: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.player = player
        view.isRoutePickerButtonBordered = false
        view.setRoutePickerButtonColor(.white, for: .normal)
        view.setAccessibilityLabel("AirPlay")
        return view
    }

    func updateNSView(_ view: AVRoutePickerView, context: Context) {}
}
