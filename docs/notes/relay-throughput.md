# How fast Windows' player reads through the loopback relay (spike)

spike/relay-throughput, from `origin/main` at c6b9a33. Measured on 2026-11-06 on the owner's Windows 11 PC against the owner's own
Jellyfin server, with the W7 test files (`docs/notes/windows-native-player-check.md`, "Setup"). The question came from the stutter
notes (`docs/notes/windows-native-stutter.md`, Finding 1): with an embedded text subtitle shown, Windows' player reads far ahead and
its clock stalls for 0.6–3 s at the start. Was that the network, the relay, or Windows' player? Every measuring change below was local
and is in no PR; this PR is notes only.

## Answer
**Windows' player reads an HTTP stream at only about 110–140 Mbit/s, and that is what starves the video during its subtitle
read-ahead.** The relay, `current-net`, the server and the network are each several times faster. The same player reading the same
file from disk, with the same subtitle shown, does not stall.

## What was measured
| Path | Rate | Stall at start with a subtitle shown | How measured |
|---|---|---|---|
| Windows' player through the relay, server over Tailscale | ~120 Mbit/s | 1080p: 2.1 s, 4K: 2.0 s | Tailscale adapter's received bytes; the engine's position once a second |
| Windows' player through the relay, server on the LAN (a second server entry the owner added by its LAN address) | ~111 Mbit/s | 1080p: 1.1 s, 4K: 3.1 s | wired adapter's received bytes (Tailscale 0) |
| libmpv straight from the server on the LAN (no relay: a plain address) | ~356 Mbit/s while filling its cache, then 42 Mbit/s | none | wired adapter's received bytes |
| The relay alone: an upstream with the data at once, a plain TCP reader on 127.0.0.1 | 1.9–5.2 Gbit/s (multi-thread runtime, as the app's), 27–36 Gbit/s (single thread), 8 KiB to 1 MiB chunks | — | a local test against `Relay` |
| `current-net`'s `send_stream` from a local HTTP server | ~36 Gbit/s, ~500 KB chunks | — | a local test in `crates/net/tests/stream.rs`'s stand-in |
| Windows' player from disk (W1's prototype, `MediaSource.CreateFromStorageFile`), SRT presented | the whole 29 s file buffered at once | **none** (1.04, 1.06, 1.05 s per second from 0.6 s) | the prototype's position once a second |

**Inside the relay, on the owner's PC.** A timing wrapper around each relayed body recorded the request's `Range`, its bytes, and how
long the relay waited on the server versus on the player (time between handing a chunk to hyper and being asked for the next). For
the 4K file with PGS and SRT, over the LAN:

| Request | Bytes | Time | Waiting on the server | Waiting on the player | Rate |
|---|---|---|---|---|---|
| `bytes=0-` (the file's start, closed early) | 1.6 MB | 0.15 s | 0.01 s | 0.11 s | 86 Mbit/s |
| `bytes=143130624-143202452` (the file's end: MKV cues) | 72 KB | 0.001 s | — | — | — |
| `bytes=352256-143130623` (the rest) | 142.8 MB | 8.0 s | **0.37 s** | **7.65 s** | 142 Mbit/s |

Chunks from the server averaged about 9 KB. Gathering them into 256 KiB frames before hyper wrote them changed nothing (116 Mbit/s, 8.6
s of 9.8 s waiting on the player). The relay sets `TCP_NODELAY` already (`crates/relay/src/relay.rs`). So Windows' player takes the
bytes off the socket at that rate whatever the relay does, on one long connection.

**Not separated:** whether that rate is Windows' HTTP reader itself or its parsing of the MKV while it reads. A plain local HTTP server
without `Range` (Python's `http.server`) left Windows' player in `Opening`, so the same file over HTTP without the relay was not
measured. The disk run says parsing alone keeps up.

## Why it shows only with a subtitle
Without a presented text track, Windows' player keeps about 15–20 s ahead and reads at the video's rate (67 Mbit/s for the 40 Mbit/s
4K file), well under its ceiling. Presenting an MKV text track makes it read far ahead at once (W7's stutter notes: a 1-minute file
in about 3 s). At ~120 Mbit/s that read-ahead and the playback share one slow reader, and a 40 Mbit/s video starves. Turning the
subtitle on 3 s later (`docs/notes/windows-native-stutter.md`, variant c) removed the stall on a 5 Mbit/s file and moved it on the 4K
one (3.6 s lost after the subtitle came on), as a fixed ceiling predicts. Holding the start paused (variant b) did not help: the
read-ahead ran the whole 4 s cap.

The 4K HDR10 60 fps stalls without subtitles (the stutter notes' Finding 2) happened with 50–59 s buffered, so this ceiling does not
explain them.

## Proposal
1. **Feed Windows' player from our own stream instead of its HTTP reader.** `MediaSource` can be made from a random-access stream
   (`MediaSource.CreateFromStream`, not verified here); the engine would hand it a stream that reads the relay's URL (or the core
   directly) with a large read-ahead, so Windows' player never does its own HTTP reads. Needs a spike first: whether Windows' player
   reads such a stream faster, how seeking and `Range` map onto it, and whether HLS (`AdaptiveMediaSource`) needs it at all (segments
   are small). This would also lift the ceiling for high-bitrate direct play generally (a 100 Mbit/s remux would not fit today).
2. **Avoid the read-ahead instead:** take embedded text subtitles from the server as files (the stutter notes' variant d), so no MKV
   text track is presented. Smaller, but leaves the ~120 Mbit/s ceiling for any high-bitrate file.
3. Leave the relay as it is: it is not the limit.

## Not verified
- Whether the rate is Windows' HTTP reader or its MKV parsing over HTTP (see above).
- How far the read-ahead goes in a feature-length file: "W7 21 Long 4K HDR10 with SRT" (6 min, made for this) is in the owner's
  test folder but the server has not indexed it yet.
- Other Windows builds and other network adapters.
- `MediaSource.CreateFromStream`'s behaviour with a custom stream (proposal 1).

## Sources
- The relay's code read: `crates/relay/src/body.rs` (one chunk per poll), `crates/relay/src/relay.rs` (`MAX_BUF_SIZE` 64 KiB,
  `set_nodelay(true)`), at c6b9a33.
- Windows' network adapter statistics (`Get-NetAdapterStatistics`) and link speeds (`Get-NetAdapter`) on this PC.
- W1's prototype (`MediaSource.CreateFromStorageFile` for a local file), outside the repository as W1 describes it.
