// swift-tools-version: 6.2
// D7 spike prototype: our own subtitle renderer over AVPlayer (libass for ASS, PGS/VobSub bitmaps). Throwaway; deleted before
// the PR merges. Links the Homebrew libass and FFmpeg dylibs from /opt/homebrew (local tools for the spike, never shipped).
import PackageDescription

let brewInclude: [SwiftSetting] = [.unsafeFlags(["-Xcc", "-I/opt/homebrew/include"])]
let brewLib: [LinkerSetting] = [.unsafeFlags(["-L/opt/homebrew/lib"])]

let package = Package(
    name: "SubSpike",
    platforms: [.macOS("26.0")],
    targets: [
        .systemLibrary(name: "CLibass", path: "Sources/CLibass"),
        .systemLibrary(name: "CFFmpeg", path: "Sources/CFFmpeg"),
        // Renderers and test-media generators shared by the app and the command-line tool.
        .target(
            name: "SubKit", dependencies: ["CLibass", "CFFmpeg"], path: "Sources/SubKit",
            swiftSettings: brewInclude, linkerSettings: brewLib),
        // The player app. build.sh wraps the binary into an ad-hoc signed .app bundle.
        .executableTarget(
            name: "SubSpike", dependencies: ["SubKit"], path: "Sources/SubSpike",
            swiftSettings: brewInclude, linkerSettings: brewLib),
        // Test-media generators (PGS, ASS), render benchmarks and the sync probe.
        .executableTarget(
            name: "SubTool", dependencies: ["SubKit"], path: "Sources/SubTool",
            swiftSettings: brewInclude, linkerSettings: brewLib),
    ]
)
