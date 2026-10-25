# The libmpv engine (`current-mpv`, ADR 0019 task 3)

What the engine does, what its tests found, and what is not verified. Design: ADR 0019 ("`current-mpv`", "Gateways, tokens and the
player", "Threads and event rate"), ADR 0007.

## Shape
- `sys`: hand-written `extern "C"` declarations for 13 functions of `include/mpv/client.h` (client API 2.5, mpv 0.41.0) and its node,
  event, property, log-message and end-file structs (ADR 0019 choice 13). A test checks sizes and field offsets for 64-bit targets
  (no `long` in these structs, so LP64 and LLP64 agree); the engine refuses a client API whose major version is not 2.
- `Engine::start` sets the options at start, asks for log messages, initialises, observes the 16 properties of ADR 0019 (with
  `MPV_FORMAT_NODE`; `display-swapchain` only in composition mode), reads `decoder-list` and `demuxer-lavf-list` into
  `BuildCapabilities`, then starts the `mpv-events` thread. Options that a build may lack (`media-controls`, `load-scripts`, `osc`,
  `ytdl`) may answer `option-not-found`; any other failure stops the start.
- Commands are asynchronous only (`mpv_command_node_async`, `mpv_set_property_async`); their failures arrive as
  `EngineEvent::CommandFailed` with the command's name, never a value. `loadfile` uses named arguments with the per-file options as a
  node map of strings (mpv `DOCS/man/input.rst`: "When using the client API, this can be a `MPV_FORMAT_NODE_MAP` ... however the
  values themselves must be strings currently").
- `stream-lavf-o` is a key-value list inside that map; each value is written as `%<bytes>%<value>`, which mpv's `read_subparam`
  (`options/m_option.c`) takes verbatim, so cookies and CRLF-terminated header lines need no other escaping.
- The event thread hands every event to a sink (the controller's channel) and calls nothing else. Dropping the engine sends `quit`;
  the thread then calls `mpv_terminate_destroy` on itself and reports `Shutdown` last. `Engine::shutdown` does the same and waits.
- The swap chain: on a new `display-swapchain` value the event thread `AddRef`s the pointer through `IUnknown`'s vtable and sends
  `SwapChain { handle, generation }`; `release()` or dropping it calls `Release` once.
- Capabilities: `BuildCapabilities` keeps mpv's own names; `engine_capabilities()` maps them onto `EngineCapabilities`
  (current-model) in Jellyfin's names: codecs one to one, `matroska` to `mkv` and `mpegts` to `ts`, the subtitle formats `check.py`
  checks with both spellings (`subrip`/`srt`, `webvtt`/`vtt`), external subtitles on, and no limits (mpv tone-maps every range).

## Cookies: what FFmpeg actually does (read 2026-10-25)
FFmpeg 9.0.2, `libavformat/http.c` (the tarball D6 pins, https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz):
- `get_cookies(s, &cookies, path, hoststr)` is called with `hoststr`, the request's Host value, built by
  `ff_url_join(hoststr, ..., tmp_host, port, NULL)` from the URL; the port is in it whenever the URL names one.
- The domain test is a plain suffix comparison: "find the offset comparison is on the min domain (b.com, not a.b.com)", then
  `av_strcasecmp(&domain[domain_offset], e->value)`. So a cookie with `domain=media.example.com` never matches
  `media.example.com:8096`, and the suffix test has no dot boundary.
- `get_cookies` reads `expires`, `domain` and `path` only; `secure` is never looked at.

So the engine does not pass a cookie's own domain: each cookie gets the stream URL's host and port as written in the URL, and a
Secure cookie is left out unless the URL is https. The live test confirms the cookie reaches the stream's host and not the redirect
target. This corrects the wording "FFmpeg scopes it per request (domain, path, secure)" of ADR 0019 and ADR 0017's earlier text;
ADR 0017 is amended. Limits: a cookie still goes to any host whose Host value ends with the stream's (the suffix test), which only an
absolute segment URL in a playlist from our own server could name; and headers go to every request of the stream, as ADR 0017 says.

## `max_redirects` needs FFmpeg 9
`"max_redirects"` is in FFmpeg 9.0.2's `http.c` options ("Maximum number of redirects") and not in n7.0, n7.1 or n8.0's (each tag's
`libavformat/http.c` from the FFmpeg repository on GitHub, searched for the option on 2026-10-25). With Ubuntu 24.04's libmpv 0.37.0
on FFmpeg 6.1.1, mpv logs "Could not set AVOption max_redirects='0'" and follows the redirect, sending the gateway header to the other
port (the redirect tests fail there, as they should). Our Windows build has FFmpeg 9.0.2. Linux: roadmap M5 follow-up.

## FFmpeg's log and one engine per process
mpv v0.41.0 `common/av_log.c` (read 2026-10-25) installs FFmpeg's log callback for the first mpv instance of the process only
(`init_libav`: `if (!log_mpv_instance) { log_mpv_instance = global; ... av_log_set_callback(mp_msg_av_log_callback); }`) and removes
it when that instance is destroyed (`uninit_libav`); without an instance the callback writes to stderr (`vfprintf(stderr, fmt, vl)`).
The first Windows CI run showed it: tests running engines in parallel printed `[hls @ ...] Opening '...segment1.ts?ApiKey=<test
token>'` on stderr. So the engine allows one instance per process (`Engine::start` waits up to 5 s for an earlier engine's destroy,
else `EngineError::Busy`), a player reuses its engine for every file, and a test runs two engines one after the other in a child
process and checks its stderr for the token, the cookie and the header value. Several player windows: roadmap follow-up.

## Tests
`cargo test -p current-mpv` runs the logic everywhere (options, display mapping, per-file options and escaping, redaction, events,
capabilities, node conversion, struct layouts, swap chain reference counting with a fake COM object). The live tests in
`src/live_tests.rs` need `--features libmpv` and are ignored without it:

| Test | Linux container (libmpv 0.37.0, FFmpeg 6.1.1, 2026-10-25) | Windows CI (D6 build: mpv 0.41.0, FFmpeg 9.0.2) |
|---|---|---|
| Headless start, capabilities contain `check.py`'s `REQUIRED_DECODERS` and `REQUIRED_DEMUXERS` | passed | passed (run 40173566806) |
| A generated one-second WAV plays to the end: `FileLoaded`, `PlaybackRestart`, duration 1 s, rising `time-pos`, `eof-reached`, then `EndFile(Stop)` | passed | passed (run 40173566806) |
| A stream that answers 302 to another port: `EndFile(Error)`, the origin got the gateway header and cookie, the target nothing | failed: no `max_redirects` before FFmpeg 9 | passed (run 40173566806) |
| An HLS playlist whose segments answer 302: segments carry header and cookie, the target gets nothing | failed: same cause | passed (run 40173566806) |
| No log line at `v` level contains the token, the cookie or the header value; a redacted URL line exists | passed | passed (run 40173566806) |
| TLS: `tls-verify=yes` refuses a self-signed stand-in (no request arrives); with it off the same stand-in opens | passed (GnuTLS) | passed (run 40173566806) |
| A second engine waits, then gets `Busy`; after the first is destroyed one starts | passed | passed (run 40175292879) |
| The next file replaces the first on the same engine (`EndFile(Stop)`, then `FileLoaded`) | passed | passed (run 40175292879) |
| Two engines one after the other in a child process: nothing secret on its stderr | passed | passed (run 40175292879) |

The TLS test makes a throwaway certificate at run time with `openssl` (nothing checked in); the Windows image lists OpenSSL 3.6.5
(actions/runner-images, `images/windows/Windows2025-VS2026-Readme.md`, image 20261007.264.1, read 2026-10-25).

## Not verified
- Anything in composition mode (`vo=gpu-next`, D3D11, the swap chain): CI runs headless. That is ADR 0019 task 5, on a device.
- That `AddRef` on the `display-swapchain` pointer cannot race mpv recreating its VO: the reference is taken as soon as the change
  arrives, before the event thread waits again, but mpv does not promise the pointer outlives the notice.
- COM reference counting of the DXGI swap chain from the event thread (DXGI objects are free-threaded; not checked).
- SChannel's revocation check when the revocation server is unreachable (docs/notes/playback.md).
