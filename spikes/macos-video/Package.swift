// swift-tools-version: 6.2
// D3 spike prototype: AVPlayer under our own SwiftUI controls on macOS. Throwaway; deleted before the PR merges.
import PackageDescription

let package = Package(
    name: "VideoSpike",
    platforms: [.macOS("26.0")],
    targets: [
        // The player app. build.sh wraps the binary into an ad-hoc signed .app bundle.
        .executableTarget(name: "VideoSpike", path: "Sources/VideoSpike"),
        // A command-line probe: what AVFoundation accepts, decodes and renders, file by file.
        .executableTarget(name: "VideoProbe", path: "Sources/VideoProbe"),
    ]
)
