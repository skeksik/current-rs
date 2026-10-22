# D7 spike: macOS subtitle renderer over AVPlayer, notes

Decision: ADR 0010. Run on 2026-10-22. The prototype (a SwiftUI app, a command-line tool and measurement scripts) is in commit
`8b8b2acc18ca779cba4ea9b10ce37e940dd6fb6d` on `spike/macos-subtitles` (`spikes/macos-subtitles/`); it is deleted before the PR
merges.

## Setup
| Item | Value |
|---|---|
| Mac | Mac14,15 (MacBook Air 15-inch, Apple M2) |
| macOS | 26.6.2 (25G83) |
| Xcode / SDK / Swift | Xcode 27.1 (27A9275), macOS 27.0 SDK (26A425), Swift 6.4, Swift 6 language mode, deployment target macOS 26 |
| Display | Built-in Liquid Retina, 2880x1864, 60 Hz, EDR potential 2.0 (D3). The external HDR display was not connected; no 120 Hz display |
| Libraries (prototype only, from Homebrew, never shipped) | libass 0.17.5_1 (logs "FriBidi 1.0.16 (SIMPLE) HarfBuzz-ng 14.4.0 (COMPLEX)"), fribidi 1.0.17, libunibreak 8.0, freetype 2.14.3, harfbuzz 14.4.0; FFmpeg 9.0.2 (GPL build) for libavformat/libavcodec and for making test media. `brew install libass` (owner's approval) also upgraded brotli 1.2.0 to 1.2.1 |
| Font | Pacifico Regular, SIL OFL 1.1, from github.com/google/fonts `ofl/pacifico` at commit `45e3d52`, not installed on the system (owner's approval). Kept out of git |
| Build | SwiftPM, release, wrapped into an ad-hoc signed `.app`; not sandboxed |

## What was built
- **`SubtitleOverlay`**: a `CALayer` added next to the `AVPlayerLayer` in the same `NSView`, so the window server composites both in
  one frame. Its frame is the player layer's `videoRect` with `isGeometryFlipped`. One content layer holds the current subtitle image,
  sized to the union of the subtitle's bitmaps. The SwiftUI control bar lies above it. The bar fades by opacity, as ADR 0008 requires.
- **ASS** (`AssRenderer`): libass reads the whole script (`ass_read_memory`), the way Jellyfin delivers it. Fonts embedded in the script
  load with `ass_set_extract_fonts`, and fonts from MKV attachments with `ass_add_font`. The CoreText provider is used, or none for
  comparison. libass renders at the video rect's size in pixels (points × backing scale). Its `ASS_Image` list (8-bit coverage
  masks plus an RGBA colour) is blended on the CPU into a premultiplied BGRA buffer, which is set as the layer's `contents`. Only
  frames that libass reports as changed are re-blended. `ass_set_line_position` lifts unpositioned lines above the bar.
- **PGS and VobSub** (`BitmapTrack`): decoded up front, either with libavformat and libavcodec (`pgssub`, `dvdsub`) or, for PGS, with a
  250-line Swift parser (`PGS.read`). Each display set becomes premultiplied bitmaps on the subtitle's canvas, scaled to the video rect
  when drawn. Bitmaps in the lower half move up only as far as they overlap the bar.
- **Three clocks**, switchable at run time:
  - *periodic*: `addPeriodicTimeObserver` every 1/60 s on the main queue.
  - *link*: `NSView.displayLink` (macOS 14+) calling `AVPlayer.currentTime()`.
  - *output*: the same display link calling `AVPlayerItemVideoOutput.itemTime(forHostTime: link.targetTimestamp)`.
- **Test media** (`make-media.sh`, all synthetic):
  - 1080p25 H.264 with a frame counter, and an HLS/fMP4 copy of it served from 127.0.0.1.
  - HDR10 1080p25 HEVC (PQ ramp), and busy-script videos at 1080p H.264 and 2160p HEVC.
  - `styled.ass`, hand-written, one feature per cue, also with Pacifico embedded as a `[Fonts]` section.
  - A busy script: 400 events, about 45 on screen at once, with `\move`, `\t` rotation and scale, `\blur`, `\be`, `\kf`, `\fad`,
    `\clip` and large blurred boxes.
  - PGS `.sup` from our own encoder (FFmpeg has none), with text drawn by CoreText.
  - VobSub converted from that PGS by FFmpeg (`-c:s dvdsub`) into `.mks`, the container Jellyfin extracts VobSub to.
  - An MKV with the ASS and the font as an attachment, extracted again the way Jellyfin does it (below).

### Sync measurement (repeatable in M3)
- The video carries its frame number N as 16 black/white blocks (bit k at x = k/16 of the width, 1/27 of the height tall, four block
  heights from the top, clear of the title bar). It is made with FFmpeg `geq` from the frame index (`make-media.sh`).
- The subtitle under test draws the frame number it believes is current in the same layout, one block height lower. For ASS that is
  one event per frame with a vector drawing; for PGS and VobSub, one display set per frame.
- `measure-sync.sh` plays the pair and screenshots only the video rect (`screencapture -R`, about 6 samples a second). The app logs
  the rect's screen coordinates. `SubTool probe-sync` reads both numbers from the block centres.
- The difference (subtitle − video) in frames is the offset the viewer sees. 0 means the subtitle matches the picture on screen; −1
  means it is one frame late.
- `-actions "4:seek=40,8:rate=2,..."` scripts seeks, rate changes and pauses, and logs their times, so each sample can be attributed to
  a phase.

## Results per case
Owner's eye: styling, sync feel, HDR brightness and PiP. Everything else was measured.

### 1. ASS styling: pass (owner: "looks right")
- These render as an ASS renderer should:
  - outline and shadow, colour, size, italic, bold and underline overrides
  - `\pos`; `\frz`; `\frx`/`\fry` with `\blur`
  - `\k`, `\kf` (sweep) and `\ko` karaoke; `\fad`; `\move` with `\t(\frz720)`
  - BorderStyle 3 boxes, `\N` line breaks
  - Hebrew and Arabic: right-to-left order and Arabic joining through FriBidi and HarfBuzz
- **Font selection bugs** found with the CoreText provider. To fix in M3; neither was investigated in libass's issue tracker:
  - *Avenir Next bold came out italic:* libass logged `fontselect: (Avenir Next, 700, 0) -> ... AvenirNext-BoldItalic`. The
    style asks for bold, not italic.
  - *Some kanji show as boxes:*
    - For 日本語 libass logged "failed to find any fallback with glyph 0x65E5", and the three glyphs drew as boxes.
    - の字幕 in the same line fell back to Hiragino Sans.
    - CoreText's own fallback (`CTFontCreateForString` from Helvetica Neue) suggests PingFang SC for 日, 本 and 字. That font lives in
      `/System/Library/PrivateFrameworks/FontServices.framework/Resources/Reserved/PingFangUI.ttc`, which libass does not appear to
      accept (inferred, not verified).
    - Hebrew fell back to Lucida Grande and Arabic to Geeza Pro, both correctly.

### 2. Fonts: pass
| Case | Result |
|---|---|
| Font embedded in the script (`[Fonts]`, uuencoded) | Used: `fontselect: (Pacifico, 400, 0) -> Pacifico-Regular, 0` |
| Font as an MKV attachment, fed with `ass_add_font` | Used, same log line |
| Font missing | Falls back to the default family (`sans-serif` → Helvetica), logged at warning level |
| CoreText provider vs none | With none, only embedded, attached and the default font file exist: no per-glyph fallback, so CJK, Hebrew and Arabic glyphs were missing. CoreText is required |
| fontconfig | Not needed and not built. Homebrew's libass builds on macOS with `--disable-fontconfig` ("libass uses coretext on macOS, fontconfig on Linux", `brew cat libass`). libass's `configure.ac` and `meson.build` at 0.17.5 detect CoreText through ApplicationServices (macOS), then `<CoreText/CoreText.h>` (commented "for iOS"). `ass_coretext.c` includes `<CoreText/CoreText.h>` under `TARGET_OS_IPHONE`, which covers tvOS. VLC's contribs build libass with `-Dcoretext=enabled` and set `BUILD_WITH_FONTCONFIG := 0` on Darwin (`contrib/src/ass/rules.mak`, `contrib/src/main.mak`). iOS and tvOS ship no fontconfig |
| Jellyfin-style extraction | `ffmpeg -map 0:2 -an -vn -c:s copy out.ass` from the MKV gave a file byte-identical to the original script. `-dump_attachment:t:0` gave a font byte-identical to the original. Jellyfin's own `SetAssFont` rewrite (below) was not reproduced |

### 3. PGS: pass
- **libavcodec vs our Swift parser,** compared by `SubTool compare-pgs`:

  | File | Event times | Rect geometry | Pixels |
  |---|---|---|---|
  | Dialog file (12 shown display sets, 2 objects in some) | Identical | Identical | Max channel difference 1/255 (YCbCr to RGB rounding) |
  | Frame counter (1500 display sets) | Identical | Identical | Identical (0 difference over 460 MB of pixels) |

  Decoding the whole 1.8 MB frame-counter file took 256 ms with libavcodec and 216 ms with the Swift parser.
- **Our encoder is the only PGS source.** It writes epoch-start display sets with up to two objects and windows, a palette in
  YCbCr (BT.709 for canvases above 576 lines, as libavcodec assumes) and RLE. A disc-authored PGS was not tested, but libavcodec
  decodes our files to the same result as our parser.
- **Drawing:** bitmaps are scaled from the canvas (1920x1080) to the video rect. The bottom object moved above the bar and the
  top-left sign stayed in place (screenshots checked).

### 4. VobSub: pass with caveats
- Jellyfin extracts VobSub to Matroska (`.mks`), never `.idx/.sub` (below). So a VobSub renderer needs a Matroska reader as well as
  the `dvdsub` decoder. The prototype used libavformat and libavcodec for both.
- FFmpeg's `dvdsub` encoder (our only source) reduced the test to 4 colours. It also ended each event up to 8 ms early (3.992 s
  instead of 4.000 s), probably from the format's coarse stop-time units (not verified).
- The 720x576 canvas is drawn over the whole 16:9 video rect, so it is stretched like anamorphic DVD video. What other players do
  for SD VobSub over HD video was not checked.

### 5. Sync: pass; the clock decides
Local MP4, 25 fps on a 60 Hz display, about 90 samples per row unless noted. "0" is the share of samples in which the subtitle
matched the frame on screen; the rest were −1 (one frame late) unless noted.

| Steady playback, 1× | periodic | link | output |
|---|---|---|---|
| ASS (libass) | 47 % (43 of 92) | 69 % (62 of 90) | 99 % (91 of 92) |
| PGS (libavcodec) | 77 % (72 of 93) | 64 % (57 of 89) | 99 % (89 of 90; one +1) |
| PGS (Swift parser) | | | 100 % (89 of 89) |
| VobSub (libavcodec) | | | 100 % of the 80 readable samples. 11 samples fell into the 8 ms gaps where FFmpeg's VobSub ends early, so no subtitle was shown |
| HLS fMP4 from 127.0.0.1, with one seek | 73 % (85 of 117) | | 99 % (114 of 115) |

Through events (ASS; seek to 40 s, 2×, 0.5×, 1×, pause, play, seek to 10 s):
- **periodic and link:** 0 or −1 in every phase, about as above. With *link*, one sample right after the seek showed the subtitle for
  the new time (frame 1000) over the old picture (frame 100): `currentTime()` jumps to the seek target before the new frame is
  shown.
- **output (first version):** 0 in every playing phase, but −1 in 18 of 19 samples while paused. While paused, `itemTime(forHostTime:)`
  read one frame behind the frame on screen.
- **output with `currentTime()` while paused** (the rate is 0): 150 of 153 at 0, paused included. The three exceptions:
  - one −1 after the first seek;
  - one −4 during 2× playback;
  - one sample in which the subtitle already showed the target of the seek to 10 s (frame 250) while the picture still showed frame
    1441. So `itemTime(forHostTime:)` also moves to the seek target before the new frame arrives.
- **output, snapped to the video's frame grid** (below): 152 of 157 at 0, 5 at −1. VobSub: 69 of 69 at 0.
- **Owner:** "in sync" through pause, seeks, rate changes and the bar showing and hiding.

`AVPlayerItemOutput.h` documents the conversion of a display-link time to item time: `itemTimeForHostTime:` "Converts a host time
value (for example a CADisplayLink timestamp …) to the equivalent time on the item's timebase", noting that the timestamp property is
the previous refresh and must be advanced by `duration`. The prototype passes `targetTimestamp`, the time of the frame being prepared.

### 6. Moving above the control bar: pass, with a title-bar finding
- **ASS:** `ass_set_line_position` (percent of the frame height, from our bar's height) lifted the bottom dialogue above the bar.
  `\pos` signs did not move.
- **PGS and VobSub:** objects in the lower half moved up exactly as far as they overlapped the bar; the top sign stayed.
- **Title bar:** in a window, the transparent title bar (`ignoresSafeArea`, as in D3) covers the top 28 points of the video. The PGS
  sign at y = 100 of 1080 was hidden under it. M3 needs a top inset for subtitles while the title bar shows, or the title bar has to
  hide with the controls.

### 7. PiP: our subtitles are not in the PiP window (owner confirmed)
- PiP shows the video only. The main window shows AVPlayerLayer's placeholder, and we hide our layer while PiP is active, since it
  would draw over the placeholder. After returning, subtitles draw again.
- **Why:** `AVPictureInPictureController`'s content source is an `AVPlayerLayer` or an `AVSampleBufferDisplayLayer` with a playback
  delegate (`AVPictureInPictureControllerContentSource`, macOS 12+, `AVKit/AVPictureInPictureController.h`). A sibling layer of ours
  is not part of either.
- **Options (not built):**
  - *Sample-buffer content source:* we would pull decoded frames from `AVPlayerItemVideoOutput`, composite the subtitle into each
    frame (Metal), and enqueue the result into an `AVSampleBufferDisplayLayer`. It is the only way to show ASS or bitmap subtitles in
    PiP, but it puts our compositing on the video path: it costs every frame, needs HDR-aware compositing, and means a second
    presentation path to keep in sync.
  - *System rendering for text tracks during PiP:* when PiP starts, turn `suppressesPlayerRendering` off for WebVTT and tx3g, so
    AVPlayer draws them itself. Whether AVPlayer's captions appear in the macOS PiP window was not verified.
  - *Accept no subtitles in PiP* for ASS, PGS and VobSub.
- **AirPlay** has the same gap. AirPlay of HLS hands the URL to the receiver (ADR 0008), so only HLS WebVTT renditions can reach it.
  Our layer never does (inferred; not tested with subtitles).

### 8. HDR: SDR reference white; owner judged on the built-in display only
- The subtitle layer stays at `CALayer.preferredDynamicRange = .standard` (the default, `CALayer.h`) with untagged sRGB contents.
  White is therefore drawn at EDR 1.0, SDR reference white.
- Apple: "EDR 0.0 represents black, 1.0 represents SDR max, also known as 'reference white' or 'UI white'"; values above 1.0 are
  "highlights and emissive surfaces"; "Most user interface elements shouldn't exceed EDR 1.0"; reference white "is scaled by the
  display brightness slider or auto brightness" ("Explore HDR rendering with EDR", WWDC21).
- ITU-R BT.2408 puts graphics white at HDR reference white, 203 cd/m² on a PQ display. That was read through a BBC post to the W3C
  colour-on-the-web list, not the ITU PDF itself (sources).
- The prototype can switch white to 0.8 (dimmed sRGB) or 2.0 (extended linear sRGB half-float contents,
  `preferredDynamicRange = .high`, `contentsHeadroom = 2`).
- **Owner (built-in display, HDR10 test video):** 1.0 is comfortable, and 2.0 was visibly brighter, so the EDR path works but is not
  wanted.
- The external HDR display was not connected: **not judged there.**

### 9. Cost
`measure-cost.sh` averages the app's own CPU time (`CLOCK_PROCESS_CPUTIME_ID`, 100 % = one core) over 12 s, and the GPU's
"Device Utilization %" from `ioreg -c IOAccelerator`. The output clock was used, with libass rendering at the on-screen pixel size.

| Video, window | Subtitles | App CPU | Subtitle draw on the main thread |
|---|---|---|---|
| 1080p H.264, 1280x720 pt window (2560x1440 px) | none | 10 % | — |
| same | `styled.ass` | 17 % | avg 1.4 ms, max 44 ms (first use of a fallback font) |
| same | busy script, every display refresh | 100 % (main thread saturated) | avg 22 ms |
| same | busy script, snapped to video frames | 63 % | avg 9.9 ms per refresh, about 24 ms per changed frame, max 29 ms |
| 2160p HEVC, full screen (2880x1620 px render) | none | 10 % | — |
| same | busy script, snapped | 67 % | max 30 ms |
| same | PGS dialog | 9 % | max 2.1 ms |

- **GPU:** the readings (2-16 %) are within the noise of the rest of the desktop; the paused app alone read 20 %. Any GPU cost of the
  subtitle layer was not measurable this way, and Instruments was not used.
- **Headless libass benchmark** (`SubTool bench-ass`, busy script, 20 s at 25 fps, release build). "Render" is `ass_render_frame`;
  "blend" is our CPU compositing plus `CGImage`:

  | Render size | Render | Blend | One core |
  |---|---|---|---|
  | 1920x1080 | 2.8 ms | 4.7 ms | 19 % |
  | 2880x1620 | 4.9 ms | 9.6 ms | 36 % |
  | 3840x2160 | 7.8 ms | 16.2 ms | 60 % |

  `styled.ass` at 1920x1080: 0.16 ms render per frame, 0.45 ms blend per changed frame.
- **Reading:**
  - The cost follows the render size on screen, not the video's resolution.
  - libass is the smaller share; the CPU blend in Swift dominates.
  - A heavy script misses a 60 Hz frame on the main thread.
  - Snapping the time to the video's frame grid (`nominalFrameRate`) cut the busy script from 100 % to 63 % CPU, because the picture
    only changes 25 times a second.
- **M3 needs three changes:** render off the main thread, one frame ahead of the display link; composite on the GPU (Metal) or with
  vImage; and snap to frame times.

## Research
### Jellyfin (source at tag v12.2, commit `ca0f16eb7c195f720a1493ed469a27c4657db0c6`, released 2026-10-05; read 2026-10-22)
Line numbers are at that tag.

**Subtitle endpoint.** `GET Videos/{routeItemId}/{routeMediaSourceId}/Subtitles/{routeIndex}/Stream.{routeFormat}`, with a
`/{routeStartPositionTicks}/` variant (`Jellyfin.Api/Controllers/SubtitleController.cs:208`, `:294`).
- **ASS/SSA:** embedded tracks are extracted with `-c:s copy` (`MediaBrowser.MediaEncoding/Subtitles/SubtitleEncoder.cs:504-511`,
  `:673`, `:689`).
  - When the requested format equals the stream's codec, the file is returned as is: "Return the original if the same format is
    being requested … ASS is a superset of SSA, skipping the conversion and preserving the styles" (`SubtitleEncoder.cs:139-146`).
  - Any other format goes through the SubtitleEdit library and loses the styling (`:85-91`, `:269-313`).
  - So the client asks for `Stream.ass` (the codec's own name).
  - After extraction, `SetAssFont` replaces `,Arial,` with `,Arial Unicode MS,` in the file (`:762-764`, `:952`).
  - On the pass-through path `startPositionTicks`, `endPositionTicks` and `copyTimestamps` are ignored (`:142-147`): the whole track
    comes back with the source's timestamps.
- **PGS and VobSub:** "Graphical subtitles cannot be parsed or rewritten, so they are always delivered as the raw stream for the client
  to render", served with `PhysicalFile(…, true)` (range requests) for any format matching `IsPgsFormat` or `IsVobSubFormat`
  (`SubtitleController.cs:236-247`; `MediaBrowser.Model/Entities/MediaStream.cs:756-770`).
  - Embedded PGS is extracted to `.sup`. VobSub goes to `.mks`, because "FFmpeg cannot mux VobSub subtitle streams back into the
    .idx/.sub pair" (`SubtitleEncoder.cs:487-497`).
  - External `.idx/.sub` files are not passed through: `GetGraphicalSubtitlePath` accepts only `.sup` or `.mks`
    (`SubtitleController.cs:485-492`; the fallback path is not verified).
  - Raw PGS streaming arrived in 10.10.0 (PR 12056); VobSub and client-rendered graphical subtitles during remux in 12.0 (PRs 16552,
    17227, 17512); raw files with range support in 12.2 (PR 18168). Each was checked as contained in the tag.
- **First request:** asking for any embedded track runs one FFmpeg pass that reads the whole file and writes every extractable track
  (`SubtitleEncoder.cs:208-210`, `:515-566`, `:647-705`). The time limit is `SubtitleExtractionTimeoutMinutes`, default 30
  (`:903-905`). The result is cached under `data/subtitles/` (`Emby.Server.Implementations/Library/PathManager.cs:39`, `:85`,
  `:92`). Expect a slow first request on large files.

**Attachments (fonts).** `GET Videos/{videoId}/{mediaSourceId}/Attachments/{index}`
(`Jellyfin.Api/Controllers/VideoAttachmentsController.cs:50`).
- `MediaAttachment` carries `Codec`, `CodecTag`, `Comment`, `Index`, `FileName`, `MimeType` and `DeliveryUrl`
  (`MediaBrowser.Model/Entities/MediaAttachment.cs:12-48`).
- `DeliveryUrl` is `/Videos/{0}/{1}/Attachments/{2}`, without an API key (`Jellyfin.Api/Helpers/MediaInfoHelper.cs:350-357`), so the
  client sends its auth header.
- Extraction uses `-dump_attachment` (`MediaBrowser.MediaEncoding/Attachments/AttachmentExtractor.cs:422-428`).

**Device profile and planner.**
- A remux to HLS is `PlayMethod.Transcode` (`MediaBrowser.Model/Dlna/StreamBuilder.cs:816-823`). For it, the server tries an External
  subtitle profile without conversion, then one with conversion, and otherwise falls back to `Encode`, which means burn-in
  (`:1536-1542`).
- An internal stream under Transcode is skipped when the server can't extract subtitles, unless it is PGS or VobSub (`:1593-1602`).
  `CanExtractSubtitles` is the admin setting `EnableSubtitleExtraction`, default true (`MediaEncoder.cs:1373-1375`,
  `EncodingOptions.cs:61`). So with extraction turned off, ASS falls back to burn-in.
- A profile matches without conversion when `Codec` equals `Format` (`:1604-1617`). VobSub matches a profile with `Container: mks`
  (`:1640-1653`).
- PlaybackInfo returns `DeliveryMethod`, `DeliveryUrl` (`/Videos/{item}/{ms}/Subtitles/{idx}/{startTicks}/Stream.{format}?ApiKey=…`,
  `StreamInfo.cs:1249-1281`) and `IsExternalUrl`. `startTicks` is 0 for HLS (`:1177-1179`).

**HLS direct stream keeps copying the video.**
- With External delivery, the transcoding URL carries no `SubtitleStreamIndex` or `SubtitleMethod` (`StreamInfo.cs:955`, `:1103`).
- `master.m3u8` defaults `SubtitleMethod` to External (`Jellyfin.Api/Controllers/DynamicHlsController.cs:496`).
- Stream copy is blocked only when "burning in subtitles", that is `SubtitleDeliveryMethod.Encode`
  (`MediaBrowser.Controller/MediaEncoding/EncodingHelper.cs:2447-2455`; also `:8051-8054`).
- Subtitle streams are dropped from the output (`-map -0:s`, `:3195-3199`).
- So External ASS, PGS or VobSub never forces a video transcode, unless the client sends `alwaysBurnInSubtitleWhenTranscoding`.

Jellyfin was not run, since the source settles both questions the manager left open. That External ASS, PGS and VobSub timestamps
match the timeline of Jellyfin's HLS output was not verified. In our own 127.0.0.1 HLS test, the time was 0-based on both sides and in
sync.

### Licences and linking (read 2026-10-22)
- **libass 0.17.5** (2026-06-24): `COPYING` ISC.
  - Also in the source: `libass/aarch64/asm.S` BSD-2-Clause ("VideoLAN and dav1d authors"; binary redistribution must reproduce the
    notice), `libass/x86/x86inc.asm` ISC, `libass/wyhash.h` Unlicense, `libass/ass_strtod.c` an old permissive UC/Sun notice.
  - No GPL or LGPL files.
  - Required: FreeType ≥ 9.17.3, FriBidi ≥ 0.19.1, HarfBuzz ≥ 1.2.3 (`meson.build`, `configure.ac`); HarfBuzz since 0.15.0.
  - Optional: libunibreak since 0.17.0, iconv. libpng is only for the test programs.
  - `meson_options.txt` has no option to drop FriBidi. Meson has been a secondary build system since 0.17.2.
- **FreeType:** `LICENSE.TXT`, FTL or GPLv2; we take the FTL. Its §2 requires binary redistribution to "provide a disclaimer that states
  that the software is based in part of the work of the FreeType Team, in the distribution documentation".
- **HarfBuzz 15.0.0:** "Old MIT" (`COPYING`). The prototype ran Homebrew's 14.4.0; its licence was not read separately.
- **FriBidi 1.0.17:** `COPYING` LGPL-2.1, with "version 2.1 of the License, or (at your option) any later version" in every `lib/`
  and `gen.tab/` file.
  - The only GPL files are `bin/getopt*.c`, which are linked into the command-line tools only (`bin/meson.build`). Building with
    `-Dbin=false` leaves them out.
  - Homebrew's "GPL-2.0-or-later AND LGPL-2.1-or-later" describes the package with its tools.
  - Size: 154 KB as a dylib (Homebrew arm64).
- **libunibreak 8.0:** zlib. **fontconfig:** MIT-style, and not needed.
- **Homebrew's dylibs** link more than we would ship: FreeType links libpng, zlib and bzip2; HarfBuzz links glib and graphite2. Our
  build turns these off, as VLC's contribs do. graphite2's licence was not checked.
- **Apple bundles:**
  - "iOS, watchOS, and tvOS support third-party frameworks but don't support third-party standalone dynamic libraries, which are those
    outside a framework bundle". Frameworks go in `Contents/Frameworks/` on macOS and `Frameworks/` on iOS and tvOS ("Placing Content
    in a Bundle").
  - An XCFramework "can include dynamic library files, but only macOS supports these libraries for dynamic linking. Dynamic linking
    on iOS, iPadOS, tvOS … requires the XCFramework to contain `.framework` bundles" ("Creating a multiplatform binary framework
    bundle").
  - App Review Guideline 2.5.2 requires apps to be self-contained and not to "download, install, or execute code which introduces or
    changes features or functionality of the app". A framework inside the bundle is self-contained; that is our reading, not an Apple
    statement.
- **LGPL-2.1 §6** requires terms that "permit modification of the work for the customer's own use and reverse engineering for debugging
  such modifications". It also requires either relinkable object code or "a suitable shared library mechanism".
  - On iOS and tvOS, replacing a framework in a signed App Store app means re-signing, so whether a framework alone satisfies §6 there
    is contested.
  - Qt's own page on LGPL obligations says some "online application stores, may have rules that are in conflict with LGPL" (written
    about LGPLv3).
  - Not verified: the GNU FAQ (it returned HTTP 429) and App Store Connect's error texts for standalone dylibs.
- **FriBidi as a framework:** done on paper only. The prototype linked Homebrew's `libfribidi.0.dylib` dynamically (`otool -L` on
  libass); no app-bundle framework layout was tried.

## Not verified
- Subtitle brightness on an external HDR display; a 120 Hz (ProMotion) display; 23.976 or 29.97 fps video (frame snapping with
  `nominalFrameRate`).
- A real Jellyfin server; disc-authored PGS and VobSub (only our own encoder and FFmpeg's VobSub encoder made test files).
- iOS and tvOS (CoreText fonts in the sandbox, frameworks); the App Store.
- The CJK fallback and the Avenir Next face bugs (causes inferred).
- Text tracks rendered by AVPlayer inside the PiP window; subtitles over AirPlay.
- GPU cost with Instruments; Dolby Vision with subtitles; libunibreak's effect on line breaking.

## Sources (read 2026-10-22)
- macOS 27.0 SDK headers (Xcode 27.1): `AVFoundation/AVPlayerItemOutput.h` (`itemTimeForHostTime:`, `suppressesPlayerRendering`),
  `AVKit/AVPictureInPictureController.h` (`AVPictureInPictureControllerContentSource`), `QuartzCore/CALayer.h`
  (`preferredDynamicRange`, `contentsHeadroom`, deprecation of `wantsExtendedDynamicRangeContent`), `AppKit/NSScreen.h` (EDR values).
- libass 0.17.5 `ass.h` (Homebrew include): `ass_set_line_position`, `ass_set_fonts`, `ASS_DefaultFontProvider`, `ass_add_font`,
  `ass_set_extract_fonts`, `ASS_Image`.
- Apple, "Explore HDR rendering with EDR" (WWDC21), https://developer.apple.com/videos/play/wwdc2021/10161/.
- ITU-R BT.2408 graphics white and HDR reference white (203 cd/m²), read through Simon Thompson (BBC) on the W3C public-colorweb list,
  https://lists.w3.org/Archives/Public/public-colorweb/2019Sep/0003.html. The ITU report itself was not read.
- Jellyfin source, https://github.com/jellyfin/jellyfin at tag v12.2 (files and lines above).
- libass https://github.com/libass/libass (tag 0.17.5: `COPYING`, `meson.build`, `configure.ac`, `meson_options.txt`, `Changelog`,
  `libass/ass_coretext.c`); FreeType https://github.com/freetype/freetype (`LICENSE.TXT`, `docs/FTL.TXT`); HarfBuzz
  https://github.com/harfbuzz/harfbuzz (15.0.0 `COPYING`); FriBidi https://github.com/fribidi/fribidi (v1.0.17 `COPYING`,
  `bin/meson.build`); libunibreak https://github.com/adah1972/libunibreak (`LICENCE`); fontconfig `COPYING` (Homebrew 2.18.3).
- VLC contribs, https://github.com/videolan/vlc (`contrib/src/ass/rules.mak`, `contrib/src/fribidi/rules.mak`,
  `contrib/src/freetype2/rules.mak`, `contrib/src/harfbuzz/rules.mak`, `contrib/src/main.mak`).
- Apple, "Placing Content in a Bundle", https://developer.apple.com/documentation/bundleresources/placing-content-in-a-bundle;
  "Creating a multiplatform binary framework bundle",
  https://developer.apple.com/documentation/xcode/creating-a-multi-platform-binary-framework-bundle; App Review Guidelines 2.5.2,
  https://developer.apple.com/app-store/review/guidelines/.
- Qt, "Obligations of the GPL and LGPL", https://www.qt.io/licensing/open-source-lgpl-obligations.
- Pacifico, https://github.com/google/fonts/tree/45e3d5269e4a17d823f6d84470bc084dc5ae46f6/ofl/pacifico (`OFL.txt`).
