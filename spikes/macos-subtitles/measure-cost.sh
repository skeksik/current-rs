#!/bin/sh
# Cost of one configuration: the app's own CPU (its [stats] lines, CLOCK_PROCESS_CPUTIME_ID) and the GPU's
# "Device Utilization %" from the IOAccelerator statistics, sampled every second.
# Usage: measure-cost.sh <out dir> <warm-up s> <seconds> <app args...>
set -eu
OUT=$1; WARM=$2; DUR=$3; shift 3
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
pkill -x SubSpike 2>/dev/null || true
sleep 1
open -n --stdout "$OUT/app.log" --stderr "$OUT/app.err" "$HERE/.build/CurrentSubtitleSpike.app" --args "$@"
sleep "$WARM"
: > "$OUT/gpu.txt"
i=0
while [ $i -lt "$DUR" ]; do
  ioreg -r -c IOAccelerator -d 1 | grep -o '"Device Utilization %"=[0-9]*' | head -1 | cut -d= -f2 >> "$OUT/gpu.txt"
  ps -o %cpu= -p "$(pgrep -x SubSpike)" >> "$OUT/cpu_ps.txt"
  sleep 1
  i=$((i + 1))
done
pkill -x SubSpike || true
GPU=$(awk '{s+=$1} END {printf "%.0f", s/NR}' "$OUT/gpu.txt")
PS=$(awk '{s+=$1} END {printf "%.0f", s/NR}' "$OUT/cpu_ps.txt")
STATS=$(grep '\[stats\]' "$OUT/app.log" | tail -n $((DUR / 2)) | sed 's/.*draw avg \([0-9.]*\) ms max \([0-9.]*\) ms | process CPU \([0-9]*\)%.*/\1 \2 \3/' |
  awk '{a+=$1; if ($2>m) m=$2; c+=$3; n++} END {printf "draw avg %.2f ms, max %.2f ms, app CPU %.0f%%", a/n, m, c/n}')
echo "$(basename "$OUT"): $STATS, ps CPU $PS%, GPU $GPU%"
