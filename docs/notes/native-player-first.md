# Native player first: research notes (2026-10-28)

Decision: `docs/decisions/0025-native-player-first.md` (accepted 2026-10-28, owner). No code was written and no device was used: every fact below is
from a document, with its address, the page's own date where it shows one, and the date read (2026-10-28 for everything here unless
said otherwise). "Not verified" means no primary source was found; the Windows prototype (ADR 0025, task W1) checks those on the
owner's PC.

## The owner's rule (2026-10-28)
"Add a native player route for all the targets as it should've been from the very beginning, remux if needed. app engine is used as
fallback in a situation when native player have no support for this media type and server remux - transcode as overall fallback."

And: the same behaviour as an existing third-party client, "until you can suggest a better way, or something special that specific
platform can or cannot do" (owner, 2026-10-28). That client's public documentation, as the manager read it on 2026-10-28:
- Native playback whenever the native player can play every selected track; otherwise its own engine (FFmpeg); the server last.
- Text subtitles (SRT, WebVTT, MP4 text) stay on the native route. Picture subtitles (PGS, DVD, DVB, XSUB) are shown by its own
  engine, so selecting one moves the file there; its on-device remux "does not apply to ... subtitles that require rendering". Styled
  ASS/SSA: the documentation does not say which route; ADR 0025 treats ASS that needs its styling like a picture subtitle (inferred).
- H.264 High 10 goes to its engine ("Local hardware decoding is required"); TrueHD Atmos goes to its engine (decoded to PCM); E-AC-3
  Atmos plays natively.
- Dolby Vision: true Dolby Vision only on the native player or its remux; the remux excludes profile 7; its engine renders only a
  compatible base layer; "Profiles without a compatible base layer require native AVPlayer or HLS playback".
- Its playback information names the route and why: "Native playback is fully compatible", "Preserves the original video stream",
  "Local hardware decoding is required", "Server compatibility playback" after "The previous playback route could not start".

## Windows (ships first)

### Formats Media Foundation plays without extra codecs
"Supported codecs - Windows apps", https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/supported-codecs
(ms.date 2026-05-12; checked twice, 2026-10-28). Scope: "these tables list the codecs that are included with Windows. Users and apps
can install additional codecs that may be available to use."

| | Included with Windows (desktop) | Notes |
|---|---|---|
| Containers (video table) | fMP4, MPEG-4, MPEG-2 PS, MPEG-2 TS, MPEG-1, 3GPP, AVCHD, ASF, AVI, **MKV**, DV | WebM and Ogg are not listed |
| Video in MKV | MPEG-1, MPEG-2, MPEG-4 Part 2, H.265, H.264, VC-1, VP8, VP9, AV1 (all "D") | "H.265 and AV1 are available with the install of the corresponding optional codec pack"; "MPEG-2/MPEG-1 codec is available with install of the optional Windows DVD Player app" |
| Video in MPEG-4 / fMP4 | H.264, H.265, VP9, AV1, MPEG-4 Part 2, VC-1, VP8 and others | Same footnotes |
| Audio | HE-AAC v1/v2, AAC-LC, ALAC, FLAC (in FLAC), LPCM, MP3, MPEG-1/2 audio, WMA, AMR-NB; AC-3 before 24H2 | "Beginning with Windows 11, version 24H2, the AC-3 codec is no longer included with Windows. However, many device manufacturers will pre-install an AC-3 codec." |
| Audio not in the table | E-AC-3, TrueHD, DTS, Opus, Vorbis | E-AC-3: see the Dolby decoder below; Vorbis: Web Media Extensions below; TrueHD, DTS, Opus: no source found (not verified) |
| Timed text | "TTML, WebVTT, SubRip, Substation Alpha, VobSub, PGS" | Whether in-band tracks in MKV and ASS styling work: not verified (below) |

The audio table's columns are containers (MPEG-4, ADTS, ASF, RIFF, AVI, AC-3, FLAC, WAV ...); it has no MKV column, so which audio
codecs MF reads from MKV is not stated (not verified).

- **Dolby Audio Decoder**, https://learn.microsoft.com/en-us/windows/win32/medfound/dolby-audio-decoder (updated 2023-08-18): decodes
  "Dolby Digital ... (AC-3)" and "Dolby Digital Plus ... (E-AC-3)"; "the decoder skips all dependent substreams". Whether it still
  ships on 24H2, given the AC-3 note above, is not verified.
- The older Win32 page "Supported Media Formats in Media Foundation" (updated 2025-04-15) lists no MKV or TS; the Windows apps page
  above is the newer one.

**So Windows reads MKV itself:** no device remux is needed for the common case (MKV with H.264, HEVC, VP9 or AV1). A container MF
does not read (WebM, Ogg without the extension, others) goes to libmpv, which reads them all (ADR 0019's capability list).

### Store codec extensions
Store product pages, https://apps.microsoft.com/detail/<id> (read 2026-10-28; prices for the US Store):

| Product | Id | Price | What it says |
|---|---|---|---|
| HEVC Video Extensions | 9NMZLZ57R3T7 | 0.99 USD | "Play High Efficiency Video Coding (HEVC) videos in any video app ... For devices that don't have hardware support for HEVC videos, software support is provided" |
| HEVC Video Extensions from Device Manufacturer | 9N4WGH0Z6VHQ | free | The same text; installed by device makers (not listed for browsing in the Store, inferred) |
| AV1 Video Extension | 9MVZQVXJBQ9V | free | "Now uses the dav1d decoder" |
| VP9 Video Extensions | 9N4D0MSMP0PT | free | "software support is provided" without VP9 hardware |
| Web Media Extensions | 9N5TDP8VCMHS | free | "OGG Container Parser • Vorbis Decoder • Theora Decoder", "automatically used by both web sites and apps" |
| Dolby Vision Extensions | 9PLTG1LWPHLF | free (Dolby Laboratories) | "This application requires a Dolby Vision licensed device." |

- **Which of them a Windows 11 PC has from the start: not verified.** The codec page only calls H.265 and AV1 "optional codec
  pack[s]". A PC without the HEVC extension has no HEVC decoder in MF at all; libmpv decodes HEVC on the graphics card (d3d11va) or in
  software regardless (ADR 0021).
- **Whether an unpackaged app such as ours can use them: not verified by Microsoft.** The extensions are packaged MFTs
  (`uap4:MediaCodec`: "Defines an extension that enables an app to install media codecs from the Microsoft Store",
  https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap4-mediacodec, ms.date 2026-06-05). The
  HEVC decoder page lists "Windows 10 [desktop apps only]" and `hevcdecoder_store.dll`
  (https://learn.microsoft.com/en-us/windows/win32/medfound/h-265---hevc-video-decoder). `MFT_ENUM_FLAG_UNTRUSTED_STOREMFT =
  0x00000400` is declared with no description (https://learn.microsoft.com/en-us/windows/win32/api/mfapi/ne-mfapi-_mft_enum_flag).
  Only secondary sources say desktop apps get them (a Media Foundation developer's blog, https://alax.info/blog/1979, 2019; a GitHub
  gist showing the classic Windows Media Player loading `HEVCDECODER_STORE.dll`). **The prototype (task W1) must check it.**
- **Telling the user which one is missing:** `Windows.Media.Core.CodecQuery.FindAllAsync(kind, category, subtype)` "returns all
  installed codecs that match the supplied parameters", the subtype being a GUID string or FOURCC, with `CodecSubtypes` for the GUIDs
  ("Query for installed codecs", https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/codec-query, ms.date
  2026-08-23). CodecQuery is not on the list of WinRT APIs unsupported in desktop apps
  (https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-api-desktop-app-support, ms.date 2026-07-22, which "might
  not be comprehensive"). Whether it works unpackaged and lists Store codecs: not verified (task W1). The Store opens a product page
  with `ms-windows-store://pdp/?ProductId=<id>` ("recommended"; https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-store-app,
  ms.date 2025-02-19); from an unpackaged app through `Launcher.LaunchUriAsync` or the shell: not verified.
- What this means for "native first": a missing extension is not a failure. The planner sees the codec missing from the native
  engine's capabilities and picks libmpv, which plays it; the supported formats page can say "Install HEVC Video Extensions to play
  this with Windows' own player" with the Store link (a proposal, ADR 0025).

### HDR
- "Use DirectX with Advanced Color on high/standard dynamic range displays",
  https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range (updated 2025-09-19): XAML "Supports playback of
  HDR videos using MediaPlayerElement"; "Windows provides a built-in HDR tonemapper effect as part of Direct2D as well as in the Media
  Foundation HDR video playback pipeline"; "Additional hardware requirements might apply ... hardware codec acceleration (10 bit HEVC,
  10 bit VP9, etc.)". That statement is about UWP XAML; **for WinUI 3's `MediaPlayerElement` it is not verified** (task W1).
- Only HDR10 (BT.2100 PQ) is named; **HLG and HDR10+ through MF: not verified**.
- **Dolby Vision** needs the Dolby Vision Extensions and "a Dolby Vision licensed device" (Store page above); "For Dolby Vision
  certified monitors, Windows will automatically choose Dolby Vision when it is available" ("HDR settings in Windows",
  support.microsoft.com, no page date). Dolby Vision through `MediaPlayer` in a WinUI 3 app: not verified; an open issue reports
  "MediaPlayerElement does not support DolbyVision" (microsoft/microsoft-ui-xaml #9238, filed by a user, no Microsoft answer seen).
  On an ordinary PC and display, then, Windows has **no true Dolby Vision route** for us; libmpv applies the Dolby Vision metadata
  itself and outputs HDR10 or SDR (ADR 0007, ADR 0019).
- **Dolby Vision, checked against leads from the owner (2026-10-28; the leads were a secondary summary, so only primary sources are
  quoted here):**
  1. *Dolby Vision only on certified displays, with Microsoft's or Dolby's extension.* Partly supported: the extension "requires a
     Dolby Vision licensed device" (Store page) and Windows picks Dolby Vision "For Dolby Vision certified monitors" ("HDR settings in
     Windows"). Microsoft lists "Dolby Vision" among the certifications to look for in a display or PC ("Display requirements for HDR
     video in Windows", https://support.microsoft.com/help/4040547, no page date). That only certain apps (Films & TV, Media Player)
     show it: not verified. Dell's article "may not see Dolby Vision option when the ACM is turned on" (Dell KB 000222733, last
     modified 2026-05-20) shows the mode exists on some vendors' laptops.
  2. *External monitors and TVs over HDMI usually get HDR10.* Microsoft's requirement for an external display is HDR10 ("The HDR
     display or TV must support HDR10, DisplayPort 1.4, HDMI 2.0 or higher, USB-C, or Thunderbolt", same page); nothing found says
     Windows sends Dolby Vision to an external display. Not verified either way.
  3. *Profile 7 falls back to HDR10 in Windows' players.* Not verified.
  4. *Only dedicated, licensed tools decode it fully.* Not verified.
  - **The owner's PC:** an HDR10-only DisplayHDR 400 monitor (no Dolby Vision, no HDR10+, per its maker's datasheet, read by the
    manager 2026-10-28) and an SDR monitor. True Dolby Vision cannot be tested there; a Dolby Vision certified built-in laptop screen
    is not testable by the owner.
  - **Which route looks better on an HDR10 monitor that is not Dolby Vision:** libplacebo (libmpv's renderer, `vo=gpu-next`) has
    "Native support for Dolby Vision HDR, including Profile 5 conversion to HDR/PQ or SDR, reading DV side data, and reshaping"
    (libplacebo `README.md`, master, read 2026-10-28), and mpv keeps the metadata by default (`format` filter: `dolbyvision` "Whether or
    not to include Dolby Vision metadata (default: yes)", mpv v0.41.0 `DOCS/man/vf.rst`). So libmpv applies each frame's reshaping and
    outputs PQ. Media Foundation without a Dolby Vision display would show the base layer: HDR10 with static metadata for profile 8.1,
    and for profile 5, which has no HDR10-compatible base layer, wrong colours or nothing (inferred, not verified). Task W1 compares
    both by eye on the owner's monitors (ADR 0025, departure 2).
- **Frame-server mode** (`IsVideoFrameServerEnabled`: "the media player does not render video content. Instead, your app should
  register for the VideoFrameAvailable event and call CopyFrameToVideoSurface", API reference, ms.date 2017-02-08): the pages say
  nothing about 10-bit, colour space or HDR metadata, and a user report (microsoft-ui-xaml #6610, closed 2023-10-25) found float targets
  refused and the output washed out. **HDR in frame-server mode: not verified, and unlikely to be simple.**

### Audio
- Multichannel PCM: the Dolby decoder outputs "Windows 10: stereo, 5.1, 7.1" (page above).
- Passthrough: the Dolby decoder MFT can output `MFAudioFormat_Dolby_AC3_SPDIF` and IEC 61937 E-AC-3 "for HDMI digital output"
  ("For HDMI output, only independent substream 0 is packed"), but **`MediaPlayer` has no documented control for bitstream output**
  (not verified). TrueHD and DTS passthrough: no source (not verified). libmpv does no passthrough today either (ADR 0021, "Audio").
- Spatial audio: "Microsoft Spatial Sound support is also integrated into Media Foundation; apps that use media foundation can
  successfully play Dolby Atmos content with no additional implementation", with "real-time Dolby Atmos encoding for both HDMI and
  stereo headphone output, DTS:X for Headphones, and Windows Sonic for Headphones" ("Spatial Sound",
  https://learn.microsoft.com/en-us/windows/win32/coreaudio/spatial-sound, ms.date 2024-07-18). Which sources keep their objects
  (E-AC-3 JOC, TrueHD Atmos): not verified. Since the Dolby decoder "skips all dependent substreams", E-AC-3 JOC objects may be lost
  (inferred, not verified).

### Subtitles
- Formats: the list above includes SubRip, WebVTT, TTML, Substation Alpha, VobSub and PGS.
- External files: `TimedTextSource.CreateFromStream` / `CreateFromUri`, and since 1703 `CreateFromStreamWithIndex` /
  `CreateFromUriWithIndex` for "image-based subtitles from the provided image and index streams" (VobSub), added through
  `MediaSource.ExternalTimedTextSources` (API reference; "Media items, playlists, and tracks",
  https://learn.microsoft.com/en-us/windows/apps/develop/media-playback/media-playback-with-mediasource, ms.date 2026-08-23). The core
  already fetches external subtitles itself (ADR 0019), so it hands bytes as a stream.
- In-band tracks: `TimedMetadataTrack` with `TimedMetadataKind` `Subtitle` (5) and `ImageSubtitle` (6, "The metadata contains images
  to be used as subtitles"), `ImageCue` with `Position`, `Extent` and `SoftwareBitmap`. Whether PGS in MKV arrives as `ImageCue`: not
  verified (task W1).
- **Presentation:** `TimedMetadataTrackPresentationMode.ApplicationPresented`: "they are not displayed by the platform. Cue events are
  raised", set with `MediaPlaybackItem.TimedMetadataTracks.SetPresentationMode` (same page). So our own subtitle view over the video,
  above our control bar, is possible for text and image cues, as on macOS (ADR 0008, ADR 0010). Platform rendering had a bug with
  caption effects, fixed for Windows App SDK 1.6 (microsoft-ui-xaml #7981, 2024-03-11, by a contributor whose affiliation is not shown).
- **ASS styling: not verified.** "Substation Alpha" is listed, nothing says its styles, positions or animation are kept. Under the
  owner's rule a styled ASS track goes to libmpv (which renders it through libass, D2 case 5).

### Streams through 127.0.0.1, HLS
- Progressive: `MediaSource.CreateFromUri` over `http://127.0.0.1:<port>/...` (the relay, ADR 0020). "loopback connections for IPC are
  blocked by default for packaged applications" ("Interprocess communication", ms.date 2020-03-23); our app is unpackaged (ADR 0009),
  and nothing found says MF itself refuses loopback (not verified, task W1). Range requests by MF: not verified.
- HLS: `AdaptiveMediaSource` "supports playback of Http Live Streaming (HLS) and Dynamic Streaming over HTTP (DASH) content", takes a
  `Windows.Web.Http.HttpClient` for headers, and its `DownloadRequested` event can rewrite each request ("Adaptive streaming",
  https://learn.microsoft.com/en-us/windows/apps/develop/media-playback/adaptive-streaming, ms.date 2026-08-23). Through the relay no
  header is needed.
- **fMP4 HLS is in doubt:** "HLS tag support" (https://learn.microsoft.com/en-us/windows/apps/develop/media-playback/hls-tag-support,
  ms.date 2026-02-24) marks `EXT-X-MAP` (the fMP4 init segment) "Not Supported"; its table stops at Windows 10 1607, so it may be stale
  (not verified). Jellyfin's transcodes for us are HLS with fMP4 segments (ADR 0019 choice 2 A), so whether the native player can take a
  server stream decides which engine plays server routes (ADR 0025, "Which engine plays a server stream").

### Detecting what plays, per file
- Before loading: `CodecQuery` (above) for the codecs, and the container rules from the codec page.
- After opening: each track's `SupportInfo.DecoderStatus` (`MediaDecoderStatus`: `FullySupported` 0, `UnsupportedSubtype` 1 "The
  encoding subtype of the media item is not supported by the decoder", `UnsupportedEncoderProperties` 2, `Degraded` 3; API reference,
  10.0.14393), read in `AudioTracksChanged` / `VideoTracksChanged`, and `MediaSourceStatus` on a track's `OpenFailed` ("Media items,
  playlists, and tracks"). This is ADR 0008's `isDecodable` check for Windows: anything but `FullySupported` on a selected track is a
  failed start.

### Our controls over the video, in WinUI 3
- `MediaPlayerElement` and `MediaTransportControls` came to WinUI 3 with Windows App SDK 1.2 ("WinUI apps can play audio and video
  with the MediaPlayerElement and MediaTransportControls", release notes 1.2, ms.date 2025-09-22; a contributor wrote on
  microsoft-ui-xaml #6610 that it shipped in 1.3). In 1.2's notes: "Setting MediaPlayerElement.Source to relative URI (ms-appx/ms-resource)
  fails in unpackaged apps" (we never use such URIs).
- `AreTransportControlsEnabled="False"` with `SetMediaPlayer`, and "If the MediaPlayer was set on MediaPlayerElement using
  SetMediaPlayer, you are responsible for ensuring the MediaPlayer is properly closed" ("Play audio and video with MediaPlayer",
  https://learn.microsoft.com/en-us/windows/apps/develop/media-playback/play-audio-and-video-with-mediaplayer, ms.date 2026-08-23).
- XAML over `MediaPlayerElement` (our acrylic bar, menus, the info panel): no document says it works or fails (not verified, task W1).
  `SwapChainPanel` with libmpv already does (ADR 0007).
- Windows App SDK: the latest stable is 2.6.4 (release channels page, ms.date 2026-10-27); we are on 2.5.1. No upgrade is proposed.

### System integration
- **SMTC:** `MediaPlayer` has its own integration; to drive the controls ourselves "you should disable the automatic integration ...
  by setting the CommandManager.IsEnabled property to false" ("Manual control of the System Media Transport Controls", ms.date
  2026-08-23). We keep today's `SystemMediaTransportControlsInterop.GetForWindow` path (ADR 0019) for both engines, so the flyout
  doesn't change with the engine.
- **Picture in picture:** `Microsoft.UI.Windowing.CompactOverlayPresenter`, "Displays an app window using a CompactOverlay
  (picture-in-picture) configuration" (API reference, updated 2026-07-28, since Windows App SDK 1.0). It is a window presenter, so
  it works for either engine; not built today.
- **Tracks:** "Setting the SelectedIndex property of the media playback item's VideoTracks list causes the ... MediaPlayer to switch
  the active video track"; audio the same ("Media items, playlists, and tracks").
- **Speed:** `PlaybackSession.PlaybackRate`; `IsSupportedPlaybackRateRange` (API reference, 10.0.15063).
- **Buffered ranges:** `PlaybackSession.GetBufferedRanges` (10.0.15063), for the controller's buffered value.
- **Chapters:** `TimedMetadataKind.Chapter` exists; whether MKV or MP4 chapters are exposed: not verified. Jellyfin also sends
  chapters with the item, so the core need not rely on either engine.

### Unpackaged compared with packaged
- "Features that require package identity" (ms.date 2026-10-03) lists nothing about media playback.
- WinRT classes that need identity include `Windows.Media.MediaExtensionManager` (so we cannot register our own codecs into MF),
  `Windows.Media.MediaProperties.MediaEncodingSubtypes`, `Windows.Media.Devices.MediaDevice` (the audio output picker's selector) and
  `Windows.Media.Playback.PlaybackMediaMarker` ("WinRT APIs not supported in desktop apps", ms.date 2026-07-22).
- Background audio: "The system will mute your app when it goes into the background if your app has not declared the
  backgroundMediaPlayback capability" (MediaPlayer guide) is written for packaged apps; for an unpackaged window: not verified (task W1).
- Store codecs and CodecQuery unpackaged: not verified (above).

## macOS: what changes against ADR 0008 and ADR 0010
- **AVPlayer stays first.** D3's table (ADR 0008, docs/notes/macos-video.md, case 5) is the native engine's capability list.
- **The device remux is needed:** AVFoundation does not open MKV or WebM (`-11828`, D3; Apple's `AVFileType` list,
  https://developer.apple.com/documentation/avfoundation/avfiletype, has no Matroska or WebM type), so MKV with H.264 or HEVC and audio
  AVPlayer takes is repackaged on the Mac instead of on the server (ADR 0008 route 2 moves to the device). Proposed: FFmpeg's
  libavformat demuxes the MKV and muxes fMP4 segments, and the core serves an HLS playlist of them through the loopback relay on
  127.0.0.1 (D3 played HLS from 127.0.0.1). The design is task M3-2's spike. What the sources say:
  - **Licence:** "FFmpeg is licensed under the GNU Lesser General Public License (LGPL) version 2.1 or later", with optional GPL parts;
    "Compile FFmpeg without "--enable-gpl" and without "--enable-nonfree"" (https://ffmpeg.org/legal.html, no page date). As D6 does
    on Windows (docs/notes/windows-mpv-build.md).
  - **fMP4 HLS:** the `hls` muxer's `hls_segment_type fmp4`: "Output segment files in fragmented MP4 format ... fmp4 files may be used
    in HLS version 7 and above" (https://ffmpeg.org/ffmpeg-formats.html).
  - **The HEVC tag:** Apple requires "'avc1', 'hvc1', or 'dvh1' rather than 'avc3', 'hev1', or 'dvhe'" (HLS authoring spec, rule
    1.10) and "The container format for HEVC video MUST be fMP4" (1.5)
    (https://developer.apple.com/documentation/http-live-streaming/hls-authoring-specification-for-apple-devices, latest revision entry
    2025-06-26). FFmpeg's `movenc.c` tag table lists `hev1` before `hvc1` for HEVC (FFmpeg master `c69954c`), so the remux must set
    `hvc1` itself.
  - **Dolby Vision:** `movenc.c`'s `mov_write_dvcc_dvvc_tag` writes `dvcC` (profile 7 and below) or `dvvC` (8 to 10) only in unofficial
    strict mode ("Not writing 'dvcC'/'dvvC' box. Requires -strict unofficial."), so the remux sets it or loses Dolby Vision. Apple's
    spec names Dolby Vision profile 5, 8.1 and 8.4 (as `SUPPLEMENTAL-CODECS`, "Dolby Vision 8.1 and 10.1 (compatible with HDR10)") and
    10, and never profile 7; profile 7 not playing on Apple devices is inferred from that silence, which the existing client's
    documentation also says (its remux excludes profile 7).
  - **MediaExtension** (format readers that could add MKV to AVFoundation) is listed for macOS 15.0 and Mac Catalyst 18.0 only
    (https://developer.apple.com/documentation/mediaextension), not iOS or tvOS; ADR 0008 already turned it down.
- **AV1 hardware:** "A17 Pro includes a dedicated AV1 decoder" (Apple Newsroom, 2023-09-12) and M3's media engine "supports AV1
  decoding" (Apple Newsroom, 2023-10-30); `VTIsHardwareDecodeSupported` tells at run time (Apple documentation). Without it AV1 goes to
  libmpv (dav1d in software) rather than to the server.
- **libmpv becomes a real fallback**, no longer deferred: it covers AV1 on Macs without AV1 hardware, `hev1`, TrueHD, DTS,
  multichannel Opus, picture subtitles and styled ASS (owner's rule), and local MKV files (ADR 0008 gaps 1 to 5). How libmpv draws
  on macOS with our SwiftUI controls over it is not researched here (render API on OpenGL, deprecated by Apple, or `vo=gpu-next`
  through Vulkan on MoltenVK; not verified): a spike of its own (task M3-3).
- **ADR 0010's subtitle layer:** under the owner's rule picture subtitles and styled ASS go to libmpv, which renders them itself, so the
  libass/PGS layer over AVPlayer is no longer needed for them. Keeping it would keep HDR and Dolby Vision with a PGS track on the native
  route: ADR 0025, departure 5 (approved: the rule first, the layer later).

## Other platforms (brief, for the ADR)

### Linux: GStreamer
- **playbin3:** "playbin3, decodebin3, uridecodebin3, parsebin, and urisourcebin are no longer experimental" (GStreamer 1.22 release
  notes, 2023-01-23); "Playbin3 and decodebin3 are now stable and the default in gst-play-1.0, GstPlay/GstPlayer" and "the recommended
  playback components" (1.24 notes, 2024-03-04). Latest stable 1.28.8 (https://gstreamer.freedesktop.org/releases/).
- **Into GTK 4:** `gtk4paintablesink`, "a GTK4 video sink that provides a GdkPaintable ... Supports zero-copy rendering via OpenGL on
  Linux and macOS" (1.22 notes); "the sink can render DMABufs directly on Linux if GTK 4.14 or newer is used" (its README). Licence
  **MPL-2.0** (`video/gtk4/Cargo.toml` in gst-plugins-rs), which ADR 0003 must rule on. A `GdkPaintable` in a `GtkPicture` takes our
  GTK controls in a `GtkOverlay` above it, as ADR 0011 does with the GLArea.
- **HDR:** with its `gtk_v4_20` feature the sink maps colorimetry to `GdkCicpParams` (source, `video/gtk4/src/sink/frame.rs`); GDK has
  `GdkCicpParams` with "16: BT.2100 PQ" and "18: BT.2100 HLG" and `GdkColorState` since 4.16 (docs.gtk.org). That is more than the
  libmpv render API offers (ADR 0011: 8-bit GLArea, no output colour space). Whether GTK sends HDR to the compositor end to end: not
  verified (ADR 0011's colour-management point still holds).
- **MKV:** `matroskademux`, "Demuxes Matroska/WebM streams into video/audio/subtitles", is in GStreamer Good
  (https://gstreamer.freedesktop.org/documentation/matroska/index.html). **No device remux on Linux.**
- **Subtitles:** `assrender` (Bad) "Renders timestamped SSA/ASS subtitles on top of a video stream" through libass; `dvdspu` (Bad)
  takes `subpicture/x-pgs` (GStreamer documentation). So GStreamer can keep ASS styling and show PGS on the native route (departure 6).
- **Hardware decoding:** the `va` plugin (Bad): `vah264dec`, `vah265dec`, `vavp9dec`, `vaav1dec` (documentation).
- **Licences:** GStreamer core LGPL-2.1 (`COPYING`); gst-libav: "if you are distributing an application which has a non-GPL compatible
  license ... you have to make sure not to build FFmpeg with GPL code enabled" (its README); gstreamer-rs `MIT OR Apache-2.0`, latest
  0.25.4. Which H.264 and HEVC decoders distributions ship in their GStreamer packages (patents): not verified.

### Android: Media3 (ExoPlayer)
- "Supported formats" (https://developer.android.com/media/media3/exoplayer/supported-formats, last updated 2026-07-21): MP4, fMP4,
  WebM, Matroska and MPEG-TS are read; HLS with TS or fMP4. "By default ExoPlayer uses Android's platform decoders. Hence the supported
  sample formats depend on the underlying platform". **No device remux on Android.**
- Extensions: "software decoder libraries for AV1, VP9, FLAC, Opus, FFmpeg, MIDI, IAMF and MPEG-H"; the FFmpeg one "provides
  `FfmpegAudioRenderer`" (audio only), the AV1 one uses dav1d (androidx/media, `release` branch, 1.11.1). Apache-2.0.
- Subtitles: `PgsParser` and `SsaParser`; SSA overrides parsed are only `\pos`, `\move` and `\an` (source), so styled ASS is not kept.
- Passthrough: `AudioCapabilities.isPassthroughPlaybackSupported(Format, AudioAttributes)` "Returns whether the device can do
  passthrough playback for format."
- Dolby Vision: "H.264/AVC, H.265/HEVC or AV1 decoders can decode the base layer of some DV profiles" (`MediaCodecUtil`, source).
- Android's app engine would be libmpv for Android (not researched).

### iOS, iPadOS, tvOS
As macOS (AVPlayer, the device remux, libmpv), with AV1 hardware from A17 Pro. tvOS has no PiP for the app's own layers and no
MediaExtension. Which Apple TV models decode AV1: not verified.

### Web
- MSE takes the byte stream formats registered with W3C: WebM, ISO BMFF (fMP4), MPEG-2 TS, MPEG audio and AAC
  (https://www.w3.org/TR/mse-byte-stream-format-registry/, Registry Draft 2026-06-04). Matroska is not registered, so **MKV needs a
  remux** (in the browser, for example the core on WebAssembly with libavformat, or on the server).
- Detecting: `canPlayType`, `MediaSource.isTypeSupported` and Media Capabilities' `decodingInfo()` ("returns a promise that fulfills
  with information about how well the user agent can decode/display media with a given configuration"; MDN, 2026-09-02).
- Native HLS: Chromium has "Enable HLS manifest parser and demuxer" in `media/media_options.gni`; that Chrome turned it on from 141
  or 142 is from a secondary source only (not verified). Safari: not verified here.
- The web has no app engine (no libmpv in a browser); its order is native, remux, server.

## Is a device remux needed? (per platform)
| Platform | Native player reads MKV? | Device remux needed |
|---|---|---|
| Windows | Yes (Media Foundation's MKV column, above) | No, for MKV. Other containers MF lacks go to libmpv |
| macOS, iOS, iPadOS, tvOS | No (D3: `-11828`) | Yes: MKV to fMP4/HLS for AVPlayer |
| Linux | Yes (`matroskademux`) | No |
| Android | Yes (Media3's Matroska extractor) | No |
| Web | No (Matroska is not an MSE byte stream format) | Yes, to fMP4 (or WebM), in the browser or on the server |

## New dependencies, for the owner's approval
Nothing below is added by this spike. Microsoft's Windows SDK and Windows App SDK packages are approved in advance (CLAUDE.md); the
rest needs the owner, with ADR 0003's licence check, in the PR that adds it.

| Platform | Dependency | Licence | For | Status |
|---|---|---|---|---|
| Windows | `Windows.Media.Playback` / `Windows.Media.Core` (Windows SDK) and `MediaPlayerElement` (Windows App SDK 2.5.1, already used) | Microsoft | The native engine | Approved in advance; no new package (both are already referenced) |
| Windows | Store codec extensions (HEVC, AV1, VP9, Web Media, Dolby Vision) | Microsoft / Dolby, installed by the user | Optional native codecs | Not shipped or redistributed by us; the app only links to the Store |
| Windows | None for a device remux | | | Not needed (MF reads MKV) |
| macOS, iOS, tvOS | FFmpeg's libavformat, libavcodec (parsers and bitstream filters only), libavutil: our own LGPL build, as D6 | LGPL-2.1-or-later | The device remux | **Needs the owner** |
| macOS | libmpv (our LGPL build) and its libraries (libplacebo, FFmpeg's decoders, libass and the rest of D6's list), plus whatever renders it on macOS (MoltenVK, Apache-2.0, if `gpu-next` on Vulkan; not decided) | LGPL-2.1-or-later and permissive | The app engine | **Needs the owner** (was deferred by ADR 0008) |
| macOS | libass, FreeType, HarfBuzz, FriBidi (ADR 0010) | ISC, FTL, MIT, LGPL | ADR 0010's layer | Only if departure 5 keeps the layer; otherwise libmpv brings libass itself |
| Linux | GStreamer core, base, good, bad (system libraries); gstreamer-rs crates (`gstreamer`, `gstreamer-video`, `gstreamer-play` or `gstreamer-player`) at 0.25.x; `gst-plugin-gtk4` (gtk4paintablesink) | LGPL-2.1-or-later; MIT OR Apache-2.0; **MPL-2.0** | The native engine | **Needs the owner**; MPL-2.0 needs a ruling under ADR 0003 |
| Android | AndroidX Media3 (ExoPlayer), optional decoder extensions | Apache-2.0 | The native engine | Later, needs the owner |
| Web | libavformat compiled to WebAssembly, or none if the server remuxes | LGPL-2.1-or-later | The remux in the browser | Later, needs the owner |
