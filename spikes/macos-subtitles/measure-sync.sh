#!/bin/sh
# Sync measurement: plays the frame-counter video with a frame-counter subtitle, screenshots the video rect repeatedly and
# reads both counters (SubTool probe-sync). Usage: measure-sync.sh <out dir> <seconds to sample> <app args...>
# The app logs the video rect (SYNC-RECT) and ACTION lines with absolute times; each sample's file name carries its time.
set -eu
OUT=$1; DUR=$2; shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
pkill -x SubSpike 2>/dev/null || true
sleep 1
open -n --stdout "$OUT/app.log" --stderr "$OUT/app.err" "$HERE/.build/CurrentSubtitleSpike.app" --args "$@"
sleep 3
RECT=$(grep SYNC-RECT "$OUT/app.log" | tail -1 | awk '{print $3}')
echo "rect $RECT"
END=$(( $(date +%s) + DUR ))
i=0
while [ "$(date +%s)" -lt "$END" ]; do
  T=$(python3 -c 'import time; print("%.3f" % (time.time() - 978307200))')  # CFAbsoluteTime
  screencapture -x -R"$RECT" "$OUT/s_$(printf %04d $i)_$T.png"
  i=$((i + 1))
done
pkill -x SubSpike || true
"$HERE/.build/release/SubTool" probe-sync "$OUT"/s_*.png > "$OUT/probe.txt"
tail -1 "$OUT/probe.txt"
