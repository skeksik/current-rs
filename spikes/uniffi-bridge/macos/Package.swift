// swift-tools-version: 6.2
// D1 spike (docs/notes/uniffi-bridge.md). Swift 6 language mode, so strict concurrency checking is on for every target.
import PackageDescription

let package = Package(
    name: "BridgeSpike",
    platforms: [.macOS(.v14)],  // @Observable
    targets: [
        // libbridge_spike.a with the C header and module map from UniFFI; built by the steps in the notes.
        .binaryTarget(name: "BridgeSpikeFFI", path: "Generated/BridgeSpikeFFI.xcframework"),
        // The generated bridge_spike.swift, compiled as its own module so the app sees only its public API.
        .target(name: "BridgeSpikeBindings", dependencies: ["BridgeSpikeFFI"], path: "Generated/swift"),
        .executableTarget(name: "BridgeSpikeApp", dependencies: ["BridgeSpikeBindings"]),
    ]
)
