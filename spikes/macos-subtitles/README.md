# D7 spike: macOS subtitle renderer over AVPlayer (throwaway)

Our own subtitle layer above an AVPlayerLayer: ASS through libass, PGS and VobSub bitmaps. Results:
`docs/notes/macos-subtitles.md`, decision: `docs/decisions/0010-macos-subtitle-renderer.md`. This folder is deleted before the
PR merges. Needs Homebrew `libass` and `ffmpeg` (local tools for the spike, never shipped).

```
./build.sh                                       # swift build -c release + ad-hoc signed .build/CurrentSubtitleSpike.app
./make-media.sh <empty folder> [font.ttf]        # synthetic test media (FFmpeg + SubTool); the font is for the embedded/attached cases
open -n .build/CurrentSubtitleSpike.app --args -media <video> -sub <file.ass|.sup|.mks>... [-font <ttf>]... \
    [-clock periodic|link|output] [-provider coretext|none] [-pgs libav|swift] [-seek s] [-pinbar] [-info] [-actions "4:seek=40,8:rate=2"]
./measure-sync.sh <out> <seconds> <app args>     # screenshots of the video rect, frame counters read by SubTool probe-sync
./measure-cost.sh <out> <warm-up> <seconds> <app args>   # app CPU (its own stats) and GPU utilisation (IOAccelerator)
.build/release/SubTool                           # generators, render-ass, bench-ass, decode, compare-pgs, probe-sync
```

Keys: Space, ←/→ ±10 s, [ ] rate, C clock, W subtitle white level (1.0 / 0.8 / 2.0 EDR), B pin the bar, P PiP, F full screen,
⌘I info.
