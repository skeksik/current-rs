# Why Windows' player did not start Jellyfin's transcode (W7 §8, part 2)

spike/native-hls-start-time, from `origin/main` at 1bbcc69. Checked on 2026-10-31 on the owner's Windows 11 PC against the owner's
own Jellyfin server, with the same setup as W7 (`docs/notes/windows-native-player-check.md`, "Setup"). Claude drove the app through
screenshots and read the info panel and the native engine's trace lines (OutputDebugString, read by a script outside the repository).
The task: W7 saw every server transcode on Windows' player end in `stream_timeout` after 8 s (three tries out of three), then libmpv
played the same stream; W1 had played Jellyfin's fMP4 HLS natively. Find the difference. The route for server streams stays the
owner's decision (ADR 0025 is unchanged here).

## Answer
**The native engine built every item with `MediaPlaybackItem(source, startTime)`, and such an item counts its position and its
duration from the start time.** The engine's first-segment check (`HlsFirstSegmentDeadline` in `NativeEngine.cs`) ends the wait when
the position moves past the position the wait began at, which is the file's position (the load's start). The item reports a
position counted from 0. For a stream started more than about 8 s into the file, the position cannot pass the start within the 8 s
deadline, so the engine reported `stream_timeout` whether or not Windows' player was playing. W1 started its streams at the start of
the file (start 0), where the two counts agree, which is why W1 did not see it.

Nothing about HLS, fMP4, HEVC, the relay or Jellyfin was the cause. With the start applied as a seek after the file opens (the
proposal below), every server stream tried here played on Windows' player.

## What was seen
| # | Build | Case | Result |
|---|---|---|---|
| 1 | main's engine + the paused-hang fix (#122) | "MKV, H.264, two SRT tracks", 1 min: play, paused at 3 s, 720p 4 Mbit/s, play | Paused: position **0**, a buffered range 0–3 s, the picture at the file's 2.7 s (its burned-in clock). After play: state stayed `Paused`, position 0, then `stream_timeout` and libmpv |
| 2 | the same | the same file, same steps | Played natively after play, the position counted from 0 (the bar read 0:15 while the picture's clock read 0:18) |
| 3 | the same | "MKV, HEVC Main 10, HDR10, 4K, 60 fps", 6 min: played from 0, seek to about 2:20, then 1080p 10 Mbit/s while playing | **`stream_timeout`** 8 s after the change, then libmpv, "Server transcode could not start." W7's failure, reproduced. libmpv started near 0:00 instead of 2:20: the core had taken the item's count as the file's position |
| 4 | main's engine + #122, the item built without a start time and the start set as a seek in `Opened` | file of 1–2: paused at 3 s, 720p 4 Mbit/s, play | Paused at 2.49 s (the file's position). Played natively 0.15 s after play; the bar and the picture's clock agreed; duration 0:57 |
| 5 | the same | file of 1–2 playing at about 30 s, 720p 1 Mbit/s | Played natively, first segment after 2.4 s |
| 6 | the same | file of 3, "Resume" at 5:21 (a local file, native) | Bar 5:27 of 5:59, the file's own numbers |
| 7 | the same | file of 3 playing natively at 5:38, 1080p 10 Mbit/s (Jellyfin: "video to hevc · audio copied · mp4", HDR10 kept) | **Played natively**, first segment after 6.3 s; info panel "Server transcode · HLS · mp4" on Windows' player |
| 8 | the same | the stream of 7, seek from 5:54 to 0:35 | Played natively from 0:35 after about 4 s; buffer ahead grew about 5 s per second |

Each trace line in 4 to 8 came from `ReportPosition`: the position, the playback state, the buffered range ahead, and whether the
first-segment wait still held. Those diagnostic lines were not kept in any PR.

**The same effect on local files.** In run 4's build without the change, a native load at 25 s into the 57 s file showed 0:08 of
0:34 eight seconds later (position and `NaturalDuration` both counted from the start time). So every native start in the middle of a
file (a resume, a fallback, a re-plan for a track, a quality, a display change) reported positions and a duration that were not the
file's. The core then reported progress and stop positions to the server from them, and a fallback started at the wrong place (run
3). W7 §18's media session end time of 5:25 for a 5:59 file fits a start at about 0:34 (inferred, not checked).

**Microsoft's documentation** of the constructor says only that `startTime` is "The position within the MediaPlaybackItem where
playback should begin" ("MediaPlaybackItem Constructors", learn.microsoft.com, ms.date 2017-02-08, read 2026-10-31). It does not say
that Position and NaturalDuration are counted from it. The counting from the start time is what this PC showed (Windows 11 26H2,
build 26300.9711); other builds not verified.

## What else was looked at
- **HEVC output.** W7 suspected the HEVC transcode (W1's was H.264). Runs 4, 5 and 7 were HEVC (Jellyfin chose HEVC each time, as in
  W7) and played natively. Not the cause.
- **A start in the middle of the file.** The cause, through the start time (above).
- **The relay** (`crates/relay`), its requests and playlist rewriting: not changed, and the streams played through it once the
  start time was fixed. Its request log was not read.
- **The PlaybackInfo the core sends** for a native server stream: not changed; Jellyfin transcoded as asked (HEVC in fMP4, audio
  copied, 164–383 fps on the server).
- **Jellyfin's own logs:** not read (the server is the owner's; nothing pointed there once runs 3 and 7 differed only in the item).

## Timing
The first segment came after 0.15 s (paused, already buffered), 2.4 s (1080p H.264 to 720p HEVC) and 6.3 s (4K HDR10 at 60 fps to
1080p HEVC); a far seek back took about 4 s. 6.3 s is close to the 8 s deadline. W1 saw a far seek into a 4K remux wait more than 20
s once. A slow server will still make the deadline move the stream to libmpv; that is the deadline's purpose (ADR 0025, caveat for
W4), but with a 4K source it may come often. Not measured over more files.

## Proposal
1. **Fix (small and clear; its own PR):** build the item with `MediaPlaybackItem(source)` and set
   `PlaybackSession.Position` to the load's start in `Opened` (before a seek the core asked for while opening). Positions, durations,
   the first-segment check and every fallback position are then the file's. Runs 4 to 8 were made with exactly this change.
2. **For the owner, the server-stream route** (W7 proposal 1, decided "Fix, then investigate"): with the cause known, Windows' player
   played Jellyfin's HEVC HDR10 transcode, an H.264-to-HEVC transcode, a resume and a far seek. Proposed: keep ADR 0025 as it is
   (Windows' player first for server streams), without an H.264-only transcoding profile for it. Decided by the owner.
3. **Not now:** the 8 s deadline stays. If 4K transcodes often take longer than that on the first segment, measure over more files
   before changing it.

## Not verified
- Whether a server stream loaded while paused always plays on Windows' player after play: run 1 stayed paused once and run 2 played,
  with the same code; after the fix, run 4 played. Too few runs to say.
- Seeking before the start time on an item built with one (whether Windows' player allows it).
- That the counting from the start time holds on other Windows builds.
- Jellyfin's logs and the relay's request log for these runs.
