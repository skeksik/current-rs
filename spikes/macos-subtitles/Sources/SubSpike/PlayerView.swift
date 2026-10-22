import AVKit
import SwiftUI

/// Hosts the AVPlayerLayer and our subtitle layer (both AppKit layers, composited together).
struct PlayerLayerView: NSViewRepresentable {
    let model: PlayerModel

    func makeNSView(context: Context) -> LayerHostView {
        let view = LayerHostView()
        view.attach(playerLayer: model.playerLayer, overlay: model.overlay)
        view.onFrame = { [weak model] link in model?.displayLinkFired(link) }
        model.host = view
        view.onLayout = { [weak model] _ in model?.printSyncRect() }
        return view
    }

    func updateNSView(_ view: LayerHostView, context: Context) {}
}

struct PlayerView: View {
    @Bindable var model: PlayerModel

    var body: some View {
        ZStack {
            Color.black
            PlayerLayerView(model: model)
        }
        .overlay(alignment: .bottom) {
            if !model.pipActive {
                ControlBar(model: model)
                    .padding(.horizontal, 24)
                    .padding(.bottom, 20)
                    // Faded, never removed (ADR 0008: views in the bar live as long as the player).
                    .opacity(model.controlsVisible ? 1 : 0)
                    .allowsHitTesting(model.controlsVisible)
            }
        }
        .overlay(alignment: .topLeading) {
            if model.showInfo { InfoPanel(model: model).padding(12).padding(.top, 28) }
        }
        .animation(.easeInOut(duration: 0.25), value: model.controlsVisible)
        .onContinuousHover { _ in model.poke() }
        .onTapGesture(count: 2) { model.toggleFullScreen() }
        .ignoresSafeArea()
        .navigationTitle(model.title)
    }
}

struct ControlBar: View {
    @Bindable var model: PlayerModel

    var body: some View {
        GlassEffectContainer {
            VStack(spacing: 6) {
                HStack(spacing: 10) {
                    Text(Self.format(model.currentTime))
                    Slider(
                        value: Binding(get: { model.currentTime }, set: { model.seek(to: $0, exact: false) }),
                        in: 0...max(model.duration, 0.1),
                        onEditingChanged: { editing in
                            model.isScrubbing = editing
                            if !editing { model.seek(to: model.currentTime) }
                            model.poke()
                        }
                    )
                    .controlSize(.small)
                    .accessibilityLabel("Playback position")
                    Text("-" + Self.format(max(0, model.duration - model.currentTime)))
                }
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)

                HStack(spacing: 18) {
                    Menu {
                        Picker("Subtitles", selection: Binding(get: { model.selectedSubtitle }, set: { model.selectSubtitle($0) })) {
                            Text("Off").tag(Int?.none)
                            ForEach(model.subtitleOptions) { Text($0.name).tag(Optional($0.id)) }
                        }
                        .pickerStyle(.inline)
                    } label: {
                        Label("Subtitles", systemImage: "captions.bubble").labelStyle(.iconOnly)
                    }
                    .menuIndicator(.hidden)
                    .fixedSize()
                    Text(String(format: "%.1f×", model.rate)).font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                    Spacer()
                    button("gobackward.10", "Skip back 10 seconds") { model.skip(-10) }
                    button(model.isPlaying ? "pause.fill" : "play.fill", model.isPlaying ? "Pause" : "Play", size: 26) { model.togglePlay() }
                    button("goforward.10", "Skip forward 10 seconds") { model.skip(10) }
                    Spacer()
                    button(model.pipActive ? "pip.exit" : "pip.enter", "Picture in Picture") { model.togglePictureInPicture() }
                        .disabled(!model.pipPossible)
                    button(model.isFullScreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                           model.isFullScreen ? "Exit Full Screen" : "Enter Full Screen") { model.toggleFullScreen() }
                }
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            .frame(maxWidth: 720)
            .glassEffect(.regular.tint(.black.opacity(0.35)), in: .rect(cornerRadius: 22))
        }
        .environment(\.colorScheme, .dark)
    }

    private func button(_ symbol: String, _ label: String, size: CGFloat = 17, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: size, weight: .medium))
                .frame(width: size + 10, height: size + 10)
                .contentShape(.rect)
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(label)
        .help(label)
    }

    static func format(_ seconds: Double) -> String {
        guard seconds.isFinite else { return "--:--" }
        let s = Int(seconds.rounded(.down))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}

struct InfoPanel: View {
    @Bindable var model: PlayerModel
    @State private var screenLine = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(screenLine).font(.caption.monospaced())
            Text(model.statsLine).font(.caption.monospaced())
            Text(String(format: "white level %.1f × SDR white | keys: C clock, W white, B pin bar, [ ] rate, P PiP, R sync rect",
                        model.whiteLevel)).font(.caption.monospaced())
            ScrollView {
                Text(model.log.suffix(40).joined(separator: "\n"))
                    .font(.caption2.monospaced())
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(width: 620, height: 200)
        }
        .padding(12)
        .glassEffect(.regular.tint(.black.opacity(0.35)), in: .rect(cornerRadius: 14))
        .environment(\.colorScheme, .dark)
        .task {
            while !Task.isCancelled {
                let screen = NSApp.keyWindow?.screen ?? NSScreen.main
                screenLine = String(format: "screen %@: EDR now %.2f, potential %.2f, reference %.2f",
                                    screen?.localizedName ?? "?", screen?.maximumExtendedDynamicRangeColorComponentValue ?? 0,
                                    screen?.maximumPotentialExtendedDynamicRangeColorComponentValue ?? 0,
                                    screen?.maximumReferenceExtendedDynamicRangeColorComponentValue ?? 0)
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }
}
