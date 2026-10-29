# The stand-in's media

`native.mp4`: what the stand-in serves as the direct-play stream of `NATIVE_ITEM` (`src/lib.rs`), for the Windows app's smoke run
to open a playback in Windows' own player (ADR 0025, W4). Our own synthetic content: FFmpeg's `testsrc` picture and a 440 Hz `sine`
tone, 2 s, 320×180 at 24 fps, H.264 Main and AAC-LC stereo at 48 kHz in MP4 with the index first (27 681 bytes). FFmpeg is a one-off
maker here, not a repository tool: nothing builds or runs it.

Made on 2026-10-29 with FFmpeg 6.1.1 (Ubuntu's `6.1.1-3ubuntu5`, its libx264 and native AAC encoders):

```
ffmpeg -f lavfi -i testsrc=size=320x180:rate=24:duration=2 -f lavfi -i sine=frequency=440:sample_rate=48000:duration=2 \
  -map 0:v -map 1:a -c:v libx264 -profile:v main -pix_fmt yuv420p -preset medium -crf 30 -g 24 \
  -c:a aac -profile:a aac_low -b:a 64k -ac 2 -map_metadata -1 -fflags +bitexact -flags:v +bitexact -flags:a +bitexact \
  -movflags +faststart native.mp4
```

Another FFmpeg or x264 version may write other bytes; the stand-in only needs a file of that shape.
