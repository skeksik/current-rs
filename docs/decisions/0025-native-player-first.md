# 0025: Native player first, then a remux on the device, then the app engine, then the server

Status: accepted (2026-10-28, owner), from the native-player-first spike. The route order, choices 1 to 3 and departures 1 to 7
are the owner's (2026-10-28). Revised by task W1 (2026-10-28): departures 1 to 3 checked on the owner's PC and kept, departure 4
dropped, departure 8 and the speed rule added (owner, 2026-10-28). Research, quotes and sources: `docs/notes/native-player-first.md`;
the device check: `docs/notes/windows-native-player.md`. No product code was written. Choice 3 amended after W7 (owner, 2026-10-31;
"How to play"), pending the owner's approval of the amended text.

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
| Picture subtitles (PGS, DVD, DVB, XSUB) on its engine | Yes | Departure 3 (PGS native); DVD, DVB, XSUB same | Same; departure 5 proposes ADR 0010's layer | Departure 6 (native) | Same (server) |
| Styled ASS/SSA | Not stated; treated as picture subtitles (inferred) | Same | Same; departure 5 | Departure 6 (native) | Same (server) |
| H.264 High 10 to its engine | Yes (no hardware decoder) | Same, from capabilities | Same | Same | Same |
| TrueHD Atmos to its engine, decoded to PCM; E-AC-3 Atmos native | Yes | Same (objects of E-AC-3 not verified) | Same | Same | Same |
| True Dolby Vision only native or remuxed; base layer on its engine; profiles without a compatible base layer to the server | Yes | Departure 2 | Same | Same | Same |
| PQ video (HDR10, HDR10+, Dolby Vision) on an SDR display | Not stated | Departure 8 (app engine) | Not checked | Not checked | Not checked |
| A speed the native player ignores (E-AC-3 Atmos on Windows) | Not stated | Re-plans to the app engine ("Track changes") | Not checked | Not checked | Not checked |
| Server streams played by the native player | Yes (HLS on AVPlayer) | Same (fMP4 HLS checked in W1) | Same | Same | Same |
| Playback information names the route and why | Yes | Same | Same | Same | Same |

### How the core decides (`current-playback`)
- **Engines:** a player has up to two engines, the native one and the app engine, each reporting `EngineCapabilities`, and knows
  whether its platform has a device remux and what it takes (containers it demuxes, streams it may copy). The libmpv engine keeps
  ADR 0019's one-engine-per-process slot; the native engine does not use it.
- **The device profile sent to Jellyfin** is the union of what the device plays: the native engine's containers and codecs, the
  containers the remux reads with the native engine's codecs, and the app engine's containers and codecs, subtitles likewise. So the
  server offers direct play whenever any local route plays the file and is asked for a stream only when none does. `TranscodingProfiles`
  stay ADR 0019's (choice 2), in a format the native engine takes where it takes one (on Windows, HLS with fMP4 segments: checked in
  W1).
- **Per file, after PlaybackInfo,** when the answer allows direct play, the planner checks the source's streams (`MediaSource`, ADR
  0019: container, codec, profile, bit depth, range, Dolby Vision profile, channels) and the selected audio and subtitle tracks against
  each route in order:
  1. Native: the container and every selected track are in the native engine's capabilities, and the selected subtitle, if any, is
     one the native route shows (text formats; picture and styled formats per departures 3, 5 and 6), and the platform's display
     rules allow it (departures 2 and 8 on Windows: they read the source's range and Dolby Vision profile and the display facts the
     UI reports).
  2. Remux: as 1, but the container is one the remux reads and the native engine does not; not with Dolby Vision profile 7 or a
     subtitle that needs rendering.
  3. App engine: the container and every selected track are in its capabilities.
  4. None: a new PlaybackInfo with `EnableDirectPlay = false` (ADR 0019), the server stream.
  A server stream is played by the first engine, native first, whose capabilities include the stream's protocol, container and codecs.
- **Track changes:** selecting an audio or subtitle track that the engine playing cannot show re-plans at the position onto the next
  route that can (for example a PGS track on Windows moves the file from Media Foundation to libmpv), as an audio change under a
  server stream already re-plans (ADR 0019, "Tracks"). The UI shows `Preparing`. The same holds for a **speed** the playing engine
  ignores: on Windows, Media Foundation ignores any rate other than 1 while an E-AC-3 Atmos (JOC) track plays (W1), so a speed change
  there re-plans to libmpv at the position, keeping the track (owner, 2026-10-28). The native engine reports which rates it takes as
  a fact (`IsSupportedPlaybackRateRange`), and the core decides. A **display change** during playback (the window moves to an SDR
  display, or Windows HDR is switched) re-plans the same way when departure 8 changes the route.
- **Fallback on a failed start:** a native start fails when the file does not open or a selected track is not decodable after
  opening: AVPlayer's `isPlayable` / `isDecodable` (ADR 0008), or an error before or soon after the first frame. On Windows `DecoderStatus`
  alone is not enough (W1): a native start has failed when a selected track's `DecoderStatus` is not `FullySupported`, a selected
  track raises `OpenFailed` (a missing decoder in MKV leaves the file `Playing` with that track silent or blank), or `MediaFailed`
  arrives soon after opening (H.264 High 10 reports `FullySupported` and then fails with `DecodingError`). The controller then tries the next route in the order above, each at most once per play,
  ending with the server stream; a failure on the server stream ends in `Failed` (ADR 0019 choice 9, widened from one re-plan to this
  chain). Each step keeps the position and the tracks.
- **Routes and reasons** (`current-model`): `Route` becomes `Native`, `DeviceRemux`, `AppEngine`, or `ServerStream { engine, ... }`
  (today's fields plus which engine plays it). Labels: "Native", "Remux on this device", "App engine", "Server remux", "Server direct
  stream" (video copied), "Server transcode". New reasons (open values, as ADR 0019): `NativeFullyCompatible` ("Native playback is fully
  compatible"), `KeepsOriginalStreams` (the remux: "Keeps the original video and audio"), `NativeLacks { track }` (for example "Windows'
  player has no TrueHD decoder"), `SubtitleNeedsRendering`, `DolbyVisionProfile7`, `PreviousRouteFailed` ("The previous playback route
  could not start"), `UserChoseAppEngine`, `SettingAlwaysAppEngine`, and from W1 `DolbyVisionOnAppEngine` (departure 2),
  `HdrOnSdrDisplay` (departure 8), `SpeedNotSupported` (a speed the native player ignores). The words are the core's (PR 93's rule: texts in the core).
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
   selected track. The owner added, after W1 (2026-10-28): the native player also cannot "play" PQ video on an SDR display on Windows
   (departure 8) or a speed it ignores ("Track changes").
3. **Both a setting and a per-playback choice:** Settings → Playback → "Player": "Automatic" (default) or "Always app engine", stored
   in `settings.json` as an open value (ADR 0013); and "Play with app engine" in the player's menu for this playback only (re-plans at
   the position). With either, the order is app engine, then the server; the reason says why.
   **Amended (owner, 2026-10-31, after W7; for the owner's approval of this text):** "Replace Always play with app engine setting with
   a setting that allow you to choose how to play (auto, native, remux, app engine, transcode) if chosen way is unsupported by playback,
   it should fallback to the supported that is below chosen, user should be able to set this setting globally in settings and for the
   current playback in player", and earlier: "should not automatically switch if it's chosen" (W7 notes, proposal 2). As decided with
   the manager the same day:
   - **"How to play"**: Auto, Native, Device remux, App engine, Server remux, Server transcode. Auto is the order above with its
     automatic re-plans. Any other choice starts at that step and falls only down the order native → device remux → app engine →
     server remux → server direct stream (video copied, audio converted; a step, not a choice) → server transcode, never above it.
   - With a choice other than Auto nothing moves back up: a display change that would let native play again re-plans nothing. When
     the route playing stops working (PQ on an SDR display, a track it cannot play, a speed, a failed start) it moves down to the next
     step that plays, and the reasons say the chosen step could not play this ("chosen_route_unavailable") before why.
   - Server remux asks the server for its stream without direct play and takes what it sends (a remux, else a direct stream or a
     transcode, which then reads as fallen); server transcode also refuses video stream copy (`AllowVideoStreamCopy` false: at our
     Jellyfin baseline v12.2, commit ca0f16eb, and at v12.3, 06de749d, it turns direct stream off and `StreamBuilder` copies no
     video; `MediaInfoHelper.cs` and `StreamBuilder.cs`, read 2026-10-31), so the server converts the video.
   - Set globally in Settings → Playback, where it replaces "Player" and keeps its stored values ("automatic", "always_app_engine"),
     and in the player for the current item only; the next item uses Settings' again. Choosing in the player re-plans at once at the
     position, Auto included. The player offers Auto and the steps possible for the file (a local step whose route the check puts
     first, the server's by its permissions); Device remux only where the platform has one (not on Windows, departure 1).

### Departures (approved by the owner, 2026-10-28)
1. **Windows has no device remux.** What differs: the existing client remuxes MKV for its native player; Media Foundation reads MKV
   itself (H.264, HEVC, VP9, AV1 and others in MKV, "Supported codecs", ms.date 2026-05-12), so step 2 is empty on Windows and no
   libavformat muxer is enabled. Why: nothing to remux into. Source: notes, "Formats Media Foundation plays". Approved by the owner
   (2026-10-28). **Checked on the owner's PC (2026-10-28), kept:** MKV with H.264, HEVC, VP9 and AV1 (with the Store extensions), MP4,
   TS and WebM all played natively (W1 notes, §2).
2. **Windows: Dolby Vision prefers libmpv.** What differs: the existing client shows true Dolby Vision on its native player or its
   remux and sends a profile without a compatible base layer (profile 5) to the server. On Windows, Media Foundation shows Dolby Vision
   only with the Dolby Vision Extensions on "a Dolby Vision licensed device" (Store page), Microsoft's requirement for external HDR
   displays is HDR10, and through WinUI 3 it is not verified; elsewhere it would show only the base layer (HDR10 with static metadata
   for profile 8.1, nothing usable for profile 5; inferred). libplacebo, libmpv's renderer, reads the Dolby Vision metadata and
   reshapes each frame ("including Profile 5 conversion to HDR/PQ or SDR", its README), and mpv keeps that metadata by default.
   Proposed: on Windows a Dolby Vision file (profiles 5, 7, 8) goes to libmpv first, and never to the server for Dolby Vision alone;
   native only on a display Windows reports in Dolby Vision mode with the extension installed, if task W1 or a later check shows it
   works. Why: a better picture than the base layer on the HDR10 monitors most users have, and no server transcode that drops HDR.
   The owner's PC (an HDR10-only DisplayHDR 400 monitor and an SDR one) can compare libmpv's conversion with Media Foundation's base
   layer by eye, HDR10+ likewise; a Dolby Vision certified built-in laptop screen is not testable by the owner. Sources: notes,
   "HDR". Approved by the owner (2026-10-28). **Checked on the owner's PC (2026-10-28), kept** (W1 notes, §4): with the Dolby Vision
   Extensions, Media Foundation did apply profile 5's reshaping on the HDR10-only monitor and on the SDR one, and profile 8.1 with
   HDR10+ looked the same as libmpv on the HDR monitor (owner). But without the extension profile 5 shows only its base layer (wrong
   colours) with `DecoderStatus` `FullySupported` and no error; the extension adds no `CodecQuery` entry, so the core cannot tell
   whether it is installed; profile 5 in MKV failed natively even with it (`MF_E_INVALID_FORMAT`); and on an SDR display Media
   Foundation clips (departure 8). libmpv showed the same DV 5 file right. So Dolby Vision goes to libmpv first, as approved; the
   native exception for a display in Dolby Vision mode stays untested (no such display).
3. **Windows: picture subtitles native, if task W1 shows them.** What differs: Windows lists VobSub and PGS among its timed text
   formats and exposes `ImageSubtitle` tracks with `ImageCue` bitmaps, which our view could draw (`ApplicationPresented`). If in-band
   PGS from MKV arrives that way, PGS stays on the native route instead of moving the file to libmpv. Why: no engine switch on a track
   change, Dolby Vision kept with the extension. Approved by the owner (2026-10-28). **Checked on the owner's PC (2026-10-28), kept
   for PGS** (W1 notes, §6): in-band PGS from MKV arrives in `ApplicationPresented` mode as `ImageCue`s, each a full-canvas 1920×1080
   premultiplied BGRA bitmap at position 0,0 and extent 100 %, which our layer drew in place over a 4K video. DVD subtitles (VobSub)
   are exposed as an `ImageSubtitle` track, but no cue was seen: they follow the owner's rule (libmpv) until a check shows their cues.
   DVB and XSUB were not checked and follow the rule.
4. **Dropped (owner, 2026-10-28).** It kept server streams on libmpv until fMP4 HLS was checked, because Microsoft's HLS tag table
   marks `EXT-X-MAP` "Not Supported" (ms.date 2026-02-24). Checked on the owner's PC (2026-10-28): `AdaptiveMediaSource` played FFmpeg's
   fMP4 HLS, Jellyfin's fMP4 transcode and Jellyfin's fMP4 remux of HEVC HDR10 through 127.0.0.1, seeks included (W1 notes, §7). So
   server streams follow the rule: native first. One caveat for W4: when Jellyfin needed more than about 10 s for the first segment
   after a far seek, Media Foundation gave up with `MF_E_NET_TIMEOUT` and asked again; the engine needs its own deadline there and
   falls back to libmpv when it passes.
5. **Apple: keep ADR 0010's subtitle layer over AVPlayer for PGS, VobSub and ASS.** What differs: the owner's rule moves those to
   libmpv. ADR 0010 already designed our own layer (libass, the core's PGS decoder) over AVPlayer, which keeps HDR and Dolby Vision and
   PiP for the video. Proposed: M3 follows the rule first (libmpv), and the layer is a later improvement once libmpv works on the Mac.
   Approved by the owner (2026-10-28): the rule first, the layer later.
6. **Linux: ASS and PGS stay on GStreamer.** What differs: GStreamer renders ASS through libass (`assrender`) and PGS (`dvdspu`), so
   the native route keeps them, with no engine switch. Source: GStreamer's plugin documentation (notes, "Linux"). Approved by the
   owner (2026-10-28), with a check in task L1 that styling and placement over our controls work.
7. **Web: no app engine.** What differs: a browser cannot load libmpv, so its order is native, remux, server. A platform that cannot,
   not a choice. Approved by the owner (2026-10-28).
8. **Windows: PQ video on an SDR display goes to libmpv.** What differs: the rule plays it natively. On the owner's SDR monitor
   (2026-10-28) Media Foundation did not tone-map PQ video: a PQ ramp turned white at about 78 nit, next to the display's SDR white of
   80 nit, a 203-nit test pattern was blown out, a real HDR10 frame was bright and washed out next to libplacebo's tone-mapped render,
   and HDR10+ made no difference (W1 notes, §4). libmpv tone-maps to the SDR display (D2, ADR 0007). HLG looked right natively there,
   so it is not included. Rule: when the display facts the UI reports say the window's display is SDR (`AdvancedColorKind` other than
   `HighDynamicRange`), a source with PQ transfer (HDR10, HDR10+, Dolby Vision) plays on libmpv first; on an HDR display the rule
   applies unchanged (HDR10 and HLG looked right natively there, owner). A display change during playback re-plans ("Track changes").
   Reason `HdrOnSdrDisplay`. Approved by the owner (2026-10-28), as "HDR video on an SDR display", then limited to PQ by the HLG
   result (owner, 2026-10-28).

### What each platform gains and loses against today
- **Windows.**
  - Gains: Windows' own decoders and HDR pipeline on HDR displays (Dolby Vision stays on libmpv, departure 2; PQ video on SDR displays
    too, departure 8); Windows' spatial sound ("apps that use media foundation can successfully play Dolby Atmos content with no
    additional implementation"; whether E-AC-3 Atmos keeps its objects was not verified in W1); the Store extensions a user has
    installed.
  - Loses or costs: a second engine to keep in step with libmpv (tracks, events, errors, the info panel, tests); the HDR output is the
    OS's, not ADR 0007's display-fact mapping, and `display-fps-override` has no counterpart; subtitles on the native route need our own
    view; an engine switch (a short `Preparing`) when a track the native player lacks is selected; HEVC and AV1 natively only with the
    Store extensions (HEVC costs 0.99 USD), else libmpv as today; AC-3 and E-AC-3 natively only where the PC maker installed a Dolby
    decoder (W1: the owner's PC has one); TrueHD, DTS, multichannel Opus and H.264 High 10 always on libmpv (W1). libmpv already plays
    everything we take, so on Windows the gain is mostly platform fit, not formats; said plainly.
- **macOS.** Gains: MKV starts and seeks without a server job (the remux), Dolby Vision 8.1 kept in MKV, local MKV files; AV1 without
  hardware, TrueHD, DTS, multichannel Opus, PGS and styled ASS without a server transcode (libmpv). Loses or costs: FFmpeg and libmpv as
  new dependencies on Apple (LGPL frameworks, ADR 0003, ADR 0010's iOS caveat), a second engine, no PiP or AirPlay on the libmpv route,
  and on the relay (all native streams) no AirPlay of relayed URLs.
- **Linux.** Gains: VA-API through GStreamer's `va` plugin, a possible HDR path (GDK colour states from 4.16) that the libmpv render API
  lacks (ADR 0011), ASS and PGS natively. Loses or costs: GStreamer and the MPL-2.0 `gtk4paintablesink` as dependencies, codec
  availability that differs per distribution (not verified), a second engine.
- **Android, iOS, tvOS, web:** follow from the start, so nothing is lost against today.

### What it amends (written by the task that implements each part)
- **ADR 0001:** wording only: "The player is our own on every platform" gains "over the platform's native decoder and renderer where
  it plays the file, and over our app engine otherwise". No change of meaning.
- **ADR 0007:** libmpv becomes the Windows app engine; the native engine is Media Foundation in `MediaPlayerElement`, with our XAML
  controls over it; the display-fact mapping stays for libmpv only.
- **ADR 0008:** route 2 (server remux for MKV) becomes the device remux; libmpv is the fallback engine, no longer deferred; the
  capability check feeds the fallback chain.
- **ADR 0010:** picture subtitles and styled ASS go to libmpv under the owner's rule; the layer comes later (departure 5).
- **ADR 0011:** GStreamer becomes the Linux native engine; libmpv the app engine; its HDR section points at GStreamer's colour states.
- **ADR 0019:** `Route`, `RouteLabel` and `RouteReason` as above; two engines per player; the device profile is the union; choice 9's
  single re-plan becomes the fallback chain; the "URL crosses no bridge" rule gains the relay-only exception for platform engines.
- **ADR 0020:** choice 1 widens: every stream for a platform engine goes through the relay, gateway or not; the relay also serves the
  device remux's playlist and segments (a local source next to `Upstream`).
- **ADR 0021:** the supported formats page shows, per format, the route that plays it ("Windows' player", "App engine", "Server
  converts") and, for a codec Windows' player lacks only for a missing Store extension, "Install HEVC Video Extensions" with a Store
  link (`ms-windows-store://pdp/?ProductId=...`; it opened from the unpackaged prototype, and `CodecQuery` names the missing
  extension, W1). The Dolby Vision Extensions cannot be detected that way.

## Task split for implementation
Each is one branch and PR; the Windows tasks come first ("Next: Jellyfin and playback complete (Windows)"). W1 is a device check on the
owner's PC, as the D2 spike was; the rest waits for its notes.

**Windows**
- **W1** (spike/windows-native-player): a throwaway WinUI 3 prototype (Windows App SDK 2.5.1, unpackaged, as the app) with
  `MediaPlayerElement` under our XAML controls, on the owner's PC. Checks: XAML over the video (no flicker, Narrator); HDR10 and HLG;
  Dolby Vision (profiles 5 and 8.1) and HDR10+ compared by eye between Media Foundation and libmpv on the HDR10-only monitor and the
  SDR one (does either route use the dynamic metadata for tone mapping on a display of about 400 nit?), with and without the Dolby
  Vision Extensions (true Dolby Vision is not testable on the owner's displays); MKV with H.264, HEVC (with and without the HEVC extension), VP9,
  AV1; AC-3, E-AC-3 and E-AC-3 JOC on 24H2, TrueHD and DTS (expected to fail), multichannel and spatial sound; in-band SRT, ASS and PGS
  in MKV and what reaches `ApplicationPresented`; `DecoderStatus` on an unsupported track; `CodecQuery` from an unpackaged app,
  including Store codecs; the Store link; progressive and HLS fMP4 through the relay on 127.0.0.1; SMTC with `CommandManager` off,
  CompactOverlay, track switching, speed. Ends in notes and a revision of this record (departures 1 to 4 answered). Done
  (2026-10-28): `docs/notes/windows-native-player.md`.
- **W2** (feat/routes-and-engines): `current-model` routes and reasons, two engines per player, the per-file route check (with the
  display rule of departures 2 and 8 from display facts), the device profile union, the fallback chain, track, speed and display
  changes across engines, the "Player" setting and "Play with app engine"; tested with fake engines. No UI.
- **W3** (feat/platform-engine-bridge): the `PlatformEngine` foreign trait and its events in `current-ffi`, the capability facts, every
  platform-engine stream through the relay; C# adapters and bindings tests with a fake C# engine against the stand-in.
- **W4** (feat/windows-native-engine): the C# Media Foundation engine, the player page with both surfaces, the decodability check
  (`DecoderStatus`, each selected track's `OpenFailed`, `MediaFailed` soon after opening; W1), tracks matched by their `Id` (the index
  order of timed metadata tracks changes between opens, W1), the supported-rate fact, HLS through `AdaptiveMediaSource` with a
  deadline of its own for a slow first segment, SMTC unchanged; smoke run on CI with the stand-in.
- **W5** (feat/windows-native-subtitles): our subtitle view for the native route's text cues (SRT subformats; ASS's static styling as
  far as it arrives) and PGS image cues (full-canvas bitmaps scaled to the video, departure 3), above the control bar.
- **W6** (feat/formats-per-route): the supported formats page per route, missing Store extensions with their link (ADR 0021), found
  through `CodecQuery` per subtype (W1).
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
- **Checked on the owner's PC in W1 (2026-10-28):** Store codecs and `CodecQuery` in an unpackaged app, the Store link, HDR10, HLG and
  Dolby Vision in WinUI 3's `MediaPlayerElement`, fMP4 HLS in `AdaptiveMediaSource`, in-band PGS and ASS in Media Foundation, XAML over
  `MediaPlayerElement`, loopback and range requests, MKV chapters (`docs/notes/windows-native-player.md`).
- **Not verified:** E-AC-3 JOC objects through Windows' spatial sound; HDR10+ dynamic metadata on an HDR display (ignored on an SDR
  one); DVD subtitle cues; Dolby Vision profile 7; true Dolby Vision output; multichannel speakers; GTK's HDR output end to end;
  libmpv's rendering on macOS.

## Sources
In `docs/notes/native-player-first.md`, each with its address and date (read 2026-10-28); the device check and its sources in
`docs/notes/windows-native-player.md`. The UniFFI manual, "Foreign traits":
https://mozilla.github.io/uniffi-rs/latest/foreign_traits.html and `docs/manual/src/foreign_traits.md` at tag v0.31.0 (read
2026-10-28).
