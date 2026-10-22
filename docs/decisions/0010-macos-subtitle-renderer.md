# 0010: macOS subtitles: our own layer over AVPlayer, libass for ASS, our own PGS and VobSub decoder

Status: proposed (2026-10-22), from the D7 spike; accepted when its PR merges. Evidence: docs/notes/macos-subtitles.md.

## Context
ADR 0008 keeps AVPlayer as the macOS engine. It leaves two subtitle gaps that today force a full video transcode (burn-in), which also
loses HDR:
1. Styled ASS/SSA: HLS carries only WebVTT.
2. Bitmap subtitles (PGS, VobSub): AVFoundation opens no container that carries them.

D7 tested our own renderer over AVPlayer on macOS 26.6 (M2) with synthetic media. It was fed the way Jellyfin delivers subtitle
streams, and its sync was measured with frame counters.

## Decision

### One subtitle layer of ours over the video, for every format
- **Layer:** a `CALayer` next to the `AVPlayerLayer` in the same host view, framed to the player layer's `videoRect`, with our
  controls above it. Window-server compositing keeps the subtitle and the video in the same frame.
- **What it draws:**
  - text tracks from `AVPlayerItemLegibleOutput` (ADR 0008);
  - ASS/SSA through libass;
  - PGS and VobSub bitmaps.
- **Keeping clear of our controls:**
  - Unpositioned ASS lines move above the control bar with `ass_set_line_position`; `\pos` signs stay where the script puts them.
  - Bitmaps in the lower half move up only as far as they overlap the bar.
  - The same applies at the top while the window's title bar shows: in a window it overlays the top 28 points of the video and hid a
    PGS sign in the spike.
- **HDR:** subtitles are drawn at SDR reference white (EDR 1.0) in a standard-range layer, not brighter.
  - Apple: "1.0 represents SDR max, also known as 'reference white' or 'UI white'"; "Most user interface elements shouldn't exceed EDR
    1.0" (WWDC21, "Explore HDR rendering with EDR").
  - ITU-R BT.2408 puts graphics white at HDR reference white (203 cd/m² on PQ).
  - The owner found 1.0 comfortable over HDR10 on the built-in display. 2.0 (EDR) was visibly brighter and not wanted.

### The clock: display link plus the player item's video output
- **Clock:** an `NSView.displayLink` callback reads `AVPlayerItemVideoOutput.itemTime(forHostTime: targetTimestamp)`, which is the item
  time of the frame being prepared.
- **Snapping:** the time is snapped to the video's frame grid, and libass renders once per video frame, not once per display refresh.
- **Paused:** the renderer uses `currentTime()` while the rate is 0.
- **Measured** at 25 fps on a 60 Hz display, as the share of screenshots in which the subtitle matched the frame on screen; the rest
  were one frame late (notes, case 5):

| Clock | Steady 1× | Through seek, 2×, 0.5×, pause |
|---|---|---|
| `addPeriodicTimeObserver` 1/60 s | 47 % (ASS), 77 % (PGS), 73 % (HLS) | 0 or −1 frame throughout |
| Display link + `currentTime()` | 69 % (ASS), 64 % (PGS) | 0 or −1; once the new time over the old picture right after a seek |
| Display link + `itemTime(forHostTime:)` | 99 % (ASS, PGS, HLS), 100 % (VobSub) | 97 % (152 of 157 at 0, snapped) |

- **Seeking:** both display-link clocks move to the seek target before the new frame is on screen. One sample in about 150 showed it.
  The engine therefore holds the subtitle from a seek until the seek's completion handler runs. That is not built, and M3 measures
  it with the same method.
- **Owner:** the result felt in sync.

### Rendering: off the main thread, one frame ahead, composited on the GPU
- **Ordinary scripts and bitmaps are cheap:** `styled.ass` added about 7 % of one core over plain playback; PGS added nothing
  measurable.
- **A busy typesetting script is not:** about 45 animated events at once cost 24 ms per changed frame at 2560x1440 and 63 % of a core,
  with a 30 ms maximum on the main thread.
- **Where the time goes:** CPU blending of libass's masks dominated, at about twice `ass_render_frame`'s time.
- **What M3 builds:**
  - libass rendering on a background queue, one video frame ahead of the display link;
  - masks composited on the GPU (Metal), or with vImage;
  - the render size set to the video's size on screen;
  - first use of a fallback font, which cost up to 45 ms, kept off the main thread.

### libass with the CoreText font provider; no fontconfig
- **Font provider:** libass uses CoreText on Apple platforms; fontconfig is neither built nor shipped.
  - libass detects CoreText on macOS, and on iOS through `<CoreText/CoreText.h>` (`configure.ac`, `ass_coretext.c`
    `TARGET_OS_IPHONE`, which covers tvOS).
  - Homebrew builds it with `--disable-fontconfig` on macOS.
  - With no provider at all, CJK, Hebrew and Arabic glyphs had no fallback.
- **Fonts come from three places:**
  - the script's `[Fonts]` section (`ass_set_extract_fonts`);
  - the media's attachments, fetched from Jellyfin's attachments endpoint and added with `ass_add_font`;
  - the system, through CoreText.
- **Two font-selection bugs** are fixed or worked around in M3 before ASS ships:
  - a bold Avenir Next came out italic;
  - CJK ideographs whose CoreText fallback is a hidden system font (PingFang) drew as boxes.

### Libraries, licences and linking (ADR 0003)
- **What ships:**

| Library | Licence | How it ships |
|---|---|---|
| libass | ISC; the arm64 asm file is BSD-2-Clause (its notice ships) | Static or dynamic |
| FreeType | FTL (not GPLv2); the FTL's credit line goes in the documentation and the licences screen | Static or dynamic |
| HarfBuzz | Old MIT | Static or dynamic |
| FriBidi | LGPL-2.1-or-later, the library only (`-Dbin=false` leaves out the GPL getopt files of its tools) | A dynamic framework in `Contents/Frameworks`, with its source and build scripts published (ADR 0003) |
| libunibreak | zlib | Optional: added only if line breaking needs it |

- **Built from source by our own script**, like D6's FFmpeg and libmpv, with features we don't use turned off: no fontconfig, and no
  libpng, bzip2, brotli or glib in FreeType and HarfBuzz.
- **iOS and tvOS** accept dynamic code only as frameworks ("Placing Content in a Bundle"), packaged as an XCFramework of frameworks.
  Whether a framework alone meets LGPL-2.1 §6 in the App Store is contested, so the iOS and tvOS decision also offers relinkable
  objects and is taken when those platforms start.
- **Approval:** every library above still needs the owner's approval and its row in ADR 0003 in the PR that adds it (M3). This record
  proposes them; it adds no dependency.

### PGS and VobSub: our own decoder, not FFmpeg, on Apple platforms
- **Why our own:**
  - macOS ships no FFmpeg (ADR 0008), and libavcodec only for subtitles would mean an LGPL build per Apple platform.
  - The spike's 250-line PGS parser decoded our files exactly as libavcodec did (identical timing and geometry, at most 1/255 per
    channel) and slightly faster (216 vs 256 ms for 1500 display sets).
- **In Rust, in the core** (`current-playback` or a small crate it owns). It turns a PGS `.sup`, or a VobSub stream in Matroska
  (`.mks`, the only form Jellyfin delivers it in), into timed, positioned premultiplied bitmaps. Apple's UIs draw them; any later
  platform without a bitmap renderer reuses it.
- **VobSub needs more:** a minimal Matroska reader for one subtitle track, plus the `dvdsub` RLE and palette from the track's private
  data. That was not written in the spike; libavformat stood in.
- **Fallback:** if the parser proves fragile on disc-authored streams, the alternative is an LGPL libavcodec built with only `pgssub`,
  `dvdsub` and the Matroska demuxer, as a dynamic framework.

### Delivery from Jellyfin
- **Device profile on macOS:** `SubtitleProfiles` with `Method: External` for `ass`, `ssa` and `pgssub`, and for `dvdsub` with
  `Container: mks`.
  - Text formats keep ADR 0008's route: WebVTT renditions in HLS, which AVPlayer also shows over AirPlay.
  - The profile never says `Encode` for a format we can draw, and the client never sends `alwaysBurnInSubtitleWhenTranscoding`.
  - With External delivery, Jellyfin's HLS direct stream keeps copying the video (`EncodingHelper.cs` blocks stream copy only for
    `Encode`; Jellyfin v12.2).
- **Fetching:** the client fetches the `DeliveryUrl` with the codec's own extension (`Stream.ass`, `Stream.sup`, `Stream.mks`).
  - Only an exact format match returns the original script with its styles.
  - Raw PGS and VobSub have range support since 12.2.
  - Fonts come from each attachment's `DeliveryUrl` with our auth header.
- **Known server behaviour the core handles:**
  - The first request for a track reads the whole media file once (slow on large files, cached afterwards).
  - If an admin has turned off subtitle extraction, ASS falls to `Encode`. The planner shows that as burn-in with its reason.
  - The server rewrites `Arial` to `Arial Unicode MS` in extracted ASS.
  - Requirements: client-rendered bitmaps during a remux need Jellyfin 12.0, and ranges need 12.2.

### What the engine tells the core (a proposal for `current-playback`, not code)
- **Each engine declares a subtitle renderer per format,** in its capabilities (an open map; unknown formats pass through):
  - `format`: the server's codec name, e.g. `ass`, `ssa`, `pgssub`, `dvdsub`, `subrip`, `webvtt`, `mov_text`, `dvbsub`.
  - `delivery`: how the engine takes it. `InStream` means the engine reads it from the media itself (tx3g in MP4, WebVTT in HLS);
    `External` means the core hands the engine a file or a byte stream.
  - Flags for what survives PiP and AirPlay.

  On macOS:
  - `ass`, `ssa`, `pgssub`, `dvdsub` (from `mks`): External, not shown in PiP or AirPlay.
  - `webvtt`, `mov_text`: InStream, shown by AVPlayer itself in PiP and AirPlay (to verify).
  - `subrip`: converted by the server to WebVTT in HLS.
- **The planner** builds the device profile's `SubtitleProfiles` from that map. It asks for burn-in only for a selected format with no
  renderer, and states the reason.
- **The controller** owns the track list. Each track has an id, language, title, format, the forced, default and hearing-impaired
  flags, and its source (in the media, server-extracted, or an external file). The controller also owns the selected track and a
  per-track delay.
  - It fetches external tracks and fonts through `current-net`, with auth and the request log. Large bitmap tracks are fetched
    incrementally.
  - It passes the engine bytes, never URLs with tokens.
- **The engine** is told only "show this track (bytes and fonts), with this delay", or "none". It reports a failure to load or render
  as a track error, and the controller falls back: to another track, or to burn-in when the user still wants that one.
- **What stays in the platform UI:** layout, such as bar insets, the title bar and full screen.

### PiP and AirPlay
- **What the user loses:** our layer is not part of the PiP window, and AirPlay sends only the stream URL. So ASS, PGS and VobSub are
  not shown in PiP or on an AirPlay receiver. The owner confirmed that PiP shows the video only.
- **M3 accepts that,** hides our layer during PiP, and tries AVPlayer's own rendering for text tracks during PiP (not verified).
- **The only path that would put our subtitles into PiP:** an `AVSampleBufferDisplayLayer` content source fed with frames we composite
  ourselves (`AVPictureInPictureControllerContentSource`, macOS 12+). It needs a decision of its own.

## Consequences
- The macOS player in M3 needs no burn-in for ASS, PGS or VobSub when the server can extract them, and HDR is kept.
- M3 adds libass, FreeType, HarfBuzz and FriBidi (with the owner's approval), a build script for them, and a PGS/VobSub decoder in the
  core.
- The roadmap's "fallback engine from D7" becomes this renderer. A second engine is still deferred to local and offline playback
  (ADR 0008).
- **Not verified:**
  - an external HDR display, 120 Hz displays, and 23.976 fps;
  - a real Jellyfin server, and disc-authored PGS and VobSub;
  - iOS and tvOS;
  - PiP text rendering, and subtitles over AirPlay;
  - GPU cost in Instruments.
