#!/bin/sh
# Makes the synthetic test files for the D3 format matrix with a local FFmpeg (a local tool, never shipped).
# Usage: make-media.sh <empty output folder>
set -eu
OUT=${1:?output folder}
mkdir -p "$OUT/hls"
cd "$OUT"
FF="ffmpeg -hide_banner -loglevel error -y"
T=20
V="-f lavfi -i testsrc2=size=1920x1080:rate=24:duration=$T"
A="-f lavfi -i sine=frequency=440:sample_rate=48000:duration=$T"
A6="-f lavfi -i sine=frequency=440:sample_rate=48000:duration=$T -filter_complex [1:a]pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0[a]"
H264="-c:v libx264 -preset veryfast -pix_fmt yuv420p"
HEVC="-c:v libx265 -preset ultrafast -pix_fmt yuv420p -x265-params log-level=error"

# HDR10 pattern: top half a PQ ramp (0 to 10000 nits left to right), bottom half moving test content, tagged BT.2020/PQ.
HDRSRC="-f lavfi -i nullsrc=size=1920x540:rate=24:duration=$T,format=yuv420p10le,geq=lum='64+X/W*876':cb=512:cr=512
 -f lavfi -i testsrc2=size=1920x540:rate=24:duration=$T"
HDRFILTER="[0:v][1:v]vstack,format=yuv420p10le[v]"
HDR10="-c:v libx265 -preset ultrafast -pix_fmt yuv420p10le -color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc
 -x265-params log-level=error:hdr10=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400"

echo "video: containers and codecs"
$FF $V $A $H264 -c:a aac h264_aac.mp4
$FF $V $A $HEVC -tag:v hvc1 -c:a aac hevc_hvc1_aac.mp4
$FF $V $A $HEVC -tag:v hev1 -c:a aac hevc_hev1_aac.mp4
$FF $V $A $H264 -c:a aac h264_aac.mkv
$FF $V $A $HEVC -c:a aac hevc_aac.mkv
$FF $V $A -c:v libsvtav1 -preset 12 -pix_fmt yuv420p10le -c:a aac av1_aac.mp4
$FF $V $A -c:v libsvtav1 -preset 12 -pix_fmt yuv420p10le -c:a aac av1_aac.mkv
$FF $V $A -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 2M -c:a libopus vp9_opus.webm
$FF $V $A -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 2M -c:a aac vp9_aac.mp4
$FF $V $A $H264 -c:a aac h264_aac.ts

echo "video: HDR10"
$FF $HDRSRC $A -filter_complex "$HDRFILTER" -map "[v]" -map 2:a $HDR10 -tag:v hvc1 -c:a aac hevc_hdr10.mp4
$FF $HDRSRC $A -filter_complex "$HDRFILTER" -map "[v]" -map 2:a $HDR10 -c:a aac hevc_hdr10.mkv

echo "audio codecs (H.264 video)"
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a ac3 -b:a 448k audio_ac3.mp4
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a eac3 -b:a 640k audio_eac3.mp4
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a truehd -strict -2 audio_truehd.mp4 || echo "  (mp4 refused TrueHD)"
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a dca -strict -2 audio_dts.mp4 || echo "  (mp4 refused DTS)"
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a flac audio_flac.mp4
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a libopus audio_opus.mp4
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a alac audio_alac.mp4
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a pcm_s24le audio_lpcm.mov
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a ac3 audio_ac3.mkv
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a eac3 audio_eac3.mkv
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a truehd -strict -2 audio_truehd.mkv
$FF $V $A6 -map 0:v -map "[a]" $H264 -c:a dca -strict -2 audio_dts.mkv
# Two audio tracks with languages, for the audio menu.
$FF $V $A $A6 -map 0:v -map 1:a -map "[a]" $H264 -c:a:0 aac -c:a:1 ac3 \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title=Stereo -metadata:s:a:1 language=fra -metadata:s:a:1 title=Surround \
  audio_two_tracks.mp4 2>/dev/null || \
$FF $V $A -f lavfi -i sine=frequency=660:sample_rate=48000:duration=$T -map 0:v -map 1:a -map 2:a $H264 -c:a:0 aac -c:a:1 ac3 \
  -metadata:s:a:0 language=eng -metadata:s:a:1 language=fra audio_two_tracks.mp4

echo "subtitles"
cat > subs.srt <<'EOF'
1
00:00:01,000 --> 00:00:05,000
Plain subtitle line one.
<i>Italic</i> and <b>bold</b>.

2
00:00:06,000 --> 00:00:10,000
Second cue, at six seconds.

3
00:00:11,000 --> 00:00:19,000
Third cue, until nineteen seconds.
EOF
cat > subs_fr.srt <<'EOF'
1
00:00:01,000 --> 00:00:09,000
Sous-titre en français.

2
00:00:10,000 --> 00:00:19,000
Deuxième réplique.
EOF
cat > styled.ass <<'EOF'
[Script Info]
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Helvetica,64,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,3,1,2,40,40,60,1
Style: Sign,Georgia,72,&H0000FFFF,&H000000FF,&H00800000,&H00000000,1,0,0,0,100,100,0,0,1,4,0,8,40,40,80,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:06.00,Default,,0,0,0,,Bottom centre, white with outline
Dialogue: 0,0:00:01.00,0:00:06.00,Sign,,0,0,0,,{\pos(480,300)\frz-12}Yellow rotated sign, top left
Dialogue: 0,0:00:07.00,0:00:12.00,Default,,0,0,0,,{\c&H0000FF&}Red{\c&HFFFFFF&} word and {\fs96}big{\fs64} text
Dialogue: 0,0:00:13.00,0:00:19.00,Default,,0,0,0,,{\k50}Ka{\k50}ra{\k50}o{\k50}ke {\fad(500,500)}fade
EOF
$FF $V $A -i subs.srt -i subs_fr.srt -map 0:v -map 1:a -map 2 -map 3 $H264 -c:a aac -c:s mov_text \
  -metadata:s:s:0 language=eng -metadata:s:s:1 language=fra subs_tx3g.mp4
$FF $V $A -i subs.srt -map 0:v -map 1:a -map 2 $H264 -c:a aac -c:s webvtt subs_wvtt.mp4 || { rm -f subs_wvtt.mp4; echo "  (mp4 refused WebVTT)"; }
$FF $V $A -i subs.srt -i styled.ass -map 0:v -map 1:a -map 2 -map 3 $H264 -c:a aac -c:s:0 srt -c:s:1 ass subs_srt_ass.mkv
# Image subtitles (PGS, VobSub): FFmpeg cannot encode text to bitmap subtitles, so none are made. They are tested only by
# reasoning: AVFoundation opens no container that carries them (see the notes).

echo "HLS: fMP4 HEVC HDR10 + AAC + WebVTT (what a server remux would produce)"
$FF -i hevc_hdr10.mp4 -map 0:v -c copy -tag:v hvc1 -f hls -hls_segment_type fmp4 -hls_time 4 -hls_playlist_type vod \
  -hls_fmp4_init_filename video_init.mp4 -hls_segment_filename 'hls/video_%03d.m4s' hls/video.m3u8
$FF -i hevc_hdr10.mp4 -map 0:a -c copy -f hls -hls_segment_type fmp4 -hls_time 4 -hls_playlist_type vod \
  -hls_fmp4_init_filename audio_init.mp4 -hls_segment_filename 'hls/audio_%03d.m4s' hls/audio.m3u8
$FF -i audio_ac3.mp4 -map 0:a -c copy -f hls -hls_segment_type fmp4 -hls_time 4 -hls_playlist_type vod \
  -hls_fmp4_init_filename ac3_init.mp4 -hls_segment_filename 'hls/ac3_%03d.m4s' hls/ac3.m3u8
for lang in en fr; do
  src=subs.srt; [ $lang = fr ] && src=subs_fr.srt
  { printf 'WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n'; $FF -i $src -f webvtt - | sed 1,2d; } > hls/subs_$lang.vtt
  printf '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:%s\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:%s.0,\nsubs_%s.vtt\n#EXT-X-ENDLIST\n' $T $T $lang > hls/subs_$lang.m3u8
done
# Styled WebVTT: cue settings and tags are what HLS subtitles can carry; ASS converted to WebVTT loses the rest.
cat > hls/subs_styled.vtt <<'EOF'
WEBVTT
X-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000

00:00:01.000 --> 00:00:06.000 line:10% position:25% align:start
Top left via cue settings, <i>italic</i>, <b>bold</b>, <u>underline</u>

00:00:07.000 --> 00:00:12.000 line:50% align:center size:50%
Middle of the screen, half width box that wraps onto a second line

00:00:13.000 --> 00:00:19.000
<c.yellow>Class span (yellow if supported)</c> and <ruby>漢字<rt>かんじ</rt></ruby>
EOF
printf '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:%s\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:%s.0,\nsubs_styled.vtt\n#EXT-X-ENDLIST\n' $T $T > hls/subs_styled.m3u8
{ printf 'WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n'; $FF -i styled.ass -f webvtt - | sed 1,2d; } > hls/subs_ass.vtt
printf '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:%s\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:%s.0,\nsubs_ass.vtt\n#EXT-X-ENDLIST\n' $T $T > hls/subs_ass.m3u8
cat > hls/master.m3u8 <<'EOF'
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="English Stereo",LANGUAGE="en",DEFAULT=YES,AUTOSELECT=YES,CHANNELS="1",URI="audio.m3u8"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="Français 5.1 (AC-3)",LANGUAGE="fr",DEFAULT=NO,AUTOSELECT=YES,CHANNELS="6",URI="ac3.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="English",LANGUAGE="en",DEFAULT=NO,AUTOSELECT=YES,URI="subs_en.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="Français",LANGUAGE="fr",DEFAULT=NO,AUTOSELECT=YES,URI="subs_fr.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="Styled WebVTT",LANGUAGE="und",DEFAULT=NO,AUTOSELECT=NO,URI="subs_styled.m3u8"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="ASS converted by FFmpeg",LANGUAGE="und",DEFAULT=NO,AUTOSELECT=NO,URI="subs_ass.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=8000000,CODECS="hvc1.2.4.L120.B0,mp4a.40.2,ac-3",RESOLUTION=1920x1080,FRAME-RATE=24.000,VIDEO-RANGE=PQ,AUDIO="aud",SUBTITLES="subs"
video.m3u8
EOF

echo "HLS: Apple's public examples, narrowed to one video range each"
APPLE=https://devstreaming-cdn.apple.com/videos/streaming/examples/adv_dv_atmos
curl -sfL "$APPLE/main.m3u8" > hls/apple_main.m3u8
for range in dvh1 hdr10 sdr; do
  awk -v range=$range -v base="$APPLE/" '
    /^#EXT-X-MEDIA/ { sub(/URI="/, "URI=\"" base); print; next }
    /^#EXT-X-I-FRAME/ { next }
    /^#EXT-X-STREAM-INF/ { keep = (range == "dvh1" && /dvh1/) || (range == "hdr10" && /VIDEO-RANGE=PQ/ && /hvc1/) ||
                                  (range == "sdr" && /VIDEO-RANGE=SDR/ && /hvc1/); if (keep) print; next }
    /^#/ { print; next }
    NF { if (keep) print base $0; keep = 0 }
  ' hls/apple_main.m3u8 > hls/apple_$range.m3u8
done
ls -la . hls | head -80
