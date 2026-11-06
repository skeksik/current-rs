# Feeding Windows' player from our own stream (spike)

spike/native-own-stream. Measured on 2026-11-06 on the owner's Windows 11 PC, in a local build from `origin/main` at 4979e1a, against
the owner's own Jellyfin server on the LAN (the second server entry the owner added by its LAN address), with the W7 test files
(`docs/notes/windows-native-player-check.md`, "Setup") on the HDR10 monitor at 60 Hz. It follows proposal 1 of
`docs/notes/relay-throughput.md`: Windows' player reads an HTTP stream at only about 110–140 Mbit/s, and that starves the video while
it reads far ahead for an MKV text subtitle; the 4K HDR10 60 fps file loses 2–3 s a minute too
(`docs/notes/windows-native-stutter.md`). The stream code was local and is in no PR; this PR is notes only.

## Answer
**Handing Windows' player a seekable stream of our own instead of the URL removes every stall measured so far.**
Outside one 30 ms slip right after a seek, no file lost a
frame of its clock: not the long 4K file with a subtitle shown (half speed before), not the 4K 60 fps file (2–3 s lost a minute
before), not the start-up stalls of the shorter files. Seeking, resuming, pause, subtitle switching and the end of the file all
worked. **But Windows' player then reads the whole file ahead as fast as the stream gives it, and keeps what it read in memory**:
about 300 MB more than over HTTP for a 6-minute 4K file, so a feature-length file needs our stream to hold it back (see "Memory").

## What the spike built
`MediaSource.CreateFromStream(stream, contentType)` instead of `MediaSource.CreateFromUri(uri)` in the native engine, for direct
play only (not HLS), behind an environment variable. The stream (a C# `Stream`, handed over with `AsRandomAccessStream()`) read the
relay's URL for the file, so the relay, its route token and its refusals stayed as they are:
- **Opening:** one `Range: bytes=0-0` request for the length and the content type (`video/x-matroska` for these files).
- **Windows:** Windows' player reads in more than one place at once: the playback position, the MKV cues at the file's end, and,
  with a text subtitle, a scan far ahead. A first version with a single read position thrashed (64 requests in a few seconds and
  playback nearly frozen). The spike's version keeps up to 4 windows; each is one HTTP request from where a read began, read ahead in
  512 KiB blocks up to 32 MiB past that window's last read, with 4 MiB kept behind it. A read up to 4 MiB past a window's end waits for
  it; a read anywhere else opens a new window, dropping the least recently read one.

## Results
| File | Before (Windows' HTTP reader) | With our stream | Requests | Rate (bytes read / time open) |
|---|---|---|---|---|
| 4K HDR10, 6 min, SRT shown ("W7 21 Long 4K HDR10 with SRT", made for the relay notes) | about half speed (relay notes) | 0 slips in 49 s from the start; 0 in 65 s resumed at 58.5 s | 4–8 | 350–890 Mbit/s |
| 4K HDR10 60 fps, 6 min, no subtitles | 2–3 s lost a minute (stutter notes) | 0 slips in 159 s from the start to the first seek; after the seeks below, one of 0.03 s right after a seek | 3–7 | 110–310 Mbit/s |
| 1080p H.264, 1 min, SRT inside | 1.1–2.1 s lost in the first 3 s (relay notes) | 0 slips in 21 s from the start | 3 | 270 Mbit/s |
| 4K HDR10, 29 s, PGS and SRT, PGS shown (the file's default) | 2.0–3.1 s lost at the start (relay notes, a subtitle shown) | 0 slips in 26 s from the start | 4 | 180–200 Mbit/s |

A **slip** is as in the stutter notes: a second in which `PlaybackSession.Position` moved more than 30 ms less than the wall clock
while the state stayed `Playing`, read once a second from a trace line of a local build.

**Starting:** `Opening` to `Playing` in 0.22 s from the start of the 6-minute 4K file and 0.35 s on a resume at 58.5 s (Windows'
player set the position before its first `Playing`; the seek-in-`Opened` of #124 still works). Not compared with the HTTP reader.

**Everything else checked on the 4K 60 fps file and the long SRT file:**
- A far seek (2:39 to 4:44): playing on at once, one new request. A seek near the end (to 5:50): one new request.
- Two 10 s skips back: each opened a new request, because 10 s of a 40 Mbit/s file is further back than the 4 MiB a window keeps
  behind. Harmless; a larger keep-behind would save them.
- Pause for 4 s and play: picked up at the same position, no slip.
- The subtitle turned off and back on through the player's menu: the cues came back, two new requests (the scan again), no slip.
- The end of the file: playback ran to 5:59.4, the player closed to the detail page and the stream closed after 2788 MiB and 7
  requests.

## Memory
Windows' player buffers a random-access stream much further ahead than an HTTP one. With our stream, `BufferedRanges` showed the
whole file from the first second (358 s ahead at 0.9 s) on both 6-minute files, with or without a subtitle, and the app's memory
followed: its private bytes **fell by about the video's rate as the file played**, so what was read ahead sat in memory.

| Run, the 6-minute 4K 60 fps file | Buffered ahead | The app's private bytes |
|---|---|---|
| Windows' HTTP reader, fresh app, 57–59 s in | 58 s | 654–655 MB, steady |
| Our stream, after several plays, 60–80 s in | 299 s | 976 → 933 → 909 MB over 20 s |
| Our stream, the long SRT file, 175–225 s in | 185 s | 1455 → 1390 → 1331 → 1172 MB over 30 s |

The runs are not like for like (a fresh app against one that had played several files), but the falling memory and the whole file
buffered say the same thing. For a 2-hour 4K remux that would be tens of gigabytes if Windows' player kept going; whether it caps
itself somewhere was not measured (no feature-length file was on the test server). Our stream's own buffers are bounded (at most 4
windows of 36 MiB).

## Proposal
1. **Build it for direct play on the native route, with the read-ahead paced by the stream.** The playback window should stay a
   bounded time ahead of the playback position (for example 60 s, mapped to bytes by the file's size and duration, which the core
   knows), so that Windows' player can never buffer more than that; the cues and subtitle-scan windows read as now. Measure memory on
   a feature-length file before it ships.
2. **Where the stream lives:** the spike's logic (windows, read-ahead, pacing) is not platform integration, so by the project's
   principles it belongs in the core: a byte-range reader over FFI that reads the relay's upstream directly (`current-net`'s
   `send_stream`), without the loopback HTTP hop, with the C# side only wrapping it as an `IRandomAccessStream`. The C# reader over
   the relay's URL that the spike used is the smaller first step and keeps the relay's refusals and events as they are. Either needs a
   decision record; this note only proposes.
3. **HLS stays on `AdaptiveMediaSource`**: its segments are small and it was not part of this spike.
4. With this in place, the subtitle variants of the stutter notes (turn the subtitle on later; take text subtitles from the server as
   files) are not needed for the stall.

## Not verified
- Memory and read-ahead on a feature-length file, and whether Windows' player caps its buffer itself.
- The server over Tailscale (about 120 Mbit/s): all runs here were on the LAN. A link slower than the read-ahead Windows' player asks
  for may bring the stalls back in another form.
- A broken connection or a refused request mid-file: the spike turned a pump's error into an `IOException`; what Windows' player shows
  for that was not seen.
- Audio track switching, scrubbing many times in a row, and files other than MKV.
- Start-up time against the HTTP reader (not measured the same way).
- Other Windows builds and other PCs.

## Sources
- MediaSource.CreateFromStream(IRandomAccessStream, String) (learn.microsoft.com, ms.date 2017-02-08, updated 2025-11-21, read
  2026-11-06): "Creates an instance of MediaSource from the provided IRandomAccessStream"; `contentType` is "The MIME type of the
  contents of the stream."
- `AsRandomAccessStream(Stream)` is already used in `apps/windows/Current.Shell/Browsing/TileImage.cs`, which cites its page.
- The app's memory: `Get-Process` (`PrivateMemorySize64`, `WorkingSet64`) on this PC.
