#!/bin/sh
# Builds current-ffi for Apple silicon macOS, generates the Swift bindings and makes the XCFramework that Package.swift uses
# (docs/notes/uniffi-bridge.md). Then: swift build --package-path bindings/swift (and swift test).
# Run from anywhere; needs Rust (rustup) and Xcode.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/bindings/swift/Generated"
target=aarch64-apple-darwin
lib="$root/target/$target/release"
# The same minimum as Package.swift's platforms. Without it, C code in dependencies (ring) is built for the runner's macOS, and the
# linker warns that those objects are newer than the package.
export MACOSX_DEPLOYMENT_TARGET=14.0

cd "$root"
rustup target add "$target"
cargo build --release --locked -p current-ffi --target "$target"
# The Jellyfin stand-in the flow tests run against (tools/jellyfin-stand-in), for the host: target/release. Test code only.
cargo build --release --locked -p jellyfin-stand-in
# The system libraries the static library needs; Package.swift links them.
cargo rustc --release --locked -p current-ffi --target "$target" --crate-type staticlib -- --print native-static-libs 2>&1 \
    | grep 'native-static-libs' || true

rm -rf "$out"
mkdir -p "$out/raw" "$out/include" "$out/swift"
cargo run --release --locked -p uniffi-bindgen -- \
    generate --library "$lib/libcurrent_ffi.dylib" --language swift --out-dir "$out/raw"
cp "$out/raw/current_ffiFFI.h" "$out/include/"
cp "$out/raw/current_ffiFFI.modulemap" "$out/include/module.modulemap"
cp "$out/raw/current_ffi.swift" "$out/swift/"

xcodebuild -create-xcframework \
    -library "$lib/libcurrent_ffi.a" -headers "$out/include" \
    -output "$out/CurrentFFI.xcframework"
echo "Made $out/CurrentFFI.xcframework"
