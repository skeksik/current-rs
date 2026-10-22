import AppKit
import SubKit
import SwiftUI

/// Arguments: -media <file or URL> [-sub <file>]... [-font <file>]... [-provider coretext|none] [-pgs libav|swift]
/// [-clock periodic|link|output] [-rate 2] [-seek 30] [-info] [-pinbar]
/// [-actions "5:seek=40,10:rate=2,15:pause,18:play,20:clock=link"] (seconds after launch)
@main
struct SubSpikeApp: App {
    @State private var model = PlayerModel()
    @State private var keys = KeyMonitor()

    var body: some Scene {
        Window("Current Subtitle Spike", id: "player") {
            PlayerView(model: model)
                .frame(minWidth: 480, minHeight: 270)
                .onAppear {
                    keys.install(model: model)
                    let a = CommandLine.arguments
                    func values(_ flag: String) -> [String] { a.indices.filter { a[$0] == flag && $0 + 1 < a.count }.map { a[$0 + 1] } }
                    func url(_ s: String) -> URL { s.contains("://") ? URL(string: s)! : URL(fileURLWithPath: s) }
                    if let clock = values("-clock").first.flatMap(ClockMode.init(rawValue:)) { model.setClock(clock) }
                    if let rate = values("-rate").first.flatMap(Float.init) { model.rate = rate; model.player.defaultRate = rate }
                    model.showInfo = a.contains("-info")
                    model.pinControls = a.contains("-pinbar")
                    guard let media = values("-media").first else { return }
                    model.open(video: url(media), subtitles: values("-sub").map(url), fonts: values("-font").map(url),
                               provider: AssRenderer.FontProvider(rawValue: values("-provider").first ?? "coretext") ?? .coretext,
                               pgsParser: values("-pgs").first ?? "libav")
                    if let seek = values("-seek").first.flatMap(Double.init) { model.seek(to: seek) }
                    if let script = values("-actions").first { model.runActions(script) }
                }
        }
        .defaultSize(width: 1280, height: 720)
        .windowToolbarStyle(.unifiedCompact)
        .commands {
            CommandMenu("Playback") {
                Button(model.isPlaying ? "Pause" : "Play") { model.togglePlay() }.keyboardShortcut(.space, modifiers: [])
                Button("Skip Back 10 Seconds") { model.skip(-10) }.keyboardShortcut(.leftArrow, modifiers: [])
                Button("Skip Forward 10 Seconds") { model.skip(10) }.keyboardShortcut(.rightArrow, modifiers: [])
                Divider()
                Button(model.pipActive ? "Exit Picture in Picture" : "Enter Picture in Picture") { model.togglePictureInPicture() }
                    .disabled(!model.pipPossible)
                Button(model.showInfo ? "Hide Info" : "Show Info") { model.showInfo.toggle() }.keyboardShortcut("i")
            }
        }
    }
}

/// Bare keys on the player window.
@MainActor
final class KeyMonitor {
    private var token: Any?

    func install(model: PlayerModel) {
        guard token == nil else { return }
        token = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak model] event in
            let keyCode = event.keyCode
            let chars = event.charactersIgnoringModifiers ?? ""
            let eligible = event.modifierFlags.intersection([.command, .option, .control]).isEmpty
            let handled = MainActor.assumeIsolated { () -> Bool in
                guard eligible, let model else { return false }
                switch (keyCode, chars) {
                case (49, _): model.togglePlay()
                case (123, _): model.skip(-10)
                case (124, _): model.skip(10)
                case (53, _): return model.exitFullScreen()
                case (_, "["): model.changeRate(up: false)
                case (_, "]"): model.changeRate(up: true)
                case (_, "c"): model.cycleClock()
                case (_, "w"): model.cycleWhiteLevel()
                case (_, "b"): model.pinControls.toggle(); model.poke()
                case (_, "p"): model.togglePictureInPicture()
                case (_, "r"): model.printSyncRect()
                case (_, "f"): model.toggleFullScreen()
                default: return false
                }
                return true
            }
            return handled ? nil : event
        }
    }
}
