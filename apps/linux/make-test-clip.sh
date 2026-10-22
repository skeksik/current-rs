#!/bin/sh
# Writes a synthetic 30-second test clip (ffmpeg's testsrc2 pattern and a sine tone) next to this script. Local tool only: the clip
# and the distro ffmpeg (a GPL build with libx264) are never shipped or checked in.
set -eu
out="$(dirname "$0")/test-clip.mkv"
ffmpeg -hide_banner -loglevel error -y \
  -f lavfi -i testsrc2=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=48000 \
  -t 30 -c:v libx264 -pix_fmt yuv420p -preset veryfast -c:a aac -b:a 128k "$out"
echo "$out"
