import AVKit
import SwiftUI

struct PlayerView: View {
    @Bindable var model: PlayerModel

    var body: some View {
        ZStack {
            Color.black
            PlayerLayerView(model: model)
            // While PiP is active, AVPlayerLayer draws the system's own placeholder; we only hide our controls.
            if model.pipActive {
                EmptyView()
            } else if let error = model.errorText {
                ContentUnavailableView("Can't play this item", systemImage: "exclamationmark.triangle", description: Text(error))
            } else if model.player.currentItem == nil {
                ContentUnavailableView("Open a video", systemImage: "play.rectangle", description: Text("File ▸ Open… (⌘O)"))
            }
        }
        .overlay(alignment: .bottom) {
            if let text = model.subtitleText, model.ownsSubtitleRendering, !model.pipActive {
                Text(text)
                    .font(.system(size: 22, weight: .medium))
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(.black.opacity(0.6), in: .rect(cornerRadius: 6))
                    // Above the control bar while it is visible, near the bottom edge otherwise.
                    .padding(.bottom, model.controlsVisible ? 120 : 28)
                    .animation(.easeInOut(duration: 0.25), value: model.controlsVisible)
                    .accessibilityLabel("Subtitle: \(text)")
            }
        }
        .overlay(alignment: .bottom) {
            if model.controlsVisible && !model.pipActive {
                ControlBar(model: model)
                    .padding(.horizontal, 24)
                    .padding(.bottom, 20)
                    .transition(.opacity)
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

/// Our own transport controls on Liquid Glass.
struct ControlBar: View {
    @Bindable var model: PlayerModel

    var body: some View {
        GlassEffectContainer {
            VStack(spacing: 6) {
                HStack(spacing: 10) {
                    Text(Self.format(model.currentTime))
                        .accessibilityHidden(true)
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
                    .accessibilityValue("\(Self.format(model.currentTime)) of \(Self.format(model.duration))")
                    Text("-" + Self.format(max(0, model.duration - model.currentTime)))
                        .accessibilityHidden(true)
                }
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)

                HStack(spacing: 18) {
                    trackMenus
                    Spacer()
                    button("gobackward.10", "Skip back 10 seconds") { model.skip(-10) }
                    button(model.isPlaying ? "pause.fill" : "play.fill", model.isPlaying ? "Pause" : "Play", size: 26) {
                        model.togglePlay()
                    }
                    button("goforward.10", "Skip forward 10 seconds") { model.skip(10) }
                    Spacer()
                    RoutePicker(player: model.player)
                        .frame(width: 24, height: 24)
                        .help("AirPlay")
                    button(model.pipActive ? "pip.exit" : "pip.enter", "Picture in Picture") { model.togglePictureInPicture() }
                        .disabled(!model.pipPossible)
                    button(
                        model.isFullScreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                        model.isFullScreen ? "Exit Full Screen" : "Enter Full Screen"
                    ) { model.toggleFullScreen() }
                }
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            .frame(maxWidth: 720)
            // Dark tinted glass keeps the symbols legible over bright video, as the system players do.
            .glassEffect(.regular.tint(.black.opacity(0.35)), in: .rect(cornerRadius: 22))
        }
        .environment(\.colorScheme, .dark)
    }

    @ViewBuilder private var trackMenus: some View {
        Menu {
            Picker("Audio", selection: Binding(get: { model.selectedAudio }, set: { model.selectAudio($0) })) {
                ForEach(model.audioOptions) { Text($0.name).tag(Optional($0.id)) }
            }
            .pickerStyle(.inline)
        } label: {
            // A Label, not a bare Image: the menu button otherwise announces the symbol's name.
            Label("Audio", systemImage: "speaker.wave.2").labelStyle(.iconOnly)
        }
        .menuIndicator(.hidden)
        .fixedSize()
        .disabled(model.audioOptions.isEmpty)
        .help("Audio")

        Menu {
            Picker("Subtitles", selection: Binding(get: { model.selectedSubtitle }, set: { model.selectSubtitle($0) })) {
                Text("Off").tag(Int?.none)
                ForEach(model.subtitleOptions) { Text($0.name).tag(Optional($0.id)) }
            }
            .pickerStyle(.inline)
        } label: {
            // A Label, not a bare Image: the menu button otherwise announces the symbol's name.
            Label("Subtitles", systemImage: "captions.bubble").labelStyle(.iconOnly)
        }
        .menuIndicator(.hidden)
        .fixedSize()
        .disabled(model.subtitleOptions.isEmpty)
        .help("Subtitles")
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
        return s >= 3600
            ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
            : String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// Diagnostics for the spike: EDR headroom of the window's screen, the layer's dynamic range, and the event log.
struct InfoPanel: View {
    @Bindable var model: PlayerModel
    @State private var screenLine = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(screenLine).font(.caption.monospaced())
            Picker("Layer dynamic range", selection: Binding(get: { model.dynamicRange }, set: { model.setDynamicRange($0) })) {
                Text("automatic").tag(CALayer.DynamicRange.automatic)
                Text("standard").tag(CALayer.DynamicRange.standard)
                Text("constrainedHigh").tag(CALayer.DynamicRange.constrainedHigh)
                Text("high").tag(CALayer.DynamicRange.high)
            }
            .pickerStyle(.segmented)
            .frame(maxWidth: 460)
            Toggle("Draw subtitles ourselves (legible output)", isOn: Binding(
                get: { model.ownsSubtitleRendering }, set: { model.setOwnsSubtitleRendering($0) }))
            ScrollView {
                Text(model.log.suffix(40).joined(separator: "\n"))
                    .font(.caption2.monospaced())
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(width: 560, height: 220)
        }
        .padding(12)
        .glassEffect(.regular.tint(.black.opacity(0.35)), in: .rect(cornerRadius: 14))
        .environment(\.colorScheme, .dark)
        .task {
            while !Task.isCancelled {
                let screen = NSApp.keyWindow?.screen ?? NSScreen.main
                let line = String(
                    format: "screen %@: EDR now %.2f, potential %.2f, reference %.2f | HDR eligible %@",
                    screen?.localizedName ?? "?",
                    screen?.maximumExtendedDynamicRangeColorComponentValue ?? 0,
                    screen?.maximumPotentialExtendedDynamicRangeColorComponentValue ?? 0,
                    screen?.maximumReferenceExtendedDynamicRangeColorComponentValue ?? 0,
                    AVPlayer.eligibleForHDRPlayback ? "yes" : "no")
                if line != screenLine { model.note(line) }
                screenLine = line
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }
}
