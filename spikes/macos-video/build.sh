#!/bin/sh
# Builds the spike and wraps it into an ad-hoc signed app bundle in .build/. No team, no signing settings.
set -eu
cd "$(dirname "$0")"
swift build -c debug
APP=.build/CurrentVideoSpike.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp .build/debug/VideoSpike "$APP/Contents/MacOS/VideoSpike"
cp Support/Info.plist "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
echo "built $APP"
