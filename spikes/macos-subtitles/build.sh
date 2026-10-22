#!/bin/sh
# Builds the spike (release, for honest timings) and wraps it into an ad-hoc signed app bundle in .build/.
set -eu
cd "$(dirname "$0")"
swift build -c release
APP=.build/CurrentSubtitleSpike.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp .build/release/SubSpike "$APP/Contents/MacOS/SubSpike"
cp Support/Info.plist "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
echo "built $APP"
