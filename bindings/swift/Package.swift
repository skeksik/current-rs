// swift-tools-version: 6.2
// The Swift side of the bridge (ADR 0004): the XCFramework and generated bindings that build.sh makes, plus hand-written adapters.
// Run build.sh first: Generated/ is not checked in.

import PackageDescription

let package = Package(
    name: "CurrentBindings",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CurrentCore", targets: ["CurrentCore"]),
    ],
    targets: [
        // libcurrent_ffi.a with the C header and module map (module current_ffiFFI).
        .binaryTarget(name: "CurrentFFI", path: "Generated/CurrentFFI.xcframework"),
        // The Swift that uniffi-bindgen generates.
        // What the static library needs on macOS (through current-net's TLS and proxy support), from rustc's native-static-libs,
        // which build.sh prints: -framework Security -framework SystemConfiguration -framework CoreFoundation -liconv (plus libSystem).
        .target(
            name: "CurrentFFIBindings",
            dependencies: ["CurrentFFI"],
            path: "Generated/swift",
            linkerSettings: [
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedLibrary("iconv"),
            ]
        ),
        // Adapters: main-actor models fed by an AsyncStream, and explicit cancellation.
        .target(name: "CurrentCore", dependencies: ["CurrentFFIBindings"]),
        .testTarget(name: "CurrentCoreTests", dependencies: ["CurrentCore"]),
    ],
    swiftLanguageModes: [.v6]
)
