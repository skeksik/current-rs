#!/bin/sh
# Makes the synthetic test media for the D7 spike with a local FFmpeg (a local tool, never shipped) and SubTool.
# Usage: make-media.sh <output folder> [font file for the embedded/attached font cases]
set -eu
OUT=${1:?output folder}
FONT=${2:-}
HERE=$(cd "$(dirname "$0")" && pwd)
TOOL="$HERE/.build/release/SubTool"
mkdir -p "$OUT/hls"
cd "$OUT"
FF="ffmpeg -hide_banner -loglevel error -y"
T=60
A="-f lavfi -i sine=frequency=440:sample_rate=48000:duration=$T"
H264="-c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50"
# Frame counter: frame N as 16 blocks (bit k at x = k/16 of the width), luma 235 for 1, 64 for 0, 1/27 of the frame high, 4 block heights from the top (clear of the title bar).
# SubTool draws the subtitle's own counter in the same layout one block-height lower.
counter() { # $1 width, $2 height
  echo "color=c=black:s=16x1:r=25:d=$T,format=gray,geq=lum='if(mod(floor(N/pow(2,X)),2),235,64)',scale=$1:$(($2 / 27)):flags=neighbor"
}

echo "sync video: 1080p25 H.264 with the frame counter"
$FF -f lavfi -i testsrc2=size=1920x1080:rate=25:duration=$T $A -f lavfi -i "$(counter 1920 1080)" \
  -filter_complex "[0:v][2:v]overlay=0:4*1080/27:format=auto,format=yuv420p[v]" -map "[v]" -map 1:a $H264 -c:a aac sync_1080p25.mp4

echo "HLS fMP4 of the sync video (what a server remux produces), served from 127.0.0.1"
$FF -i sync_1080p25.mp4 -c copy -f hls -hls_segment_type fmp4 -hls_time 4 -hls_playlist_type vod \
  -hls_fmp4_init_filename init.mp4 -hls_segment_filename 'hls/seg_%03d.m4s' hls/sync.m3u8

echo "HDR10 1080p25 (PQ ramp on top, test pattern below)"
HDR10="-c:v libx265 -preset ultrafast -pix_fmt yuv420p10le -color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc
 -x265-params log-level=error:hdr10=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400"
$FF -f lavfi -i "nullsrc=size=1920x540:rate=25:duration=30,format=yuv420p10le,geq=lum='64+X/W*876':cb=512:cr=512" \
  -f lavfi -i testsrc2=size=1920x540:rate=25:duration=30 -f lavfi -i sine=frequency=440:sample_rate=48000:duration=30 \
  -filter_complex "[0:v][1:v]vstack,format=yuv420p10le[v]" -map "[v]" -map 2:a $HDR10 -tag:v hvc1 -c:a aac hdr10_1080p25.mp4

echo "busy-script videos: 1080p H.264 and 2160p HEVC, 30 s"
$FF -f lavfi -i testsrc2=size=1920x1080:rate=25:duration=30 -f lavfi -i sine=duration=30 $H264 -c:a aac busy_1080p25.mp4
$FF -f lavfi -i testsrc2=size=3840x2160:rate=25:duration=30 -f lavfi -i sine=duration=30 \
  -c:v libx265 -preset ultrafast -pix_fmt yuv420p -x265-params log-level=error -tag:v hvc1 -c:a aac busy_2160p25.mp4

echo "subtitles"
"$TOOL" ass-sync sync.ass 1920 1080 25 $T
"$TOOL" sup-sync sync.sup 1920 1080 25 $T
"$TOOL" sup-dialog dialog.sup 1920 1080
"$TOOL" sup-dialog dialog576.sup 720 576
"$TOOL" ass-busy busy.ass 30
cp "$HERE/styled.ass" styled.ass
# VobSub: FFmpeg converts the 720x576 PGS to dvd_subtitle (bitmap to bitmap; 4 colours) in Matroska, the form Jellyfin
# extracts VobSub to (.mks). -copyts keeps the original times; -fix_sub_duration ends each event at the next PGS display set.
$FF -fix_sub_duration -i dialog576.sup -copyts -c:s dvdsub -f matroska dialog_vobsub.mks
# A VobSub frame counter too, for sync (its own 720x576 canvas).
"$TOOL" sup-sync sync576.sup 720 576 25 $T
$FF -fix_sub_duration -i sync576.sup -copyts -c:s dvdsub -f matroska sync_vobsub.mks

if [ -n "$FONT" ]; then
  echo "fonts: embedded in the script, and as an MKV attachment extracted the way Jellyfin does"
  "$TOOL" embed-font styled.ass "$FONT" styled_embedded.ass
  $FF -i sync_1080p25.mp4 -i styled.ass -map 0:v -map 0:a -map 1 -c copy -attach "$FONT" \
    -metadata:s:t mimetype=font/ttf styled_attached.mkv
  # Jellyfin's subtitle extraction (-c:s copy, SubtitleEncoder.cs) and attachment extraction (-dump_attachment).
  $FF -i styled_attached.mkv -map 0:2 -an -vn -c:s copy extracted.ass
  rm -f attachment.ttf
  ffmpeg -hide_banner -loglevel error -dump_attachment:t:0 attachment.ttf -i styled_attached.mkv -t 0 -f null - || true
fi
ls -la . hls
