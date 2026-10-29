# The stand-in's media

`native.mp4`: what the stand-in serves as the direct-play stream of `NATIVE_ITEM` (`src/lib.rs`), for the Windows app's smoke run
to open a playback in Windows' own player (ADR 0025, W4). Our own synthetic content: FFmpeg's `testsrc` picture, 8 s, 320×180 at
24 fps, H.264 Main in MP4 with the index first (36 228 bytes). No audio: CI's Windows runner has no audio device, and Media
Foundation fails an audio track there with `MF_E_NO_AUDIO_PLAYBACK_DEVICE` (0xC00D4E84; seen on CI 2026-10-29 with an earlier
H.264 + AAC clip). 8 s leaves the smoke run time to add the item's SubRip file (one cue, 1 s to 2 s) both ways the engine can,
and to seek back to its cue. FFmpeg is a one-off maker here, not a repository tool: nothing builds or runs it.

Made on 2026-10-29 with FFmpeg 6.1.1 (Ubuntu's `6.1.1-3ubuntu5`, its libx264 encoder):

```
ffmpeg -f lavfi -i testsrc=size=320x180:rate=24:duration=8 -c:v libx264 -profile:v main -pix_fmt yuv420p -preset medium -crf 30 \
  -g 24 -map_metadata -1 -fflags +bitexact -flags:v +bitexact -movflags +faststart native.mp4
```

Another FFmpeg or x264 version may write other bytes; the stand-in only needs a file of that shape.
