# One stream reader in the core for every player (spike)

spike/shared-stream-reader, from `origin/main` at d4b88ff, written on 2026-11-06. No code. It turns the owner's decision into a
design: **"Make limit calculation the same as it is in libmpv and proceed with shared module"** (owner, 2026-11-06), on the
manager's proposal the owner accepted: one stream reader in the shared Rust core, with one read-ahead rule, that every player can
read a file through, so memory and buffering behave the same on every platform whatever each player does on its own. The decision
record is ADR 0026 (proposed).

Where it comes from:
- Windows' player reads an HTTP stream at only about 110–140 Mbit/s, and its playback stalls while it reads far ahead for an MKV text
  subtitle or a 40 Mbit/s 4K file (`docs/notes/relay-throughput.md`, `docs/notes/windows-native-stutter.md`).
- Handing it a seekable stream of our own (`MediaSource.CreateFromStream` over a C# stream that read the relay's URL with a large
  read-ahead) removed every stall measured, but Windows' player then read the whole 6-minute 4K file ahead and kept it in memory,
  about 300 MB more than over HTTP (`docs/notes/windows-native-own-stream.md`, "Memory"). Its proposal 1 was a bounded read-ahead
  paced by the stream; proposal 2 put the logic in the core.

## 1. The limit rule: libmpv's
### What libmpv does
Read in mpv at tag v0.41.0 (commit 41f6a645, the version our Windows build pins: `build/mpv-windows/sources.toml`) and at master
commit 9028187538, both on 2026-11-06. The defaults are the same in both.

- `demux/demux.c`, `demux_conf.defaults`: `.enable_cache = -1, // auto`, `.max_bytes = 150 * 1024 * 1024`,
  `.max_bytes_bw = 50 * 1024 * 1024`, `.donate_fw = true`, `.min_secs = 1.0`, `.min_secs_cache = 1000.0 * 60 * 60`. (Master adds a
  `demuxer-hysteresis-bytes` option, off by default; nothing else differs here.)
- `current-mpv` sets none of these options (`crates/mpv/src/options.rs`, `engine.rs` at d4b88ff: no `cache`, `demuxer-*` or
  `cache-secs`), so the app runs on these defaults.
- `DOCS/man/options.rst` (v0.41.0):
  - `--demuxer-max-bytes`: "This controls how much the demuxer is allowed to buffer ahead. … The demuxer will stop reading additional
    packets as soon as one of the limits is reached. (The limits still can be slightly overstepped due to technical reasons.)"
  - `--demuxer-max-back-bytes`: "This controls how much past data the demuxer is allowed to preserve. This is useful only if the cache
    is enabled." And: "If the end of the file is reached, the remaining unused forward buffer space is "donated" to the backbuffer
    (unless the backbuffer size is set to 0, or `--demuxer-donate-buffer` is set to `no`). … (The opposite does not happen: free
    backward buffer is never "donated" to the forward buffer.)"
  - `--demuxer-donate-buffer`: "the back buffer may use up memory up to the sum of the forward and back buffer options, minus the
    active size of the forward buffer." (In the code, `prune_old_packets` applies this at any time, not only at the end of the file.)
  - `--cache-secs`: "The default value is set to something very high, so the actually achieved readahead will usually be limited by
    the value of the --demuxer-max-bytes option."
  - `--cache=<yes|no|auto>`: "The `auto` choice enables this depending on whether the stream is thought to involve network accesses
    or other slow media (this is an imperfect heuristic)."
  - `--demuxer-readahead-secs` (default 1): "The main purpose of this option is to limit the readhead for local playback".
- How "auto" decides (`demux.c`, `update_opts`): `use_cache = is_streaming` unless `cache` is set; with the cache, `min_secs` becomes
  `max(min_secs, min_secs_cache)` and the cache is seekable; without it, `max_bytes_bw = 0`. `is_streaming` comes from the stream's
  `streaming` flag, which `stream/stream_lavf.c` sets for everything FFmpeg opens ("enable cache (should be avoided for files, but
  no way to detect this)"): our http URLs, relayed ones included.
- How the forward limit is counted (`demux.c`, `read_packet`): `total_fw_bytes` is the sum of `get_forward_buffered_bytes` over the
  streams, i.e. demuxed packets not yet handed to the decoders, and reading stops when it reaches `max_bytes`.

So for every stream the app plays today on libmpv: **at most 150 MiB of packets ahead of what the decoders have taken, at most
50 MiB kept behind (plus any unused forward space), counted in bytes, not seconds.**

### The reader's rule
The reader applies the same numbers to file bytes:
- **Ahead: 150 MiB.** The reader never hands the player a byte more than 150 MiB past the playback point's byte offset, and fetches
  from the server only up to that point too. Bytes up to it are served at once if the reader has them, and fetched as fast as the
  server gives them otherwise. A read beyond it waits until the playback point moves on (with the guards in §2).
- **Behind: 50 MiB, plus the unused part of the 150 MiB ahead** (libmpv's donation), of bytes the reader keeps for reads behind the
  read positions (short skips back), least recently read first out.
- **One budget per file**, summed over all its read positions (§4), as libmpv sums over a file's streams.

What this bounds is what the player can pull ahead, not what it keeps: a player that buffers everything it is given (Windows', per
the own-stream notes) then holds at most about 150 MiB ahead, split between its buffer and the reader's. What it keeps behind the
playback point is its own (Windows' private bytes fell as the file played in the own-stream spike, so it seems to drop what it
played; inferred, not documented).

### How the playback point maps to bytes
The player's own read offset is not the playback point: Windows' player reads far ahead of it. The core knows the playback position
(the engine's position events), the file's duration and its size (`MediaSource`, ADR 0019), so:

`point = size × position / duration`, clamped to the file.

Correct on average; wrong where the bitrate is uneven (VBR):
- **The error.** The true offset is the integral of the bitrate up to the position. For a file whose first half is 60 Mbit/s and
  second half 20 Mbit/s, at the middle the true offset is 75 % of the file and the estimate 50 %. On a 30 GB remux that is 7.5 GB.
  - Estimate **behind** the truth (a dense part first): reads the player needs to play now are past `point + 150 MiB`, so the reader
    would hold them and playback would stall. Not acceptable.
  - Estimate **ahead** of the truth (a dense part later): the player may pull more than 150 MiB ahead. Costs memory only.
  Typical films vary less than that example, but a quiet first act and a loud last one is common; how far real files stray was not
  measured.
- **Guards** (proposed, ADR 0026):
  1. **Anchored at each start and seek.** After a load or a seek the player's first read in the new window is where the file holds
     that position (the player found it in the container's index), so the point is set to that offset and moves on from there at the
     average rate (size / duration). The error then grows only from the last anchor, not from the start of the file. Cheap, but a long
     uneven stretch without a seek still drifts; which first read is the anchor (not a read of the cues) needs care in R1.
  2. **A held read is released when the player starves.** If the engine reports `Buffering` (or its position stops while `Playing`)
     while one of its reads is held, the reader serves it and lifts the point to that read's offset. An estimate error then costs
     memory, never a stall; each release is counted for the info panel and the device check.
  3. **Later, if the device check shows guard 2 firing:** the container's own time-to-byte index (Matroska `Cues`, MP4 sample tables),
     which the player reads anyway, read by the reader for the mapping. That is container parsing in the core and is not proposed for
     the first version.
- **A seek before the core knows the new position.** Seeks reach the player through the core (`seek_to`, ADR 0019; the system media
  controls go through the core too), so the reader moves the playback point when the command is sent, before the player's first read
  there. The start position of a load is known the same way. A player that seeks on its own (not expected on any route) is covered by
  guard 2: its first read after the jump would be held, it reports `Buffering`, and the read is served.

### What differs from libmpv, said plainly
- mpv counts **demuxed packets of the streams it reads**; the reader counts **file bytes**, every track's, the container's overhead
  and any audio track nobody listens to included. A file with five audio tracks gives fewer seconds ahead for the same 150 MiB.
- mpv counts from **what the decoders have taken**; the reader counts from **an estimate of the playback point**, with the error above.
- mpv's limit is **its own memory**; the reader's bounds what the player can pull, and the player's own buffer is the player's
  (§3).
- mpv applies 150 MiB however fast the network is. So does the reader: at 40 Mbit/s that is about 31 s of a 4K file ahead, at
  80 Mbit/s about 16 s, at 8 Mbit/s about 2.6 minutes. Seconds are not a limit, as in mpv (`cache-secs` is 1000 hours).

## 2. Seeking, the back buffer, errors and the relay
- **Seeking.** A read inside a window, or up to a small gap past its end (4 MiB, as the spike), is served from it; any other read
  opens a new window with one ranged request (`Range: bytes=<offset>-`). The bytes a window holds behind its reads stay in the back
  budget, so a 10 s skip back on a 40 Mbit/s file (about 50 MB) is served from memory, which the spike's 4 MiB was not
  (own-stream notes, "Everything else checked").
- **Size and type.** Opening reads the size from the first answer's `Content-Range` (the spike used `bytes=0-0`) and the content type
  from its `Content-Type`, else from the container the core already knows (`video/x-matroska` for MKV).
- **Errors and reconnects mid-file.** `current-net`'s `send_stream` already retries a failed request once on a newly selected address
  (ADR 0020 choice 6, roaming). A body that breaks off mid-window is reopened from the next byte it needs, with `If-Range` (the first
  answer's `ETag`, else its `Last-Modified`) so a changed file on the server is not stitched in; three tries a window with a short
  back-off, all inside the controller's 60 s stall rule (ADR 0019). After that the read fails, the platform's stream throws (an
  `IOException` in C#, as the spike did), the engine reports an error, and the controller's fallback chain moves on at the position
  (ADR 0025). What Windows' player shows for that exception was not seen in the spike.
- **Gateway credentials, refusals and events.** The reader reads through the relay's `Upstream` trait (in the app, the server's
  `ServerClient::send_stream`), so it adds the selected address's gateway credentials and the token exactly as the relay does, never
  follows a redirect, and holds no credential the platform can see. Its failures map to the relay's events (`RelayEvent::Refused`,
  `NotFound`, `Server`, `Network`, `BadRange`) on the same route's sink, once each, so the controller's 401 rule (ADR 0017, ADR 0020
  "Lifecycle") is unchanged. The 206 check (`Content-Range` must start where `Range` does) applies as in the relay. No HLS playlist
  ever goes through the reader.
- **Route and lifetime.** A reader belongs to a relay route (same server, same path prefixes, same events) and dies with it: stop,
  item change, re-plan, player close. Removing the route fails pending reads with a cancelled error.
- **The info panel** (the core's words, as today): buffered ahead, in seconds (`(highest served offset of the playback window −
  point) × duration / size`) and bytes; the read rate over the last few seconds; requests and reconnects so far; how many held reads
  were released by guard 2. The platform's own buffered range stays shown where the engine reports it.

## 3. How each player can read through it
| Platform | How a player reads our bytes | What the player does with its own buffer when fed this way |
|---|---|---|
| Windows | `MediaSource.CreateFromStream(IRandomAccessStream, String)` | Reads the whole file ahead as fast as it is given and keeps it (measured, own-stream notes); not documented |
| libmpv (Windows, Linux, macOS later) | `mpv_stream_cb_add_ro` and a custom protocol | Read-ahead of 1 s and no back cache unless `cache=yes` (§3.2) |
| macOS, iOS, tvOS (AVPlayer) | `AVAssetResourceLoaderDelegate` on a custom-scheme `AVURLAsset` | Asks for ranges, often to the end of the resource; buffers about `preferredForwardBufferDuration` ahead |
| Linux (GStreamer) | `appsrc` in random-access mode, or `giostreamsrc` over a `GInputStream` | Not verified for `playbin3`'s own queues |
| Android (Media3) | A custom `DataSource` | `DefaultLoadControl`: 50 s ahead by default, bytes capped |
| Web | Media Source Extensions: the page appends bytes itself | The browser evicts and throws `QuotaExceededError` when full |

### 3.1 Windows: `IRandomAccessStream`
- `MediaSource.CreateFromStream(IRandomAccessStream, String)`: "Creates an instance of MediaSource from the provided
  IRandomAccessStream"; `contentType` is "The MIME type of the contents of the stream" (learn.microsoft.com, ms.date 2017-02-08,
  updated 2025-11-21, read 2026-11-06).
- `IRandomAccessStream`: "Supports random access of data in input and output streams" (same site and dates); reads arrive through
  `IInputStream.ReadAsync(IBuffer, UInt32, InputStreamOptions)`, whose options are `None`, `Partial` ("completes when one or more bytes
  is available") and `ReadAhead` ("may optionally read ahead and prefetch additional bytes") (`InputStreamOptions`, same dates).
- The spike wrapped a C# `Stream` with `AsRandomAccessStream()`. Windows' player read at the playback position, at the MKV cues at
  the end, and far ahead for a text subtitle, all at once (own-stream notes, "What the spike built"); it buffered the whole file
  (`BufferedRanges` 358 s ahead at 0.9 s) and its memory followed. How it sizes its buffer for a byte stream is not documented that we
  found. With the reader it can take at most 150 MiB past the playback point.
- In the app: the C# engine wraps the reader's FFI object as a `Stream` (async reads, `Seek`, `Length`) and hands it over with
  `AsRandomAccessStream()`, as the spike did; the core gives it the size and the content type.

### 3.2 libmpv: `stream_cb.h`
- `include/mpv/stream_cb.h` (v0.41.0 and master 9028187538, identical, read 2026-11-06): "Warning: this API is not stable yet." The
  read callback's "semantics … match read(2) in blocking mode … If no data can be immediately read, the callback must block until
  there is new data." Seek is optional ("mpv will issue a seek to position 0 immediately after opening … to test whether the stream
  is seekable"); `cancel_fn` "will be called from a separate thread than the demux thread, and should not block"; callbacks "must not
  invoke libmpv APIs as that would cause a deadlock".
- `stream/stream_cb.c` (v0.41.0) sets `seekable`, `fast_skip`, `get_size` and the callbacks, but **not `streaming`**, so with
  `cache=auto` mpv treats such a stream as a local file: `demuxer-readahead-secs` of 1 s ahead and no back buffer (`max_bytes_bw = 0`
  without the seekable cache, `demux.c` `update_opts`). To keep today's behaviour mpv would need `cache=yes`, and then it buffers up to
  150 MiB of packets itself, on top of what the reader holds.
- Implementing it means `unsafe` C callbacks in `current-mpv` (it already allows `unsafe`, ADR 0019) that block a thread mpv owns on
  the core's async reads.

### 3.3 Apple: `AVAssetResourceLoaderDelegate`
(developer.apple.com, read 2026-11-06 through its documentation JSON; the pages carry no date.)
- The asset's resource loader hands the delegate `AVAssetResourceLoadingRequest`s whose `dataRequest` names `requestedOffset` ("The
  position within the resource of the first byte requested") and `requestedLength`.
- `requestsAllDataToEndOfResource`: "When this property is true, you should disregard the value of requestedLength and incrementally
  provide as much data, starting from the requested offset, as the resource contains. Continue until all available data was
  successfully loaded, the request was cancelled, or an error occurs." So the reader's limit is what keeps such a request from
  reading the whole file: the delegate answers `respond(with:)` only as fast as the reader serves, which holds at the limit.
- `respond(with:)`: "may be invoked multiple times … to provide the full range of requested data incrementally … The system can retain
  the data you provide and keep processing it for an indefinite period after this method returns."
- `AVPlayerItem.preferredForwardBufferDuration`: "The duration the player should buffer media from the network ahead of the playhead
  … If set to 0, the player will choose an appropriate level of buffering for most use cases." Whether it applies to a
  resource-loader asset ("from the network") is not documented: not verified.
- Limit: a resource loader cannot serve HLS segments (Apple's engineer, Developer Forums thread 113063, quoted in ADR 0020), so the Mac
  device remux (fMP4 HLS, ADR 0025) stays on the relay's HTTP; only AVPlayer's direct play of a progressive file can use the reader.
  AirPlay of a custom-scheme asset was not checked (loopback URLs already fail on receivers, ADR 0008).

### 3.4 Linux: GStreamer
(gstreamer.freedesktop.org, read 2026-11-06; the pages carry no date.)
- `appsrc`, `stream-type`: "For seekable streams the application should connect to the seek-data signal"; `seek-data`: "Seek to
  the given offset. The next push-buffer should produce buffers from the new offset." The application manual: "random-access
  stream-type will make appsrc activate pull mode scheduling while the other stream-types activate push mode", and "In random-access
  mode, it is important to configure the size property with the number of bytes in the stream." `max-bytes`: "The maximum amount of
  bytes that can be queued internally."
- `giostreamsrc`: "This plugin reads data from a custom GIO GInputStream."
- With either, the GStreamer engine (Rust, ADR 0025: no bridge on Linux) reads the reader directly in-process. What `playbin3`'s own
  queues buffer ahead with such a source was not checked.

### 3.5 Android: Media3 `DataSource`
(developer.android.com, read 2026-11-06.) `DataSource`: "Reads up to length bytes of data from the input." `DefaultLoadControl`:
`DEFAULT_MIN_BUFFER_MS = 50000` and `DEFAULT_MAX_BUFFER_MS = 50000` ("The default maximum duration of media that the player will
attempt to buffer"), `DEFAULT_TARGET_BUFFER_BYTES = -1` (computed per track type: `DEFAULT_VIDEO_BUFFER_SIZE = 131072000`,
`DEFAULT_AUDIO_BUFFER_SIZE = 13107200`, at most `DEFAULT_MAX_BUFFER_SIZE = 210239488`), `DEFAULT_PRIORITIZE_TIME_OVER_SIZE_THRESHOLDS =
false`. So Media3 holds itself to about 50 s or ~125 MiB of video; the reader's limit sits above that. Kotlin bindings for the core
do not exist yet (later platform).

### 3.6 Web: MSE
(W3C Media Source Extensions 2, Working Draft 04 November 2026, read 2026-11-06.) The page appends bytes itself (`appendBuffer`);
when the buffer is full the "coded frame eviction algorithm" runs and, "If the [[buffer full flag]] equals true, then throw a
QuotaExceededError exception". The page decides what it appends, so the reader's rule maps directly (append up to the limit, `remove()`
behind). MSE takes fMP4 or WebM, not MKV (the remux, ADR 0025), and a browser runs the core only as WebAssembly: a later platform.

## 4. Several read positions at once
Windows' player reads in up to three places for one MKV (own-stream notes): the playback position, the cues at the end, and with a
text subtitle a scan far ahead. The spike kept up to 4 windows; the reader does the same (one ranged request each, the least recently
read dropped).

- **The playback window** (the one whose reads are nearest above the playback point): held to `point + 150 MiB`.
- **An index window** (a few reads at the end of the file, Matroska `Cues`, `SeekHead`, tags): small, served without the forward
  limit up to 8 MiB per window (a proposal; cues of a 2-hour file are around a megabyte, not measured), counted in the budget.
- **A scan window** (reads that run ahead faster than playback, the subtitle scan): the open choice B in ADR 0026.
  - Recommended: held to `point + 150 MiB` like the playback window. The scan is what read the whole file in the spike, so this is
    what bounds memory.
  - The cost: if Windows' player waits for the scan to reach a cue before it shows that cue, cues may come late or not at all for the
    part past the limit, and if its playback waits on the scan (the stutter notes saw its clock stall during the scan over HTTP),
    playback may stall at the limit. Guard 2 (§1) then releases the held read: memory again, not a stall. Neither was measured; it is
    a named measurement in the device check (R4).
  - The alternative: only the playback window is held, index and scan windows read freely. No risk to cues, but the subtitle scan
    can again pull the whole file into the player's memory.

## 5. Should libmpv move onto the reader?
- **For:** one behaviour on every route; libmpv's memory and network use follow the same code as the native player's; reconnects and
  the info panel's numbers the same; no loopback HTTP for libmpv either.
- **Against:**
  - Double buffering: on a `stream_cb` stream mpv needs `cache=yes` to keep its seekable cache and read-ahead (§3.2), so it would
    hold up to 150 MiB of packets and the reader up to 150 MiB of bytes. Lowering mpv's `demuxer-max-bytes` to avoid that departs from
    "the same as libmpv" in mpv itself.
  - libmpv already applies this rule natively and plays smoothly (relay notes: straight from the server on the LAN, ~356 Mbit/s while
    filling its cache, then the video's rate; the long 4K file with a subtitle shown played without a stall), so it has no problem to
    fix.
  - `stream_cb.h` says the API "is not stable yet"; callbacks block an mpv thread on the core's runtime; new `unsafe` code.
- **Recommendation: libmpv stays on HTTP** (directly, or through the relay where ADR 0020 says), and the question is reopened only if
  the device check shows the two routes behaving differently in a way users notice. The owner decides.

## 6. Dependencies
None. The reader needs tokio, `bytes` and `current-net`, which `current-relay` already uses; UniFFI is in place. Nothing for the
owner to approve.

## Not verified
- How far real films' bitrates stray from their average (the size × position / duration error), and how often guard 2 would fire.
- What Windows' player does when a read is held: whether it shows `Buffering`, whether cues come late with the scan held (§4), and how
  it reports an exception from the stream.
- Memory on a feature-length file with the reader (the device check, R4).
- Whether `preferredForwardBufferDuration` applies to a resource-loader asset; what AVPlayer, GStreamer's `playbin3` and Media3 do
  with such a source in practice.
- That Matroska cues stay under 8 MiB on long files.
- AirPlay with a resource-loader asset.

## Sources (read 2026-11-06)
- mpv v0.41.0 (41f6a645) and master 9028187538: `demux/demux.c`, `DOCS/man/options.rst`, `include/mpv/stream_cb.h`,
  `stream/stream_cb.c`, `stream/stream_lavf.c`, `options/options.c` (github.com/mpv-player/mpv).
- This repository at d4b88ff: `build/mpv-windows/sources.toml`, `crates/mpv/src/options.rs`, `crates/relay/src/` (`upstream.rs`,
  `events.rs`, `relay.rs`), `crates/net/src/stream.rs`, `crates/ffi/src/platform_engine.rs`,
  `apps/windows/Current.Shell/Player/NativeEngine.cs`; `docs/notes/windows-native-stutter.md`, `docs/notes/relay-throughput.md`,
  `docs/notes/windows-native-own-stream.md`, `docs/notes/windows-mpv-build.md`; ADRs 0004, 0019, 0020, 0025.
- Microsoft Learn (ms.date 2017-02-08, updated 2025-11-21): `MediaSource.CreateFromStream(IRandomAccessStream, String)`,
  `IRandomAccessStream`, `InputStreamOptions`.
- Apple Developer documentation: `AVAssetResourceLoaderDelegate`, `AVAssetResourceLoadingDataRequest` (`requestedOffset`,
  `requestedLength`, `requestsAllDataToEndOfResource`, `respond(with:)`), `AVPlayerItem.preferredForwardBufferDuration`; Developer
  Forums thread 113063 as quoted in ADR 0020.
- GStreamer documentation: `appsrc`, "Pipeline manipulation" (application manual), `giostreamsrc`.
- Android Developers: `androidx.media3.datasource.DataSource`, `androidx.media3.exoplayer.DefaultLoadControl`.
- W3C, Media Source Extensions 2, Working Draft 04 November 2026.
