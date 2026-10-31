# Stutter on Windows' player (W7, "Playback smoothness")

spike/native-stutter, from `origin/main` at 1bbcc69. Measured on 2026-10-31 on the owner's Windows 11 PC against the owner's own
Jellyfin server, with W7's setup (`docs/notes/windows-native-player-check.md`, "Setup"): the HDR10-only DisplayHDR 400 monitor at
144 Hz with Windows HDR on, the owner's NVIDIA card, the W7 test files. W7 saw the native route stutter on the 4K HDR10 60 fps file
(about 40 Mbit/s) and for the first seconds after a start (about 3 s on the 1080p SRT file, about 8 s on the 4K HDR10 file with PGS),
and could not say whether the decoder, the HDR composition or the relay was the cause. The owner did not watch these runs; nothing
here is judged by eye.

## How it was measured
- **The playback clock.** A local build (not in any PR) added a trace line to the native engine once a second with
  `PlaybackSession.Position`, `PlaybackState` and the buffered range ahead of the position, and one per state change. A script outside
  the repository read them (OutputDebugString). A **slip** is a second in which the position moved more than 30 ms less than the
  wall clock while the state stayed `Playing`: Media Foundation's presentation clock stood still for that long, so the picture froze
  and, when the audio renderer is what waited, the sound too.
- **Load.** Once every 2–3 s (`Get-Counter`): the app's GPU use per engine type (`GPU Engine(pid_*_engtype_VideoDecode)` and `_3D`), the
  whole machine's video decode use, the app's CPU share, and the bytes it read.
- **A/B builds** for subtitles: the same local build with the engine's subtitle code switched off in two steps (below).

## Results
| File | Seconds | Slips (> 30 ms) | Lost | Where | Buffer ahead |
|---|---|---|---|---|---|
| "MKV, HEVC Main 10, HDR10, 4K, 60 fps", 6 min, no subtitles | 122 | 16 | 1.33 s | spread over the run, 0.03–0.18 s each (16.9 s, 18.8, 26.7, 38.5, 56.5, 60.4 …) | 13 s, growing to 39 s; never `Buffering` |
| "MKV, HEVC Main 10, HDR10, 4K, 60 fps, PGS and SRT", 29 s, SRT shown | 30 | 6 | 2.02 s | all in the first 5 s (0.07–0.71 s each); none in the 24 s after | 13 s, growing to 23 s within 6 s |
| "MKV, H.264, SRT inside", 1080p 24 fps, 1 min, SRT shown | 55 | 4 | 1.63 s | all in the first 3 s (0.18–0.69 s each); none after | 13 s, then 37, 46, 52 s: nearly the whole file read in 3 s |
| "MKV, H.264, two SRT tracks", 1080p 24 fps, 1 min, German shown; a seek to 30 s, then English chosen | 42 | 3 | 0.65 s | all in the first 3 s; none after the seek or the switch | 14 s, then 38, 51, 57 s |
| "MKV, HEVC Main 10, SDR", 1080p 24 fps, 1 min, no subtitles | 27 | 0 | 0 | — | 14–17 s, steady |

**Load on the 4K HDR10 run:** the GPU's video decode engine at 4–13 % (average 8 %) for the app, the same for the whole machine, the
app's own 3D use 0–1 %, its CPU 1–2 % of the machine. The decoder is the graphics card's (with the Store HEVC extension) and far from
busy. The info panel's "Player · software decoder" on this route is the panel finding of W7 §1, not the decoder in use.

### Finding 1: the first seconds with a subtitle shown (cause found)
The start-up stutter comes with an embedded text subtitle track that the engine sets to `ApplicationPresented`. Windows' player
then reads far ahead of the position, much faster than it plays (the whole 1-minute file in about 3 s), and its clock stalls during that
read. The same file:

| Build | Slips in the first 5 s | Read-ahead |
|---|---|---|
| As shipped (track `ApplicationPresented`, our cue handlers) | 1.63 s | the whole file in about 3 s |
| Track `ApplicationPresented`, **no cue handlers of ours** | 1.21 s | 58 s ahead after 3 s |
| Every subtitle track left `Disabled` (the engine's subtitle code off) | 0 | 14–17 s, steady |

So our subtitle view and its cue events are not the cause: presenting the text track is. After the first seconds it does not come
back: no slip after a seek inside the file or after switching to the other track while playing. Why Windows' player reads ahead for a
presented MKV text track is not documented that we found. That it is finding the sparse subtitle blocks between the video is inferred.
How far it reads in a feature-length file (minutes of data, or the whole file) was not measured: every test file here is a minute or
shorter, except the 6-minute one, which has no subtitles.

### Finding 2: 4K HDR10 at 60 fps (cause not found)
Without subtitles, the 4K HDR10 60 fps file lost 0.03–0.18 s every few seconds (16 times in 122 s, about 1 % of the time) while
the buffer stayed 13–39 s ahead and the state never left `Playing`. That rules out the relay and the network (the data was there)
and a busy decoder or CPU (above). Left:
- **The display path.** 60 fps on a 144 Hz panel with Windows HDR on, composed by DWM: frame pacing at 2.4 refreshes per frame. Pacing
  alone gives uneven frame times, not a clock that stops; a stall in presenting would.
- **Media Foundation's renderer** waiting on something we did not see (the GPU's video processor, a present queue).
- The 1080p 24 fps files never slipped once they were past the start, on the same monitor.

W7's screen recording (about 24 distinct frames in 9 s of 60 fps video) is far worse than 1 % lost: the recorder probably dropped
frames with HDR on, as W7 said it might.

## Proposal
1. **The start-up stall with subtitles: for the owner** (not fixed here; no option was tested):
   - a. Accept it: it is a second or two, once per start.
   - b. Hold the start paused until Windows' player has finished reading ahead after the subtitle track is set (the buffered range
      stops jumping, or a few seconds pass), then play. The stall becomes a longer start (`Preparing`). Small, in `NativeEngine.cs`.
   - c. Take embedded text subtitles from the server as files, as external subtitles are taken today (the core fetches them and the
      engine adds them as `TimedTextSource`), and leave the embedded tracks `Disabled`. Jellyfin then extracts the subtitle from the
      file, which takes server time before the first cue; PGS cannot go that way (`TimedTextSource` reads text formats).
   Proposed: try b first, measured as above; c if b leaves the start too slow on long files.
2. **4K HDR10 at 60 fps:** measure before any change:
   - the same file with the HDR monitor at 60 Hz and at 120 Hz (the owner changes the refresh rate);
   - the same local file in Windows' own Media Player app, by eye;
   - the same file on libmpv on the HDR monitor (needs the "How to play" setting from the roadmap, or the setting written by hand);
   - a GPU trace (GPUView or PresentMon), which needs an elevated prompt or a new tool, so the owner's approval.
   The 1 % may be the platform's, as it would be in Windows' own player; the comparison with Media Player decides whether there is
   anything for us to fix.
3. **How far Windows' player reads ahead in a long file with a shown subtitle:** measure with a feature-length synthetic file before
   choosing between 1b and 1c. A read of the whole file would also cost network and server load on every start.

## Also seen
Two subtitle cues on screen at once after a seek ("German track 10" and "11"), and two SRT cues together on the 4K file: the stale
cue finding of W7 §10 and §13, which has its own roadmap line.

## Not verified
- Anything judged by eye: the owner did not watch these runs.
- Finding 2's cause; the display's refresh rate was not changed.
- The read-ahead with a long file (proposal 3), and with PGS shown on its own (the 4K file with PGS showed its SRT track).
- Whether a stall happens without our build's trace line (one `Trace` call a second, on the UI thread; the files without subtitles
  played without start-up slips with the same line).
- That the same holds with Windows HDR off: then PQ video plays on libmpv (departure 8).

## Sources
- `MediaPlaybackSession.Position`, `PlaybackState`, `GetBufferedRanges()`, `TimedMetadataTrackPresentationMode` as the engine uses them
  (`apps/windows/Current.Shell/Player/NativeEngine.cs` at 1bbcc69).
- Windows performance counters `GPU Engine(*)\Utilization Percentage` and the process's CPU time and `ReadTransferCount`, read on this
  PC.
