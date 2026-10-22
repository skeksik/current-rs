# D3 spike: macOS video (throwaway)

AVPlayer under our own SwiftUI controls. Results: `docs/notes/macos-video.md`, decision: `docs/decisions/0008-macos-video.md`.
This folder is deleted before the PR merges.

```
./build.sh                                   # swift build + ad-hoc signed .build/CurrentVideoSpike.app (no team, no signing settings)
./make-media.sh <empty folder>               # synthetic test files; needs a local FFmpeg (a local tool, never shipped)
.build/debug/VideoProbe [--supplemental] [--caps] <file or URL>...   # what AVFoundation opens, decodes and plays
open -n .build/CurrentVideoSpike.app --args -media <file or URL> [-media ...] [-systemSubtitles YES]
```

Serve the HLS folder locally with `python3 -m http.server 8000 --bind 127.0.0.1` from the media folder.
Keys: Space, ←/→ (±10 s), ⌘←/⌘→ (previous/next item), Esc (leave full screen), ⌘I (info: EDR headroom, layer dynamic
range, subtitle renderer, log), ⌘O / ⌘L / ⌘J (open file, location, Jellyfin; Jellyfin credentials stay in memory).
