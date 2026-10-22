#!/bin/sh
# D1 spike: builds the Rust static library, generates the Swift bindings, wraps the library in an XCFramework and
# builds the SwiftUI app (docs/notes/uniffi-bridge.md). Needs: the Rust toolchain (rust-toolchain.toml), Xcode 26.
# Usage: sh spikes/uniffi-bridge/macos/build.sh   then   swift run --package-path spikes/uniffi-bridge/macos
set -eu
here=$(cd "$(dirname "$0")" && pwd)
spike=$(dirname "$here")
gen="$here/Generated"
target=aarch64-apple-darwin

cd "$spike"
rustup target add "$target"
cargo build --release --locked -p bridge-spike --target "$target"
lib="$spike/target/$target/release"

# UniFFI's own generator, built from the spike workspace at the same uniffi version as the library.
rm -rf "$gen"
cargo run --release --locked -p uniffi-bindgen -- \
    generate --library "$lib/libbridge_spike.dylib" --language swift --out-dir "$gen/raw"
mkdir -p "$gen/include" "$gen/swift"
cp "$gen/raw/bridge_spikeFFI.h" "$gen/include/"
cp "$gen/raw/bridge_spikeFFI.modulemap" "$gen/include/module.modulemap"
cp "$gen/raw/bridge_spike.swift" "$gen/swift/"

xcodebuild -create-xcframework -library "$lib/libbridge_spike.a" -headers "$gen/include" \
    -output "$gen/BridgeSpikeFFI.xcframework"

swift --version
swift build -c release --package-path "$here"
echo "App: swift run -c release --package-path $here"
