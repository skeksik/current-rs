# 0026: One stream reader in the core, with libmpv's read-ahead limits

Status: accepted (owner, 2026-11-06), from the shared-stream-reader spike. The owner chose A1, B1 and C1 (2026-11-06); the other
options are kept as considered. Implementation notes from R1 (feat/stream-reader, 2026-11-06) under "As built (R1)". Research, quotes and sources: `docs/notes/shared-stream-reader.md`. No product code was written.

## Context
The owner's decision (2026-11-06): "Make limit calculation the same as it is in libmpv and proceed with shared module", on the
manager's proposal: one stream reader in the shared Rust core, with one read-ahead rule, that every player can read a file through,
so memory and buffering behave the same on every platform whatever each player does on its own.

What led here:
- Windows' player reads an HTTP stream at about 110–140 Mbit/s and stalls while it reads far ahead: half speed on a long 4K file with
  a text subtitle shown, 2–3 s lost a minute on 4K HDR10 at 60 fps (`docs/notes/relay-throughput.md`,
  `docs/notes/windows-native-stutter.md`).
- Fed from a seekable stream of our own (`MediaSource.CreateFromStream`), it stalled nowhere, but read the whole file ahead and kept
  it: about 300 MB more for a 6-minute 4K file; for a feature-length remux that would be tens of gigabytes
  (`docs/notes/windows-native-own-stream.md`).
- libmpv, our app engine, already bounds itself: mpv v0.41.0 (our build) and mpv master have `demuxer-max-bytes` 150 MiB ahead and
  `demuxer-max-back-bytes` 50 MiB behind, with unused forward space donated to the back buffer, in bytes, not seconds (`cache-secs`
  1000 hours) (notes §1).

## Decision

### The reader
- A **stream reader** in the core reads one file of a relay route (ADR 0020) by byte ranges for a player: `size`, `content_type`,
  `read(offset, max_len)` returning up to `max_len` bytes (fewer at the end of a window or the file, empty at the end of the file).
- **Windows** of the file: one ranged upstream request each (`Range: bytes=<offset>-`), up to 4 per file, the least recently read
  dropped; a read inside a window or up to 4 MiB past its end is served by it, any other read opens a window (as the own-stream
  spike).
- **Limits, libmpv's** (notes §1):
  - **Ahead: 150 MiB past the playback point's byte offset**, for what the reader fetches and what it hands the player. A read past it
    waits until the point moves on.
  - **Behind: 50 MiB, plus the unused part of the 150 MiB ahead** (mpv's donation), of bytes kept for reads behind the read
    positions, least recently read first out.
  - One budget per file, summed over its windows, as mpv sums over a file's streams. The numbers are the reader's constants, not
    settings (mpv's are options; we expose none, as `current-mpv` sets none).
- **The playback point**, from what the core knows: `size × position / duration`, clamped, re-anchored at each start and seek to the
  player's first read in the new window, and moved at seeks the core sends before the player reads there (notes §1, "How the
  playback point maps to bytes"). **Starvation guard:** a read held while the engine reports `Buffering` (or its position stops while
  `Playing`) is served at once and lifts the point to it, so an estimate error costs memory, never a stall; each release is counted.
- **Index windows** (a few reads near the end of the file: Matroska cues, tags) are served without the forward limit up to 8 MiB each,
  counted in the budget. Scan windows: choice B.
- **Errors:** a body that breaks off is reopened from the byte it needs, with `If-Range`, three tries a window with a short back-off
  inside the controller's 60 s stall rule; then the read fails and the engine's error runs ADR 0025's fallback chain at the position.
  `send_stream`'s roaming retry (ADR 0020 choice 6) is underneath, unchanged.
- **Credentials, refusals, events:** the reader reads through the relay's `Upstream` trait, so gateway credentials and the token are
  added upstream as for the relay; its failures report the relay's `RelayEvent`s on the route's sink (once each), so the 401 rule is
  unchanged; the 206 range check applies. No HLS playlist goes through it.
- **Lifetime:** a reader belongs to a route and ends with it (stop, item change, re-plan, player close); pending reads then fail as
  cancelled.
- **The info panel** shows, for a route on the reader: buffered ahead (seconds and bytes, from the reader's served and fetched offsets
  against the point), the read rate, requests and reconnects, and starvation releases. Words in the core.

### As built (R1)
Written by R1 (feat/stream-reader, 2026-11-06), with the manager under the owner's rules; where it differs from the text above, this
holds.
- **A reader route is its own type** (`current_relay::ReaderRoute`): the relay's `Upstream`, prefixes and once-per-route events, but
  no port, token or URL. `Relay::add_route` is unchanged. Dropping it cancels its readers (`ReadError::Cancelled`) and their requests.
  This replaces "the listener starts on a route's first HTTP use".
- **Only the playback window prefetches**: the latest-starting window, not an index window, that starts at most the gap (4 MiB) past
  the point and ends at most the read-ahead before it. Other windows fetch only for reads waiting on them (up to the forward limit, not
  held to the summed budget), so a window left at the file's head after a resume does not download its way up to the point. Bytes are
  appended exactly to the limits; a chunk read past them waits in the pump (at most one chunk per window).
- **Pinning the point:** after a start or `seek_to`, the first read that can be the playback position pins it: not an index read,
  not a read in the file's first 4 MiB while the estimate is past them, not more than the gap past the estimate (a scan running
  ahead), not more than the read-ahead before it. A starvation release pins it too.
- **Without a duration** the server's bitrate gives the rate. Without either, the point does not follow the position: it moves only
  when a read pins it after a start or seek and at starvation releases, so memory stays bounded and the player may stall briefly
  each read-ahead; `ReaderFacts::rate_known` is false so the panel can say so.
- **A file that changed on the server** (a 200 to an `If-Range` request, or another part than asked) fails the window as
  `RelayEvent::BadRange`. A body that breaks off is asked again `tries` times (3) with a doubling back-off (500 ms first); the count
  starts again after each chunk received.
- **A cancelled read** (its future dropped) stops counting as waiting, so its window stops fetching for it.
- **`ReaderFacts`:** size, point, `rate_known`, bytes held ahead, served ahead (bytes and time), the read rate over 5 s, requests,
  reconnects, starvation releases, windows.

### Choice A: which crate (decided: A1, owner, 2026-11-06)
- **A1 (chosen): a `reader` module in `current-relay`.** It reuses `Upstream`, `RelayEvent` and its once-per-route sink, the 206
  check and the route's lifetime; no hyper on its path. The crate becomes "the loopback relay and the stream reader"; it still knows no
  service. The relay's listener starts only when a route is used over HTTP, so a route read only through the reader opens no port.
- Considered, A2: a new crate `current-reader` (copying `crates/model`'s shape) that depends on `current-relay` for the trait and the events. Cleaner
  name, one more crate and a public seam in `current-relay` for routes.
- Considered, A3: in `current-playback`: no, it has no HTTP by design (ADR 0019).

### Choice B: which windows the forward limit holds (decided: B1, owner, 2026-11-06)
- **B1 (chosen): every window but index windows**, the subtitle scan included. The scan is what read the whole file in the
  spike, so only this bounds the player's memory. Cost: if Windows' player waits for the scan to reach a cue before showing it, cues
  past the limit may come late or not at all; if its playback waits on the scan, playback may stall at the limit until the starvation
  guard releases the read. Neither is measured: a named measurement in R4 ("cues with the scan held").
- Considered, B2: only the playback window. No risk to cues; the scan may again pull the whole file into the player's memory.

### Choice C: libmpv (decided: C1, owner, 2026-11-06)
- **C1 (chosen): libmpv stays on HTTP** (directly, or through the relay where ADR 0020 says). It already applies these limits
  itself and plays smoothly; on mpv's custom stream API (`mpv_stream_cb_add_ro`, "not stable yet") mpv treats the stream as a local
  file (1 s read-ahead, no back cache) unless `cache=yes`, and then buffers 150 MiB itself on top of the reader (notes §3.2, §5).
- Considered, C2: libmpv on the reader with `cache=yes`: one behaviour on every route, at the cost of double buffering, new `unsafe` callbacks in
  `current-mpv` that block an mpv thread on the core's runtime, and an unstable API. Lowering mpv's own limits instead would depart from
  "the same as libmpv" in mpv itself.

### Over FFI (ADR 0004)
- `current-ffi` exports a **`StreamReader`** UniFFI object: `size() -> u64`, `content_type() -> String`,
  `async read(offset: u64, max_len: u32, cancel: Arc<CancelToken>) -> Result<Vec<u8>, StreamReadError>`, `close()`. `StreamReadError`
  is typed (`Cancelled`, `Failed`, `Closed`) and carries no URL or status text.
- `PlatformFile` (ADR 0019, as amended by ADR 0025 W3) gains `reader: Option<Arc<StreamReader>>`. When it is set, the platform reads
  the file through it and `url` is empty: no relay URL crosses the bridge for that file.
- The async read and cancel follow ADR 0004 as written: the core's runtime does the work; the C# wrapper registers its
  `CancellationToken` to the `CancelToken` and maps `Cancelled` to `OperationCanceledException`, other errors to `IOException`. Each
  read copies its bytes into a `RustBuffer` and again into the platform's buffer; at the rates seen (under 1 Gbit/s) that is not a
  concern (not measured).
- **C#:** a `Stream` over the object (`ReadAsync`, `Seek`, `Length`, `CanSeek`), handed to `MediaSource.CreateFromStream` with
  `AsRandomAccessStream()`. **Swift (later):** the `AVAssetResourceLoaderDelegate`'s data source, answering `respond(with:)` as reads
  complete. **Linux (later):** no bridge; GStreamer's `appsrc` (random-access) reads the reader in Rust.
- ADR 0004 needs no amendment: this is an async call with a cancel token, its existing pattern.

### Which routes use it
| Route | Reader |
|---|---|
| Direct play on Windows' native route | **Yes, first** (W tasks below) |
| HLS and every server stream (remux, direct stream, transcode), any engine | No: unchanged (`AdaptiveMediaSource` on Windows, AVPlayer's HLS on Apple) |
| The Mac's device remux (fMP4 HLS through the relay) | No: a resource loader cannot serve HLS segments (ADR 0020) |
| AVPlayer direct play of a progressive file (M3) | Later, with M3-1 |
| GStreamer direct play (M5) | Later, with L2 |
| libmpv, every platform | No: stays on HTTP (choice C1) |
| Android, web | When those platforms start |

### What it amends (written by the task that implements each part)
- **ADR 0019, "What crosses `current-ffi`":** `PlatformFile` carries a `StreamReader` for routes on the reader and then no URL; the
  redaction exception for the relay URL holds only for files without a reader.
- **ADR 0020:** a route can also be read through the stream reader (no HTTP hop): same server, prefixes, upstream, events and lifetime;
  a route made for the reader is its own type and opens no port (R1, below). "Streaming" ("no buffering beyond a chunk per connection")
  stays the relay's rule; the reader buffers up to this record's limits. With A1, "Where it lives" names the reader.
- **ADR 0025, "URLs across the bridge":** direct play on a platform engine reads through the reader where this record's table says,
  else through the relay as now; the info panel's reader facts.
- **ADR 0002 and CLAUDE.md's layout:** no new crate (A1); CLAUDE.md's line for `crates/relay` names the reader (R1).

## Task split (Windows first; each one branch and PR)
- **R1** (feat/stream-reader): the reader in `current-relay` (choice A1) with windows, the limits, the playback point and its guards,
  reconnects, events and route lifetime; tests with a fake upstream: the forward and back limits (byte-exact), donation, the point
  moving with position events and at seeks, the starvation guard, index windows, a break mid-body reopened with `If-Range`, refusals
  mapped to `RelayEvent`s once, cancellation on route removal, bounded memory with a reader that never reads.
- **R2** (feat/stream-reader-ffi): `StreamReader` and `PlatformFile.reader` in `current-ffi`; the C# `Stream` adapter; bindings tests
  against the stand-in (reads, seeks, cancel, a refusal, route removal). Swift adapter only when M3 needs it.
- **R3** (feat/windows-native-reader): the Windows engine opens direct play with `MediaSource.CreateFromStream` over the reader,
  reports `Buffering` as the guard needs, the info panel's facts; HLS unchanged. Smoke run on CI.
- **R4** (device check, by hand, the owner's PC): memory over a feature-length file (private bytes against the HTTP route and
  libmpv), over Tailscale, a connection dropped mid-file, seeks (far, near the end, short skips back), an audio track switch, cues
  with the scan held (choice B), how often the starvation guard fired; the stutter cases of the stutter and relay notes again.

## New dependencies
None (notes §6).

## Consequences
- Windows' player gets the throughput of the core's HTTP client instead of its own, and its memory is bounded by the same numbers as
  libmpv's, in bytes.
- No stream URL crosses the bridge for files on the reader.
- One more piece of buffering logic in the core, with an estimate (position to bytes) that can be wrong on uneven files; the guard
  turns that into memory, and the device check measures it.
- Two ways to read a route (HTTP and the reader) to keep in step; both share the upstream, the events and the 401 rule.
- Not verified: Windows' player's behaviour when a read is held (cues, `Buffering`), memory on a feature-length file, the estimate's
  error on real films, and every non-Windows player's behaviour with such a source (notes, "Not verified").

## Sources
In `docs/notes/shared-stream-reader.md` ("Sources", read 2026-11-06): mpv v0.41.0 and master 9028187538 (`demux/demux.c`,
`DOCS/man/options.rst`, `include/mpv/stream_cb.h`, `stream/stream_cb.c`, `stream/stream_lavf.c`); Microsoft Learn
(`MediaSource.CreateFromStream`, `IRandomAccessStream`, `InputStreamOptions`); Apple's AVFoundation documentation; GStreamer's `appsrc`
and `giostreamsrc`; Media3's `DataSource` and `DefaultLoadControl`; W3C Media Source Extensions 2; ADRs 0004, 0019, 0020, 0025.
