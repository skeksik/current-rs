import AVFoundation
import AppKit
import SwiftUI
import UniformTypeIdentifiers

@main
struct VideoSpikeApp: App {
    @State private var model = PlayerModel()
    @State private var jellyfin = JellyfinSession()
    @State private var sheet: Sheet?
    @State private var keys = KeyMonitor()

    enum Sheet: String, Identifiable {
        case location, jellyfin
        var id: String { rawValue }
    }

    var body: some Scene {
        Window("Current Video Spike", id: "player") {
            PlayerView(model: model)
                .frame(minWidth: 480, minHeight: 270)
                .sheet(item: $sheet) { sheet in
                    switch sheet {
                    case .location: LocationSheet(model: model)
                    case .jellyfin: JellyfinSheet(session: jellyfin, model: model)
                    }
                }
                .onAppear {
                    keys.install(model: model)
                    // `-media <path or URL>` (repeatable). Bare arguments would reach AppKit as documents to open, which
                    // suppresses the launch window.
                    let args = CommandLine.arguments
                    let files = args.indices.filter { args[$0] == "-media" && $0 + 1 < args.count }.map { args[$0 + 1] }
                    let urls = files.map { $0.contains("://") ? URL(string: $0)! : URL(fileURLWithPath: $0) }
                    if !urls.isEmpty { model.open(urls) }
                }
        }
        .defaultSize(width: 960, height: 540)
        .windowToolbarStyle(.unifiedCompact)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Open…") { openPanel() }.keyboardShortcut("o")
                Button("Open Location…") { sheet = .location }.keyboardShortcut("l")
                Button("Jellyfin…") { sheet = .jellyfin }.keyboardShortcut("j")
            }
            CommandMenu("Playback") {
                // Bare keys are handled by KeyMonitor so they never reach a focused text field; the menu shows them.
                Button(model.isPlaying ? "Pause" : "Play") { model.togglePlay() }
                    .keyboardShortcut(.space, modifiers: [])
                Button("Skip Back 10 Seconds") { model.skip(-10) }.keyboardShortcut(.leftArrow, modifiers: [])
                Button("Skip Forward 10 Seconds") { model.skip(10) }.keyboardShortcut(.rightArrow, modifiers: [])
                Divider()
                Button("Previous Item") { model.previous() }.keyboardShortcut(.leftArrow, modifiers: .command)
                Button("Next Item") { model.next() }.keyboardShortcut(.rightArrow, modifiers: .command)
                Divider()
                Button(model.pipActive ? "Exit Picture in Picture" : "Enter Picture in Picture") { model.togglePictureInPicture() }
                    .disabled(!model.pipPossible)
                Divider()
                Button(model.showInfo ? "Hide Info" : "Show Info") { model.showInfo.toggle() }.keyboardShortcut("i")
            }
            CommandMenu("Audio") {
                Picker("Audio Track", selection: Binding(get: { model.selectedAudio }, set: { model.selectAudio($0) })) {
                    ForEach(model.audioOptions) { Text($0.name).tag(Optional($0.id)) }
                }
                .pickerStyle(.inline)
                .disabled(model.audioOptions.isEmpty)
            }
            CommandMenu("Subtitles") {
                Picker("Subtitles", selection: Binding(get: { model.selectedSubtitle }, set: { model.selectSubtitle($0) })) {
                    Text("Off").tag(Int?.none)
                    ForEach(model.subtitleOptions) { Text($0.name).tag(Optional($0.id)) }
                }
                .pickerStyle(.inline)
                .disabled(model.subtitleOptions.isEmpty)
            }
        }
    }

    private func openPanel() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.allowedContentTypes = [.movie, .audiovisualContent, .data]
        if panel.runModal() == .OK { model.open(panel.urls) }
    }
}

/// Space, arrows and Esc on the player window. Text fields (sheets) keep their keys.
@MainActor
final class KeyMonitor {
    private var token: Any?

    func install(model: PlayerModel) {
        guard token == nil else { return }
        token = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak model] event in
            let keyCode = event.keyCode
            let eligible = event.modifierFlags.intersection([.command, .option, .control]).isEmpty
            let handled = MainActor.assumeIsolated { () -> Bool in
                guard eligible, let model, let window = NSApp.keyWindow, window.attachedSheet == nil, window.sheetParent == nil,
                    !(window.firstResponder is NSText)
                else { return false }
                switch keyCode {
                case 49: model.togglePlay()  // Space
                case 123: model.skip(-10)  // Left arrow
                case 124: model.skip(10)  // Right arrow
                case 53: return model.exitFullScreen()  // Esc leaves full screen only
                default: return false
                }
                return true
            }
            return handled ? nil : event
        }
    }
}

struct LocationSheet: View {
    let model: PlayerModel
    @State private var text = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Open Location").font(.headline)
            TextField("URL", text: $text, prompt: Text("http://127.0.0.1:8000/hls/master.m3u8"))
                .frame(width: 460)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Open") {
                    if let url = URL(string: text) { model.open([url]) }
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }
}
