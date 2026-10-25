# 0019: Playback: current-playback and current-mpv (M2, Windows first)

Status: accepted when its PR merges; open choices decided by the owner 2026-10-25 (see "Decided by the owner"). From the playback
spike.
Research, quotes and sources: `docs/notes/playback.md`. No code was written.

## Context
M2 needs Jellyfin playback on Windows every day, with resume and progress synced to the server (docs/roadmap.md). ADR 0002 gives
`current-playback` the device profile, route planner, engine trait and playback controller, and `current-mpv` the libmpv engine.
ADR 0007 decided the Windows video route (libmpv in D3D11 composition mode under a `SwapChainPanel`) and listed what M2 must do. ADR
0017 decided how gateway credentials reach the player, in outline. ADR 0008 (AVPlayer, M3) and ADR 0011 (libmpv in a `GtkGLArea`, M5)
must fit the same core.

What the research found (details and quotes in the notes):
- **Jellyfin 12.2 never offers direct stream.** `MediaInfoHelper.SetDeviceSpecificData` turns it off for every client ("direct-stream
  http streaming is currently broken"). A PlaybackInfo answer is either direct play (the client builds the static stream URL) or a
  `TranscodingUrl`, and a remux is a transcode job that copies the streams.
- **The profile's `MaxStreamingBitrate` defaults to 8 Mbit/s**, and a source without a known bitrate counts as 40 Mbit/s, so a
  client that leaves it out gets transcodes it does not need.
- **The transcode reasons are not in the JSON** (`TranscodeReasons` is `[JsonIgnore]`); they are only in the `TranscodingUrl` query
  and in the session's `TranscodingInfo`.
- **Every server-made URL carries the token as `ApiKey`:** `TranscodingUrl`, every HLS variant and segment (the master's query string
  is copied into them), and subtitle delivery URLs. An external subtitle whose path is an http URL points to another host.
- **Progress reports keep HLS transcodes alive** (60 s kill timer; progressive 10 s), and a session without a check-in for 5 minutes
  is stopped by the server. The server decides "played" from the reported position (5 % / 90 % / 300 s defaults).
- **Jellyfin already remembers track choices** per user and item (from the indexes in progress reports) and applies the user's
  language preferences; the answer's `DefaultAudioStreamIndex` and `DefaultSubtitleStreamIndex` carry the result.
- **Media segments** (intro, outro, recap, preview, commercial) are a 12.2 API, but no provider ships with the server: plugins only.
- **mpv 0.41.0 turns TLS verification off** unless `tls-verify=yes`; it reads cookies only from a Netscape **file**; its
  `stream-lavf-o` is applied last and reaches FFmpeg's `cookies`, `headers` and `max_redirects` directly; and it logs the URL at
  info level ("Playing: %s").
- **FFmpeg 9.0.2 keeps custom headers on a redirect to another host**, and `max_redirects=0` makes any redirect an error.

## Decision

### Crates and where things live
| Crate | Gets |
|---|---|
| `current-model` | The playback types the bridge and other crates see: `MediaSource`, `MediaStream`, `StreamKind`, `Codec`, `VideoRange`, `Route`, `RouteReason`, `PlaybackPlan`, `TrackChoice`, `Segment`, `SegmentKind`, `PlaybackProblem`, `PlayerState`, `NowPlaying`. Open values for what a server defines (codecs, stream kinds, ranges, reasons, segment kinds) |
| `current-jellyfin` | Wire types (private) and calls: `PlaybackInfo` with the Jellyfin `DeviceProfile` built from `current-playback`'s capabilities, the static stream URL, `Sessions/Playing`, `/Progress`, `/Stopped`, `MediaSegments`, the session's `TranscodingInfo`, the user's playback configuration |
| `current-playback` | Logic only, no HTTP and no libmpv: engine capabilities, the `Engine` trait, the `MediaServer` trait (what a server must answer for playback), the planner, the controller, track selection, segments, next episode, fallback, progress cadence |
| `current-mpv` | The libmpv engine: the only crate with `unsafe` (ADR 0002), the event thread, option mapping (ADR 0007's colour mapping included), capabilities of our build |
| `current-app` | Composition: a `Player` per window that joins a `current-playback` controller, a `current-mpv` engine and the signed-in `JellyfinClient` (as `MediaServer`); gateway credentials and the 401 rule (ADR 0017) |
| `current-ffi` | The player object for C# and Swift (below) |

`current-playback` knows no service: Stash scenes (later) answer the same `MediaServer` trait, and AVPlayer (M3) is another `Engine`.

### Media sources and streams (`current-model`)
- `MediaSource { id, name, container, size, bitrate, duration, streams: Vec<MediaStream>, default_audio, default_subtitle,
  has_segments, is_remote }`; `MediaStream { index, kind, codec, profile, level, language, title, display_title, is_default,
  is_forced, is_hearing_impaired, is_external, video: Option<VideoFacts>, audio: Option<AudioFacts>, subtitle_delivery }`;
  `VideoFacts { width, height, bit_depth, frame_rate, range, range_type, dv_profile }`, `AudioFacts { channels, layout, sample_rate }`.
  `index` is the server's stream index (Jellyfin's `MediaStream.Index`), the one the server and progress reports use.
- The browse spike's media summary (ADR 0018: container, stream counts and labels) is derived from an item's first `MediaSource`,
  so the details screen needs no second wire type.
- No URL, path or token is in these types. Paths stay in `current-jellyfin`; URLs exist only inside the engine handoff (below).

### Engine capabilities and the device profile
- **Each engine reports `EngineCapabilities`:** containers it demuxes, video and audio codecs it decodes (with profile, level, bit
  depth and range limits where it has them), subtitle formats it renders, and whether it can take external subtitles. For `current-mpv`
  these are the lists our Windows build is checked for (`build/mpv-windows/check.py`: H.264, HEVC, AV1, VP9, VP8, MPEG-2, VC-1; AAC,
  AC-3, E-AC-3, TrueHD, DTS, FLAC, Opus, Vorbis, MP3, PCM; ASS, SRT, WebVTT, PGS, DVD, DVB, mov_text; Matroska/WebM, MP4/MOV, MPEG-TS,
  AVI, Ogg, FLAC) read at start from mpv's `decoder-list` and `demuxer-lavf-list`, so a different build reports what it really has.
  mpv tone-maps HDR and Dolby Vision itself (ADR 0007), so no range is excluded (choice 1).
- **`current-jellyfin` maps capabilities to Jellyfin's `DeviceProfile`:** one `DirectPlayProfile` per container with the codec lists;
  `SubtitleProfiles` with `Embed` for every format the engine renders and `External` for text formats and PGS files; `CodecProfiles`
  only for the quality limit; `TranscodingProfiles` per choice 2; `MaxStreamingBitrate` always set (never Jellyfin's 8 Mbit/s
  default) from the quality choice (choice 10). The profile is sent in every PlaybackInfo body.

### Route planner
- Input: the item, the chosen source (the user's version pick, else the server's first), the start position, the chosen tracks, the
  quality limit and flags from the controller (direct play allowed or not after a fallback).
- One `POST /Items/{id}/PlaybackInfo` per plan with the profile, `MaxStreamingBitrate`, `StartTimeTicks`, `AudioStreamIndex`,
  `SubtitleStreamIndex`, `MediaSourceId`, and `EnableDirectPlay` false only after a fallback.
- **Routes** (`Route`, closed: Current defines them): `DirectPlay` when `SupportsDirectPlay`; otherwise `ServerStream { protocol,
  container, video, audio }` from `TranscodingUrl`, where `video` and `audio` are `Copy` or `Transcode`. They are shown as "Direct
  play", "Remux" (both copied), "Direct stream" (video copied, audio converted) and "Transcode". Before start the planner predicts copy
  from the reasons; after start it reads the session's `TranscodingInfo` (`IsVideoDirect`, `IsAudioDirect`) and corrects the label.
  No `SupportsDirectPlay` and no `TranscodingUrl` is `PlaybackProblem::NotPlayable { reasons }`.
- **Reasons** (`RouteReason`, open): the server's `TranscodeReasons` names from the `TranscodingUrl` query, mapped to constants
  (unknown names pass through), plus Current's own: `QualityLimit`, `FallbackAfterEngineError`, `UserChoseTranscode`. The playback
  information view shows them in plain words, with the server's name for unknown ones.
- **Permissions:** the server enforces the user's policy (`EnableVideoPlaybackTranscoding`, `EnablePlaybackRemuxing`,
  `ForceRemoteSourceTranscoding`, `RemoteClientBitrateLimit`); the planner reads them from the user only to explain an outcome ("the
  server limits remote streaming to 8 Mbit/s").
- **Direct play URL:** built by `current-jellyfin`: `/Videos/{id}/stream?static=true&mediaSourceId=..&playSessionId=..&tag=..`
  on the selected address, plus the token per choice 3. Never the source's own `Path`, even for remote sources.

### Playback controller
One controller per player, a state machine in `current-playback`, driven by user actions, engine events and a clock (testable with a
fake engine, a fake server and a fake clock):
- **States** (`PlayerState`): `Idle`, `Preparing` (planning), `Loading`, `Playing`, `Paused`, `Buffering`, `Seeking`, `Ended`,
  `Failed { problem }`. Position, duration, buffered range, tracks, route and next-episode state ride along.
- **Start and resume:** the caller passes the start position (resume or start over; the details screen owns that choice, ADR 0018);
  it goes into PlaybackInfo (`StartTimeTicks`) and into mpv's per-file `start` option, for direct play and HLS alike.
- **Progress reporting:** `Sessions/Playing` after the file loaded and the first `playback-restart`; `/Progress` on a fixed cadence
  (choice 7) and at once on pause, resume, seek done and track change; `/Stopped` with the position on stop, end, item change,
  error (`Failed = true`) and app exit (best effort, short timeout). Reports carry `PlaySessionId`, `MediaSourceId`, the stream indexes
  and the `PlayMethod` (`DirectPlay` or `Transcode`). The server decides "played" and resume points; Current never marks played
  itself during playback.
- **Tracks:** audio and subtitle lists come from the `MediaSource` (server indexes) joined with the engine's track list (mpv's
  `ff-index` for embedded streams; equality with Jellyfin's index is not verified, so the join is tested on real files in task 5).
  Defaults are the answer's `DefaultAudioStreamIndex` and `DefaultSubtitleStreamIndex` (choice 6). Switching under direct play
  sets `aid`/`sid`; under a server stream, an audio change or a burned-in subtitle change re-plans at the current position (new
  `PlaySessionId`, the old one reported stopped) and the UI shows `Preparing`.
- **External subtitles:** the core fetches every external subtitle itself (Jellyfin's delivery URL through `current-net` with the
  address's credentials; a URL on another host with no credentials at all) and hands it to mpv as `memory://` (text) or `hex://`
  (binary), so no subtitle request from mpv carries a token, a header or a cookie, and none can redirect.
- **Next episode:** for an episode the controller asks the server for the following one at start (the browse spike's query) and
  offers it from the outro segment, else in the last seconds or at the end (choice 8). Jellyfin's `EnableNextEpisodeAutoPlay`
  is read with the user.
- **Segments:** when the source `HasSegments`, the controller fetches `GET /MediaSegments/{id}` once and exposes the segment under
  the position (`Intro`, `Recap`, `Preview`, `Outro`, `Commercial`; unknown kinds pass through). With the auto-skip setting on
  (decided choice 8), it seeks past `Intro` and `Outro` segments itself. Without a provider plugin the feature is simply absent.
- **Fallback** (choice 9): an engine failure on a direct-play route (`END_FILE` with an error before or soon after the first
  frame, or a decoder failure mpv reports) re-plans once with `EnableDirectPlay = false` at the current position and the same tracks,
  with the reason `FallbackAfterEngineError`. A failure on a server stream, or a second failure, ends in `Failed`.
- **Errors** (`PlaybackProblem`, flat, no server text or URL): `NotPlayable { reasons }`, `Engine { kind }`, `Network`,
  `GatewaySignInNeeded`, `SignedOut`, `Server { status }`, `NotFound`. A redirect, a 401, 403 or an HTML answer on a stream runs ADR
  0017's 401 rule through `current-net`'s probe before choosing between `GatewaySignInNeeded` and `SignedOut`.
- **Stopping:** stop reports, unloads (`stop` command), and keeps the engine for the next item; closing the player window destroys it.

### Gateways, tokens and the player
- **Token:** `ApiKey` in the URL (choice 3), since every server-made URL already carries it.
- **TLS:** always `tls-verify=yes` (mpv's default is off); verification is Windows' own through SChannel, which matches ADR 0014's
  "OS trust only". Plain http only where ADR 0012 allows it for the token.
- **Gateway cookies:** never a cookie file. The engine passes FFmpeg's own `cookies` option (newline-separated `Set-Cookie` syntax,
  with domain, path and `secure`) through `stream-lavf-o` as a per-file option of `loadfile`, so it lives only for that file and
  FFmpeg scopes it per request (choice 5). This corrects ADR 0017's wording ("its `cookies` option").
- **Basic and extra headers:** FFmpeg's `headers` option the same way, per file.
- **Redirects off the address** (choice 4): every stream Current gives mpv gets `max_redirects=0`. Jellyfin's stream endpoints
  do not redirect (remote sources are proxied by the server), so a redirect means a gateway or a misconfigured proxy, which the 401
  rule then classifies. Headers therefore never follow a redirect to another host.
- **Redaction:** no URL that carries a token or a cookie crosses `current-ffi`, enters the request log, the playback information view
  or a file. The engine never sets `log-file`; it takes mpv's messages through `mpv_request_log_messages` at `info` (`v` while
  diagnosing), and replaces the session token, every gateway secret value and any `ApiKey=`/`api_key=` value with `<redacted>` before a
  line reaches the log; URL-valued properties (`path`, `stream-open-filename`, `playlist`, `filename`) are never read out. The playback
  information view shows routes, codecs and reasons only. Tested with a token that must not appear in any captured line.
- **Cast and AirPlay:** unchanged from ADR 0017 (not in M2).

### `current-mpv`
- **Binding:** hand-written `extern "C"` declarations for the subset of `client.h` the engine uses (about 25 functions and the event
  and node structs), linked against `mpv.lib` from our D6 build (Windows) or the system libmpv (Linux development, ADR 0011); no
  binding crate (choice 13). All `unsafe` sits in one `sys` module and the handle type, each block commented with the invariant.
- **Handle:** `Mpv` owns the `mpv_handle`; the client API is thread-safe ("generally fully thread-safe"), so commands go from any
  thread, always as `mpv_command_node_async` / `mpv_set_property_async` so no core worker waits on mpv's lock. `mpv_terminate_destroy`
  (blocking until the VO thread ends, ADR 0007) runs on a blocking thread.
- **Event thread:** one OS thread per engine (`mpv-events`) blocks in `mpv_wait_event` and turns events into `EngineEvent`s on a
  channel to the controller; it never calls a UI listener. The wakeup callback is not used.
- **Observed properties:** `time-pos`, `pause`, `paused-for-cache`, `seeking`, `duration`, `seekable`, `eof-reached`, `track-list`,
  `aid`, `sid`, `demuxer-cache-time`, `video-params`, `hwdec-current`, `volume`, `mute`, `display-swapchain`. mpv coalesces changes per
  event-queue drain; the controller throttles further (below).
- **Options at start:** ADR 0007's list (`vo=gpu-next`, `gpu-api=d3d11`, `gpu-context=d3d11`, `d3d11-output-mode=composition`,
  `d3d11-composition-size`, `hwdec=auto-safe`, `force-window=yes`), plus `idle=yes`, `keep-open=yes` (the controller decides what
  happens at the end), `config=no`, `input-default-bindings=no`, `input-vo-keyboard=no`, `media-controls=no`, `tls-verify=yes`,
  `user-agent=Current/<version>`, `network-timeout=30`. `load-scripts`, `osc` and `ytdl` exist only in builds with Lua, JavaScript
  or C plugins (`options/options.c`, `#if HAVE_LUA`), which ours is not; the engine turns them off only where mpv knows them (a
  system libmpv on Linux, ADR 0011). Per file through `loadfile`'s option map: `start`, `aid`, `sid`, `stream-lavf-o`
  (`max_redirects`, `cookies`, `headers`).
- **Display facts to options:** ADR 0007's mapping, unchanged, plus `display-fps-override` from the refresh rate. `video-sync` stays
  mpv's default until `display-resample` is checked on a device (ADR 0007 item 5).
- **Swap chain:** on each `display-swapchain` change the engine `AddRef`s the pointer through `IUnknown`'s vtable (no Windows crate
  needed) and publishes `SwapChain { handle: u64, generation }`; the UI calls `SetSwapChain`, then `release()`; dropping releases too.
- **Capabilities:** read once from `decoder-list` and `demuxer-lavf-list` after `mpv_initialize`.
- **Shipping:** the Visual C++ runtime that the DLLs need stays a roadmap follow-up for ADR 0003 with the owner, done in task 3.

### What crosses `current-ffi`
| Direction | What |
|---|---|
| UI to core | `open_player(composition_size, display_facts) -> Player` (the size is needed before the engine starts, ADR 0007); `set_composition_size`, `set_display_facts` (HDR, peak and SDR white nits, refresh rate) |
| UI to core | Actions: `play(item, start)`, `pause`, `resume`, `toggle`, `seek_to`, `seek_by`, `stop`, `select_audio`, `select_subtitle`, `set_quality`, `skip_segment`, `play_next`, `set_volume`, `set_muted`; long ones async with a `CancelToken` |
| Core to UI | `subscribe_state`: one value with `PlayerState`, position (throttled), duration, buffered, tracks, route label, segment under the position, next episode, problem |
| Core to UI | `subscribe_swap_chain`: `SwapChain { handle, generation }` with `release()` |
| Core to UI | `subscribe_now_playing`: what the system media controls need (below) |
| Core to UI | `playback_info()`: route, reasons, source and output facts, the server's transcoding facts, the engine's decoder and output facts; no URL |
| Shutdown | UI detaches the panel and releases the swap chain; dropping `Player` reports stopped and destroys the engine (ADR 0007's order) |

### Threads and event rate
- mpv can report `time-pos` once per frame (24 to 144 per second), coalesced by its event loop. The controller keeps the exact position
  internally and publishes state to listeners at most 4 times a second while playing, and at once on any state, seek, track, segment
  or route change. A playing player therefore causes about 5 listener calls a second, of the same kind the M1 flows make.
- The engine's event thread is not a core worker and calls no listener. Playback adds no tokio worker threads: mpv calls are
  asynchronous and the one blocking call runs on the blocking pool.
- Listener delivery is whatever the panic-hook follow-up (fix/uniffi-panic-hook, in progress) settles; playback adds no second
  delivery mechanism. The roadmap's question "decide when the playback work raises the worker count" is answered: playback does not
  raise it.

### Media keys, the media flyout and full screen (Windows; the core's part only)
- `NowPlaying` (value): title, subtitle (series, season and episode), artwork bytes (through the image path, with gateway credentials,
  ADR 0017), media type video, playback status (playing, paused, stopped, changing), timeline (start, end, position, seekable range),
  enabled commands (play, pause, stop, seek; next when a next episode exists; previous as restart), and a timeline revision that
  changes on every seek completion, state change and every 5 s while playing (D2: a stale position stays until the timeline is
  updated). The UI copies it into `SystemMediaTransportControls` from `GetForWindow` and forwards button presses and position
  requests as actions. Setting the display updater's type to video is what keeps the screen saver off (Microsoft Learn).
- Full screen, the presenter, cursor hiding, keyboard shortcuts and Narrator names are UI work (ADR 0007 item 3); the core only
  provides the actions they call.

## Decided by the owner (2026-10-25)
1. Device profile: **generated** from the engine's capabilities.
2. Transcode format: **A**, HLS with fMP4 segments (HEVC, then H.264), with the TS/H.264 profile listed second.
3. Token for the player: **`ApiKey` in the URL**, with the redaction rules in "Gateways, tokens and the player".
4. Redirects off the address: **A**, `max_redirects=0` on every stream.
5. Gateway cookies for mpv: **A**, FFmpeg's `cookies` option through `stream-lavf-o`, per file.
6. Remembered track choices: **A**, follow the Jellyfin user's own preferences.
7. Progress cadence: **A**, every 10 s plus at once on events.
8. Segments and next episode: **B, the auto-skip option.** Skip buttons; the "Next episode" card at the outro, or in the last 30 s
   without segments; autoplay after a 10 s countdown when the Jellyfin user's `EnableNextEpisodeAutoPlay` is on. **Plus a Current
   setting to skip intros and credits automatically** (the `Intro` and `Outro` segments), off by default, stored in `settings.json`
   as an open value (ADR 0013) so later kinds (recap, preview) can join without a migration. It is shown under a "Playback" group in
   Settings, laid out as the existing Settings page's groups are (docs/notes/windows-shell.md). When it is on, the controller seeks
   past the segment as the position enters it, once per segment and play; a seek back into the segment does not skip it again.
9. Fallback: **A**, one automatic re-plan with direct play off.
10. Quality: **A**, "Maximum" by default, plus a per-playback quality menu remembered separately for local and internet addresses;
    no automatic bandwidth test now.
11. Refresh-rate switching: **A**, not in M2; `display-fps-override` and the `display-resample` check on the device.
12. Recorded, not a choice (D6's settled items).
13. libmpv binding: **A**, hand-written `extern "C"` declarations, no new dependency.

The amendments to ADR 0017 and ADR 0007 below are approved; they are written into those ADRs by task 3, not by this record.

The options as they were put to the owner follow.

## Open choices (as proposed)

1. **Device profile: generated from the engine's capabilities (recommended) or a fixed profile per platform.** Generated, the
   profile follows the build (a codec added or removed in D6 changes it) and the same code serves AVPlayer, whose capabilities come
   from track decodability (ADR 0008). Fixed is simpler to read but drifts from the build. Within it: no range limit (mpv tone-maps
   HDR10, HLG and Dolby Vision, ADR 0007) and no resolution limit; codec limits only when task 5 finds a file that fails.
   **Recommendation:** generated; consequence: a profile test per build in CI (`check.py`'s lists against the profile).

2. **Transcode format.** A: HLS with fMP4 segments, HEVC then H.264 video, AAC, AC-3, E-AC-3, Opus, FLAC audio (Jellyfin's fMP4 HLS
   audio list includes these, `StreamBuilder._supportedHlsAudioCodecsMp4`). B: HLS with MPEG-TS segments, H.264 and AAC only (most
   compatible, worst quality per bit). C: progressive (`stream.mkv`): one long response, so a seek past the transcoded part needs a
   new request and job (not researched further). **Recommendation: A, with a TS/H.264 profile listed second.** The server ranks the
   listed profiles by what it can copy and uses only `ts` profiles for sources marked `UseMostCompatibleTranscodingProfile`
   (`StreamBuilder.GetVideoTranscodeProfile`), so the second profile is its fallback. HEVC keeps 4K transcodes at a reasonable
   bitrate. Consequence: HLS seeking restarts the server's job near the target, which the controller shows as `Seeking`.

3. **The token for the player: `ApiKey` in the URL (recommended) or a header.** The URL form works on every 12.2 server, is what
   `TranscodingUrl`, HLS segments and subtitle URLs already contain, and needs no header that could follow a redirect. A header
   (`Authorization: MediaBrowser Token=...` through `headers`) would serve direct play only, and the server-made URLs would still carry
   `ApiKey`. **Recommendation:** `ApiKey` in the URL, with the redaction rules above (log lines filtered by exact token match and by
   `ApiKey=`/`api_key=`, no `log-file`, no URL properties read, no URL across the bridge). Consequence: the token sits in the URL mpv
   holds in memory, as in every Jellyfin client using these URLs.

4. **Stopping at a redirect off the address.** A (recommended): `max_redirects=0` on every stream, so any redirect fails and the 401
   rule classifies it. B: the core resolves redirects with `current-net` (same-host rule, ADR 0006) and hands mpv the final URL; it
   does not cover HLS segments, which mpv fetches itself. C: no header secrets on streams, cookies only; it leaves Basic gateways
   without playback. **Recommendation: A.** Consequence: a server behind a proxy that redirects streams on purpose (for example to a
   CDN) cannot play until the proxy stops redirecting; none is known for Jellyfin. That `max_redirects=0` reaches HLS segments through
   mpv's option propagation is read from code and checked in task 3.

5. **Gateway cookies for mpv.** A (recommended): FFmpeg's `cookies` option per file through `stream-lavf-o`, nothing on disk.
   B: mpv's `cookies-file` with a temporary file, deleted after loading (a secret on disk, even briefly). C: a loopback relay in the
   core that adds credentials itself (most control, most code; ADR 0017 keeps it for Apple's header gap in M3). **Recommendation: A.**
   Consequence: cookies that the gateway rotates during playback stay in FFmpeg (it updates them per HLS response) and are not read
   back; the core's own requests keep using `current-net`'s cookie updates (ADR 0017 choice 4).

6. **Remembered track choices.** A (recommended): follow the Jellyfin user's own preferences: `AudioLanguagePreference`,
   `PlayDefaultAudioTrack`, `SubtitleLanguagePreference`, `SubtitleMode` and `RememberAudioSelections` /
   `RememberSubtitleSelections` in `UserDto.Configuration`, which the server applies to `DefaultAudioStreamIndex` and
   `DefaultSubtitleStreamIndex`, remembering per user and item from the indexes Current reports (`MediaSourceManager`,
   `SessionManager.UpdatePlaybackSettings`). B: Current's own memory per series in `current-store` (language, forced, hearing
   impaired), applied when the next episode starts. C: A, plus B's series carry-over only when Jellyfin has no remembered index for
   the item. **Recommendation: A** for M2: no second store, and choices follow the user to other clients and devices.
   Consequence: a choice made in episode 1 is not carried to episode 2 unless the user's language preference already picks it; C can be
   added later without changing A.

7. **Progress cadence.** A (recommended): every 10 s, paused or not, plus at once on events. B: every 30 s (fewer requests; a crash
   loses up to 30 s of position; still under the 60 s HLS timer). C: events only plus `Sessions/Playing/Ping` while paused.
   **Recommendation: A**: well under both server timers, small requests. Consequence: 6 requests a minute per player.

8. **Segments and next episode.** A (recommended): a "Skip intro" / "Skip recap" / "Skip preview" button while the position is in such
   a segment, no automatic skipping; at the outro (or the last 30 s without segments) a "Next episode" card, and at the end the next
   episode starts after a 10 s countdown when the Jellyfin user's `EnableNextEpisodeAutoPlay` is on. B: A plus a setting to skip
   intros automatically (off by default). C: buttons only, never autoplay. **Recommendation: A**, with B as a later setting.
   Consequence: without a segment plugin on the server, only the end-of-file card appears.

9. **Fallback after an engine failure.** A (recommended): one automatic re-plan with direct play off, shown as "Transcoding: the player
   could not play this file directly" in playback information. B: ask the user first. **Recommendation: A.** Consequence: a file mpv
   cannot play is transcoded silently; the reason is visible in playback information.

10. **Quality and bitrate.** A (recommended): default "Maximum" (`MaxStreamingBitrate` at the server's ceiling, so the server's
    `RemoteClientBitrateLimit` is the only cap), and a per-playback quality menu with fixed steps (for example 40, 20, 10, 8, 4, 2
    Mbit/s) that re-plans at the position; remembered per server address kind (local or internet) in settings. B: an automatic bitrate
    test (`/Playback/BitrateTest`) before each internet playback. **Recommendation: A**; B later if users on slow links ask.
    Consequence: on a slow internet link the user picks a lower quality by hand.

11. **Refresh-rate switching.** A (recommended): not in M2; mpv gets the display's refresh rate (`display-fps-override`) and
    `display-resample` is evaluated on the owner's displays. B: switch the monitor's mode to the content's rate in full screen
    (Win32 display mode APIs; their behaviour with HDR and several monitors not researched). **Recommendation: A.** Consequence: 23.976
    fps on a 60 Hz display shows the usual judder unless `display-resample` proves good.

12. **Not a choice, recorded:** D6 already settled the mpv release and the two libraries. mpv **0.41.0** is pinned
    (`docs/notes/windows-mpv-build.md`, 2026-10-22: "0.41.0 has `--d3d11-output-mode=composition`, `--d3d11-composition-size` and
    `--display-fps-override`"; confirmed in the v0.41.0 tree on 2026-10-25). **uchardet** is used under LGPL-2.1-or-later (ADR 0003,
    "owner's approval of uchardet, 2026-10-22"). **LAME** is left out (ADR 0003, "owner, 2026-10-22: a player needs no MP3 encoder").
    Roadmap D6: "no LAME; uchardet allowed (owner, 2026-10-22)".

13. **The libmpv binding.** A (recommended): hand-written declarations for the functions used, no new dependency. B: a binding crate
    (`libmpv-sys` or bindgen at build time): a new dependency, needs the owner's approval and a licence check under ADR 0003.
    **Recommendation: A**: the API surface is small and stable (client API 2.5), and every `unsafe` stays visible in our code.
    Consequence: a test that checks our struct layouts against the header on each CI platform.

## Task split for implementation
Each task is its own branch and PR (CLAUDE.md); 1 and 3 can run in parallel, 2 needs 1, 4 needs 2 and 3, 5 needs 4.

1. **`current-model` playback types and `current-jellyfin` playback calls** (feat/jellyfin-playback). PlaybackInfo with the profile,
   the static URL, playstate reports, segments, the session's transcoding facts, the user's playback configuration; the stand-in answers
   them. Done when: wire tests against hand-written 12.2 responses pass (direct play, HLS transcode with reasons in the URL, no
   playable route, segments, a 401), the profile's JSON matches a hand-checked sample, and nothing logged or in Debug contains a token.
2. **`current-playback`** (feat/playback-core). Capabilities, the `Engine` and `MediaServer` traits, planner, controller, tracks,
   segments with the auto-skip setting (decided choice 8), next episode, fallback, cadence. Done when: tests with a fake engine,
   server and clock cover start and resume, every report and its timing, pause and seek reports, track switching on both routes, the
   fallback once and only once, a failure after fallback, segments, auto-skip on and off (once per segment, not again after a seek
   back), the next-episode card and countdown, and the throttled state rate.
3. **`current-mpv` and the Windows libmpv pin** (feat/mpv-engine). The binding, handle, event thread, options, display-fact mapping,
   swap chain handle, capabilities, redaction, and linking against the D6 artifact in CI; the VC++ runtime recorded in ADR 0003 with
   the owner; the approved amendments written into ADR 0017 ("The player") and ADR 0007 ("What M2 must do" item 4). Done when: on the
   Windows CI runner a headless engine (`vo=null`) initialises, reports capabilities that match `check.py`, plays a small generated
   file to the end, reports position and end events, refuses a redirect with `max_redirects=0` on a local stand-in for the playlist
   and for an HLS segment, sends no gateway header after a refused redirect, never shows the test token in a captured log line, and
   verifies TLS (`tls-verify=yes`) against a self-signed stand-in. Tests that need libmpv and cannot run on a platform are `#[ignore]`
   with the reason.
4. **`current-app` + `current-ffi` + C# adapters** (feat/player-bridge). The `Player`, its subscriptions, the swap chain handle with
   `release()`, display facts and composition size, actions with cancellation, `NowPlaying`, playback information; C# records and a
   `PlayerSession` adapter. Done when: Rust bridge tests with a fake engine pass, the C# tests against the stand-in pass in CI (state,
   reporting, release), and no URL crosses the bridge (checked on every record).
5. **Windows player UI, on the owner's PC with real files** (feat/windows-player). `SwapChainPanel` host, Fluent controls, track and
   quality menus, playback information, segment buttons and the next-episode card, the auto-skip switch under Settings' "Playback"
   group, full screen, keyboard, cursor hiding, Narrator,
   SMTC through `GetForWindow`. Done when the owner has checked on a device:
   - direct play, remux and transcode of real library files over the LAN and through a gateway (cookie and Basic), with resume and
     progress seen in the Jellyfin dashboard and on another client;
   - HDR10, HLG, Dolby Vision profiles 5, 7 and 8, and HDR10+ on the HDR monitor, and tone-mapped on the SDR one; moving between them;
   - 23.976, 24, 25 and 60 fps on 60, 120 and 144 Hz, with and without `video-sync=display-resample` (choice 11);
   - audio and subtitle switching on both routes, with Jellyfin's remembered choice on the next start, and that mpv's `ff-index`
     matches Jellyfin's stream index on these files;
   - the fallback on a file mpv cannot play; a gateway expiring during playback; a network drop and recovery;
   - media keys, the flyout's timeline after seeks, full screen on both monitors (ADR 0007's one-row gap fixed), Narrator.

## Consequences
- Playback logic is testable without a server or libmpv, and the next engine (AVPlayer, M3) implements one trait.
- The player never sees a token outside a URL that stays in the engine, never follows a redirect, and never verifies TLS less than the
  core does.
- Direct stream does not exist with Jellyfin 12.2; "Remux" and "Direct stream" are labels for transcode jobs that copy streams.
- Intro and credits skip appear only on servers with a segment plugin.
- External subtitles cost the core one fetch each; mpv fetches only the main stream.

## Amendments to merged ADRs (approved by the owner 2026-10-25; written in task 3)
- **ADR 0017, "The player":** cookies go to FFmpeg's `cookies` option through `stream-lavf-o` per file (mpv's own `cookies` only
  switches its cookie file on); every stream gets `max_redirects=0` and `tls-verify=yes`; external subtitles are fetched by the core.
- **ADR 0007, "What M2 must do" item 4:** done by D6 (mpv 0.41.0, uchardet allowed, no LAME); item 5 is task 5 here.

## Sources (read 2026-10-25)
Quotes and paths in `docs/notes/playback.md`.
- Jellyfin server `v12.2` (commit ca0f16eb): `Jellyfin.Api/Controllers/MediaInfoController.cs`, `PlaystateController.cs`,
  `MediaSegmentsController.cs`, `VideosController.cs`, `DynamicHlsController.cs`, `TvShowsController.cs`,
  `Jellyfin.Api/Helpers/MediaInfoHelper.cs`, `DynamicHlsHelper.cs`, `MediaBrowser.Model/Dlna/` (`DeviceProfile.cs`, `StreamBuilder.cs`,
  `StreamInfo.cs`, profile classes), `MediaBrowser.Model/Dto/MediaSourceInfo.cs`, `MediaBrowser.Model/Session/` (playstate and
  `TranscodeReason`), `MediaBrowser.Model/Configuration/UserConfiguration.cs`, `ServerConfiguration.cs`,
  `MediaBrowser.MediaEncoding/Transcoding/TranscodeManager.cs`, `Emby.Server.Implementations/Session/SessionManager.cs`,
  `Library/UserDataManager.cs`, `Library/MediaSourceManager.cs`, `Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`.
- mpv `v0.41.0` (commit 41f6a645): `include/mpv/client.h`, `DOCS/man/options.rst`, `input.rst`, `mpv.rst`, `stream/stream_lavf.c`,
  `stream/cookies.c`, `demux/demux_lavf.c`, `player/loadfile.c`.
- FFmpeg 9.0.2 (the tarball our build pins): `libavformat/http.c`, `tls.h`, `tls_schannel.c`, `hls.c`, `aviobuf.c`.
- Microsoft Learn, "Manual control of the System Media Transport Controls" (updated 2026-08-27).
- ADR 0003, 0007, 0008, 0011, 0017; `docs/notes/windows-mpv-build.md`, `docs/notes/windows-video.md`, `docs/notes/gateway-auth.md`;
  `build/mpv-windows/check.py`.
