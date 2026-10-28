# 0025: Native player first, then a remux on the device, then the app engine, then the server

Status: proposed (2026-10-28), from the native-player-first spike. The route order and choices 1 to 3 are the owner's (2026-10-28);
the departures are proposals, each for the owner's approval. Research, quotes and sources: `docs/notes/native-player-first.md`. No code
was written.

## Context
The owner's decision (2026-10-28): "Add a native player route for all the targets as it should've been from the very beginning, remux if
needed. app engine is used as fallback in a situation when native player have no support for this media type and server remux -
transcode as overall fallback." And: the same behaviour as an existing third-party client, "until you can suggest a better way, or
something special that specific platform can or cannot do" (owner, 2026-10-28).

Today:
- **Windows:** libmpv only (ADR 0007, ADR 0019), then the server (remux or transcode, ADR 0019 choice 9).
- **macOS:** AVPlayer, then the server (ADR 0008), with our own subtitle layer for ASS and PGS (ADR 0010); a second engine deferred.
- **Linux:** libmpv (ADR 0011).
- **The core:** one `Engine` per player in `current-playback`, a planner that asks the server once and picks direct play or a server
  stream, and a controller that falls back to the server once (ADR 0019). The supported formats page reads one engine's capabilities
  (ADR 0021).

"Native player" means the platform's decoder and renderer only. Our own controls stay over the video on every route (ADR 0001).

## Decision

### The route order, on every platform
1. **Native:** the platform's player reads the file as it is. Windows: Media Foundation through `Windows.Media.Playback.MediaPlayer`
   in a WinUI 3 `MediaPlayerElement`. macOS, iOS, iPadOS, tvOS: AVPlayer. Linux: GStreamer (`playbin3` into `gtk4paintablesink`).
   Android: Media3 (ExoPlayer). Web: the video element with MSE.
2. **Remux on the device:** the native player takes every selected track but not the container (MKV for AVPlayer, MKV in the
   browser). The core repackages the streams without decoding them (FFmpeg's libavformat) and serves the result to the native player
   through the loopback relay (ADR 0020). Only where a platform needs it (table below).
3. **App engine:** libmpv, our own build, when the native player cannot play a selected track.
4. **The server:** a remux, else a transcode (ADR 0019's server stream), when neither the device nor the app engine can.

| Platform | Native | Remux on the device | App engine | Server |
|---|---|---|---|---|
| Windows | Media Foundation (`MediaPlayer`) | Not needed: MF reads MKV (departure 1) | libmpv (D6, today's engine) | Remux, transcode |
| macOS, iOS, iPadOS, tvOS | AVPlayer | MKV to fMP4 HLS, libavformat | libmpv (new on Apple) | Remux, transcode |
| Linux | GStreamer | Not needed: `matroskademux` | libmpv (ADR 0011) | Remux, transcode |
| Android (later) | Media3 | Not needed: Matroska extractor | libmpv for Android (not researched) | Remux, transcode |
| Web (later) | Video element, MSE | MKV to fMP4, in the browser or on the server | None (departure 7) | Remux, transcode |

### The rule, the existing client and ours
The existing client's behaviour is the default on every platform; where ours differs it is a numbered departure below.

| Rule | The existing client | Windows | Apple | Linux | Web |
|---|---|---|---|---|---|
| Native when it plays every selected track | Yes | Same | Same | Same | Same |
| Remux when only the container is missing | Yes, MKV to its native player | Departure 1 (none needed) | Same | Same (none needed) | Same |
| No remux with a subtitle that needs rendering, or Dolby Vision profile 7 | Yes | Not applicable | Same | Not applicable | Same |
| Its own engine when the native player lacks a track | Yes | Same | Same | Same | Departure 7 (no engine) |
| Picture subtitles (PGS, DVD, DVB, XSUB) on its engine | Yes | Same; departure 3 proposes native | Same; departure 5 proposes ADR 0010's layer | Departure 6 (native) | Same (server) |
| Styled ASS/SSA | Not stated; treated as picture subtitles (inferred) | Same | Same; departure 5 | Departure 6 (native) | Same (server) |
| H.264 High 10 to its engine | Yes (no hardware decoder) | Same, from capabilities | Same | Same | Same |
| TrueHD Atmos to its engine, decoded to PCM; E-AC-3 Atmos native | Yes | Same (objects of E-AC-3 not verified) | Same | Same | Same |
| True Dolby Vision only native or remuxed; base layer on its engine; profiles without a compatible base layer to the server | Yes | Departure 2 | Same | Same | Same |
| Server streams played by the native player | Yes (HLS on AVPlayer) | Departure 4 until fMP4 HLS is checked | Same | Same | Same |
| Playback information names the route and why | Yes | Same | Same | Same | Same |

### How the core decides (`current-playback`)
- **Engines:** a player has up to two engines, the native one and the app engine, each reporting `EngineCapabilities`, and knows
  whether its platform has a device remux and what it takes (containers it demuxes, streams it may copy). The libmpv engine keeps
  ADR 0019's one-engine-per-process slot; the native engine does not use it.
- **The device profile sent to Jellyfin** is the union of what the device plays: the native engine's containers and codecs, the
  containers the remux reads with the native engine's codecs, and the app engine's containers and codecs, subtitles likewise. So the
  server offers direct play whenever any local route plays the file and is asked for a stream only when none does. `TranscodingProfiles`
  stay ADR 0019's (choice 2), in a format the native engine takes where it takes one (departure 4 on Windows).
- **Per file, after PlaybackInfo,** when the answer allows direct play, the planner checks the source's streams (`MediaSource`, ADR
  0019: container, codec, profile, bit depth, range, Dolby Vision profile, channels) and the selected audio and subtitle tracks against
  each route in order:
  1. Native: the container and every selected track are in the native engine's capabilities, and the selected subtitle, if any, is
     one the native route shows (text formats; picture and styled formats per departures 3, 5 and 6).
  2. Remux: as 1, but the container is one the remux reads and the native engine does not; not with Dolby Vision profile 7 or a
     subtitle that needs rendering.
  3. App engine: the container and every selected track are in its capabilities.
  4. None: a new PlaybackInfo with `EnableDirectPlay = false` (ADR 0019), the server stream.
  A server stream is played by the first engine, native first, whose capabilities include the stream's protocol, container and codecs.
- **Track changes:** selecting an audio or subtitle track that the engine playing cannot show re-plans at the position onto the next
  route that can (for example a PGS track on Windows moves the file from Media Foundation to libmpv), as an audio change under a
  server stream already re-plans (ADR 0019, "Tracks"). The UI shows `Preparing`.
- **Fallback on a failed start:** a native start fails when the file does not open or a selected track is not decodable after
  opening: AVPlayer's `isPlayable` / `isDecodable` (ADR 0008), Windows' `SupportInfo.DecoderStatus` other than `FullySupported`, or an
  error before or soon after the first frame. The controller then tries the next route in the order above, each at most once per play,
  ending with the server stream; a failure on the server stream ends in `Failed` (ADR 0019 choice 9, widened from one re-plan to this
  chain). Each step keeps the position and the tracks.
- **Routes and reasons** (`current-model`): `Route` becomes `Native`, `DeviceRemux`, `AppEngine`, or `ServerStream { engine, ... }`
  (today's fields plus which engine plays it). Labels: "Native", "Remux on this device", "App engine", "Server remux", "Server direct
  stream" (video copied), "Server transcode". New reasons (open values, as ADR 0019): `NativeFullyCompatible` ("Native playback is fully
  compatible"), `KeepsOriginalStreams` (the remux: "Keeps the original video and audio"), `NativeLacks { track }` (for example "Windows'
  player has no TrueHD decoder"), `SubtitleNeedsRendering`, `DolbyVisionProfile7`, `PreviousRouteFailed` ("The previous playback route
  could not start"), `UserChoseAppEngine`, `SettingAlwaysAppEngine`. The words are the core's (PR 93's rule: texts in the core).
- **The playback information panel** names the route and its reasons, and each failed route before it.

### How a platform engine plugs into the core
- **Windows and Apple: a UniFFI foreign trait.** `current-ffi` declares `#[uniffi::export(with_foreign)] trait PlatformEngine: Send +
  Sync` (UniFFI's "foreign traits": "UniFFI traits can be implemented by foreign code", manual for 0.31, read 2026-10-28) with the
  `Engine` trait's methods (load, pause, seek, select tracks, volume, stop, add a subtitle as bytes), each returning `Result` so a
  foreign exception becomes an engine error (`From<uniffi::UnexpectedUniFFICallbackError>`, as the manual recommends). The C# or Swift
  object implements it; `current-app` wraps it as a `current_playback::Engine`. Events come back through a Rust object the core hands
  the platform engine (`PlatformEngineEvents::send(EngineEvent)`), the same events libmpv's event thread produces, marshalled by the
  core, never by the UI.
- **Capabilities stay logic in the core.** The UI reports facts: on Windows the `CodecQuery` answers per subtype, on Apple
  `VTIsHardwareDecodeSupported` and the VP9 opt-in (ADR 0008); `current-app` maps them to `EngineCapabilities` with its own tables, as
  ADR 0021 does for the graphics card. The post-open check (decodability per track) is reported by the engine as facts too.
- **URLs across the bridge:** the native engine is in C# or Swift, so for the first time a stream URL crosses `current-ffi`. Every
  stream for a platform engine goes through the loopback relay (ADR 0020), not only those on gateway addresses: the URL then holds the
  relay's route token only, never `ApiKey` (ADR 0020 choice 4 B), and the token dies with the route. The platform engine never logs
  the URL. Apple's AirPlay of relayed URLs fails (ADR 0008); AirPlay is decided with M3 (open point).
- **Subtitles on the native route** are fetched by the core (ADR 0019, "External subtitles") and handed over as bytes; embedded text
  tracks are shown by our own view, so they sit above our control bar: AVPlayer's legible output (ADR 0008) and Windows' timed
  metadata tracks in `ApplicationPresented` mode (notes, "Subtitles").
- **Linux: no bridge.** GStreamer is driven from Rust (gstreamer-rs) in the app or a `current-gst` crate implementing `Engine`
  directly, as libmpv is.
- **The player surface:** the platform's player page hosts both surfaces (Windows: `MediaPlayerElement` and today's `SwapChainPanel`)
  and shows the one of the engine playing; the core says which in the player state.

### Choices decided by the owner (2026-10-28)
1. **A selected subtitle that needs rendering sends the file to the app engine** (picture subtitles, and ASS whose styling must be
   kept; the ASS part inferred from the existing client). Departures 3, 5 and 6 propose keeping some of them native.
2. **Native always first, per selected track, with no other exceptions:** the app engine only when the native player cannot play a
   selected track.
3. **Both a setting and a per-playback choice:** Settings → Playback → "Player": "Automatic" (default) or "Always app engine", stored
   in `settings.json` as an open value (ADR 0013); and "Play with app engine" in the player's menu for this playback only (re-plans at
   the position). With either, the order is app engine, then the server; the reason says why.

### Departures, each for the owner's approval
1. **Windows has no device remux.** What differs: the existing client remuxes MKV for its native player; Media Foundation reads MKV
   itself (H.264, HEVC, VP9, AV1 and others in MKV, "Supported codecs", ms.date 2026-05-12), so step 2 is empty on Windows and no
   libavformat muxer is enabled. Why: nothing to remux into. Source: notes, "Formats Media Foundation plays". Checked on the device in
   task W1. Needs the owner's approval.
2. **Windows: Dolby Vision on libmpv, not the server.** What differs: the existing client shows true Dolby Vision only natively and
   sends a profile without a compatible base layer (profile 5) to the server. On Windows, Media Foundation shows Dolby Vision only with
   the Dolby Vision Extensions on "a Dolby Vision licensed device" (Store page), and through WinUI 3 not verified; libmpv applies the
   Dolby Vision metadata itself and outputs HDR10 or SDR (ADR 0007, ADR 0019). Proposed: Dolby Vision goes native only where task W1
   shows it works with the extension on a licensed display; otherwise every profile (5, 7, 8) goes to libmpv, and never to the server
   for Dolby Vision alone. Why: a server transcode of profile 5 loses the HDR, libmpv keeps it as HDR10. Needs the owner's approval.
3. **Windows: picture subtitles native, if task W1 shows them.** What differs: Windows lists VobSub and PGS among its timed text
   formats and exposes `ImageSubtitle` tracks with `ImageCue` bitmaps, which our view could draw (`ApplicationPresented`). If in-band
   PGS from MKV arrives that way, PGS stays on the native route instead of moving the file to libmpv. Why: no engine switch on a track
   change, Dolby Vision kept with the extension. Not verified; until W1 shows it, the owner's rule applies. Needs the owner's approval.
4. **Windows: server streams on libmpv until fMP4 HLS is checked.** What differs: the existing client plays the server's HLS on its
   native player. Microsoft's HLS tag table marks `EXT-X-MAP` (fMP4's init segment) "Not Supported", up to Windows 10 1607 (ms.date
   2026-02-24, possibly stale), and our transcodes are HLS with fMP4 segments (ADR 0019 choice 2). Proposed: server streams play on
   libmpv on Windows (today's working route) unless task W1 shows `AdaptiveMediaSource` playing Jellyfin's fMP4 HLS through the relay;
   then native first, as the rule says. Needs the owner's approval.
5. **Apple: keep ADR 0010's subtitle layer over AVPlayer for PGS, VobSub and ASS.** What differs: the owner's rule moves those to
   libmpv. ADR 0010 already designed our own layer (libass, the core's PGS decoder) over AVPlayer, which keeps HDR and Dolby Vision and
   PiP for the video. Proposed: M3 follows the rule first (libmpv), and the layer is a later improvement once libmpv works on the Mac.
   Needs the owner's approval for either order.
6. **Linux: ASS and PGS stay on GStreamer.** What differs: GStreamer renders ASS through libass (`assrender`) and PGS (`dvdspu`), so
   the native route keeps them, with no engine switch. Source: GStreamer's plugin documentation (notes, "Linux"). Needs the owner's
   approval (and a check in task L1 that styling and placement over our controls work).
7. **Web: no app engine.** What differs: a browser cannot load libmpv, so its order is native, remux, server. A platform that cannot,
   not a choice; recorded for the owner.

### What each platform gains and loses against today
- **Windows.**
  - Gains: Windows' own decoders and HDR pipeline (and Dolby Vision where the extension and a licensed display allow it, departure 2);
    Windows' spatial sound ("apps that use media foundation can successfully play Dolby Atmos content with no additional
    implementation", for formats not verified); the Store extensions a user has installed.
  - Loses or costs: a second engine to keep in step with libmpv (tracks, events, errors, the info panel, tests); the HDR output is the
    OS's, not ADR 0007's display-fact mapping, and `display-fps-override` has no counterpart; subtitles on the native route need our own
    view; an engine switch (a short `Preparing`) when a track the native player lacks is selected; HEVC and AV1 natively only with the
    Store extensions (HEVC costs 0.99 USD), else libmpv as today. libmpv already plays everything we take, so on Windows the gain is
    mostly platform fit, not formats; said plainly.
- **macOS.** Gains: MKV starts and seeks without a server job (the remux), Dolby Vision 8.1 kept in MKV, local MKV files; AV1 without
  hardware, TrueHD, DTS, multichannel Opus, PGS and styled ASS without a server transcode (libmpv). Loses or costs: FFmpeg and libmpv as
  new dependencies on Apple (LGPL frameworks, ADR 0003, ADR 0010's iOS caveat), a second engine, no PiP or AirPlay on the libmpv route,
  and on the relay (all native streams) no AirPlay of relayed URLs.
- **Linux.** Gains: VA-API through GStreamer's `va` plugin, a possible HDR path (GDK colour states from 4.16) that the libmpv render API
  lacks (ADR 0011), ASS and PGS natively. Loses or costs: GStreamer and the MPL-2.0 `gtk4paintablesink` as dependencies, codec
  availability that differs per distribution (not verified), a second engine.
- **Android, iOS, tvOS, web:** follow from the start, so nothing is lost against today.

### What it amends (written when the owner approves this record)
- **ADR 0001:** wording only: "The player is our own on every platform" gains "over the platform's native decoder and renderer where
  it plays the file, and over our app engine otherwise". No change of meaning.
- **ADR 0007:** libmpv becomes the Windows app engine; the native engine is Media Foundation in `MediaPlayerElement`, with our XAML
  controls over it; the display-fact mapping stays for libmpv only.
- **ADR 0008:** route 2 (server remux for MKV) becomes the device remux; libmpv is the fallback engine, no longer deferred; the
  capability check feeds the fallback chain.
- **ADR 0010:** picture subtitles and styled ASS go to libmpv under the owner's rule; the layer stays proposed as departure 5.
- **ADR 0011:** GStreamer becomes the Linux native engine; libmpv the app engine; its HDR section points at GStreamer's colour states.
- **ADR 0019:** `Route`, `RouteLabel` and `RouteReason` as above; two engines per player; the device profile is the union; choice 9's
  single re-plan becomes the fallback chain; the "URL crosses no bridge" rule gains the relay-only exception for platform engines.
- **ADR 0020:** choice 1 widens: every stream for a platform engine goes through the relay, gateway or not; the relay also serves the
  device remux's playlist and segments (a local source next to `Upstream`).
- **ADR 0021:** the supported formats page shows, per format, the route that plays it ("Windows' player", "App engine", "Server
  converts") and, for a codec Windows' player lacks only for a missing Store extension, "Install HEVC Video Extensions" with a Store
  link (`ms-windows-store://pdp/?ProductId=...`; whether it opens from an unpackaged app is checked in W1).

## Task split for implementation
Each is one branch and PR; the Windows tasks come first ("Next: Jellyfin and playback complete (Windows)"). W1 is a device check on the
owner's PC, as the D2 spike was; the rest waits for its notes.

**Windows**
- **W1** (spike/windows-native-player): a throwaway WinUI 3 prototype (Windows App SDK 2.5.1, unpackaged, as the app) with
  `MediaPlayerElement` under our XAML controls, on the owner's PC. Checks: XAML over the video (no flicker, Narrator); HDR10, HLG,
  HDR10+ and Dolby Vision with and without the Dolby Vision Extensions; MKV with H.264, HEVC (with and without the HEVC extension), VP9,
  AV1; AC-3, E-AC-3 and E-AC-3 JOC on 24H2, TrueHD and DTS (expected to fail), multichannel and spatial sound; in-band SRT, ASS and PGS
  in MKV and what reaches `ApplicationPresented`; `DecoderStatus` on an unsupported track; `CodecQuery` from an unpackaged app,
  including Store codecs; the Store link; progressive and HLS fMP4 through the relay on 127.0.0.1; SMTC with `CommandManager` off,
  CompactOverlay, track switching, speed. Ends in notes and a revision of this record (departures 1 to 4 answered).
- **W2** (feat/routes-and-engines): `current-model` routes and reasons, two engines per player, the per-file route check, the device
  profile union, the fallback chain, track changes across engines, the "Player" setting and "Play with app engine"; tested with fake
  engines. No UI.
- **W3** (feat/platform-engine-bridge): the `PlatformEngine` foreign trait and its events in `current-ffi`, the capability facts, every
  platform-engine stream through the relay; C# adapters and bindings tests with a fake C# engine against the stand-in.
- **W4** (feat/windows-native-engine): the C# Media Foundation engine, the player page with both surfaces, the decodability check,
  SMTC unchanged; smoke run on CI with the stand-in.
- **W5** (feat/windows-native-subtitles): our subtitle view for the native route's text cues (and image cues if departure 3 is
  approved), above the control bar.
- **W6** (feat/formats-per-route): the supported formats page per route, missing Store extensions with their link (ADR 0021).
- **W7** (device check, by hand): the owner plays real files on each route, sees the route and reasons in the info panel, and the
  fallback chain on a file that fails natively.

**macOS (M3)**
- **M3-1:** AVPlayer as a `PlatformEngine` in Swift (ADR 0008's engine on the new trait).
- **M3-2** (spike/macos-device-remux): libavformat MKV to fMP4 HLS served by the relay: segmenting on the source's keyframes (Matroska
  cues), seeking, `hvc1`, Dolby Vision boxes (`-strict unofficial`), audio AVPlayer lacks, the LGPL build on macOS; an ADR.
- **M3-3** (spike/macos-libmpv): libmpv on macOS under our SwiftUI controls (render API or `gpu-next`), HDR/EDR, our LGPL build; an ADR.
- **M3-4, M3-5:** the device remux and the libmpv engine on macOS, from those ADRs.

**Linux (M5)**
- **L1** (spike/linux-gstreamer): `playbin3` into `gtk4paintablesink` under our GTK controls: VA-API, HDR through GDK colour states,
  `assrender` and PGS (departure 6), MPRIS unchanged; an ADR (amends ADR 0011).
- **L2:** the GStreamer engine implementing `Engine` in Rust; libmpv stays the app engine.

**Later platforms:** Android (Media3 first, libmpv for Android as the app engine, a spike first), iOS and tvOS (macOS's engines,
remux and libmpv; the LGPL framework question of ADR 0010), the web (MSE, the remux in WebAssembly or on the server, no app engine).

## New dependencies (for the owner's approval; none added here)
The full table with licences is in the notes ("New dependencies"). In short:
- **Windows:** none new. `Windows.Media.*` and `MediaPlayerElement` come with the Windows SDK and Windows App SDK already referenced
  (approved in advance, CLAUDE.md). The Store codec extensions are installed by the user, never shipped.
- **Apple:** FFmpeg's libavformat, libavcodec and libavutil (our LGPL build) for the remux; libmpv and its libraries (our LGPL build)
  for the app engine, plus its macOS renderer's needs (not decided; MoltenVK, Apache-2.0, if Vulkan).
- **Linux:** GStreamer core, base, good and bad as system libraries (LGPL-2.1-or-later); gstreamer-rs 0.25.x (MIT OR Apache-2.0);
  `gst-plugin-gtk4` (MPL-2.0, which ADR 0003 must rule on).
- **Android, web:** Media3 (Apache-2.0); libavformat in WebAssembly (LGPL), when those platforms start.

## Consequences
- Every platform has a native engine and, except the web, libmpv: two engines to keep in step, tested through one `Engine` trait and
  fake engines in the core.
- The device profile asks the server for less: only what no local route plays.
- Platform-engine streams always go through the relay, so stream URLs crossing the bridge hold no session token.
- The info panel can always say which route played and why.
- **Not verified** (each in the notes, and in task W1 for Windows): Store codecs and `CodecQuery` in an unpackaged app; HDR and Dolby
  Vision in WinUI 3's `MediaPlayerElement`; HLG and HDR10+ in Media Foundation; E-AC-3 JOC objects; fMP4 HLS in
  `AdaptiveMediaSource`; in-band PGS and ASS styling in Media Foundation; XAML over `MediaPlayerElement`; loopback and range requests
  from Media Foundation; MKV chapters; GTK's HDR output end to end; libmpv's rendering on macOS.

## Sources
In `docs/notes/native-player-first.md`, each with its address and date (read 2026-10-28). The UniFFI manual, "Foreign traits":
https://mozilla.github.io/uniffi-rs/latest/foreign_traits.html and `docs/manual/src/foreign_traits.md` at tag v0.31.0 (read
2026-10-28).
