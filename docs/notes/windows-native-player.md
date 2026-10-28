# W1 spike notes: Windows' own player under our controls (2026-10-28)

Decision: `docs/decisions/0025-native-player-first.md` (revised by this spike). Research before it: `docs/notes/native-player-first.md`.
Everything below was seen on the owner's PC on 2026-10-28 unless it says otherwise. The prototype is throwaway and is not in the
repository: it lives outside it, in `%USERPROFILE%\Developer\Projects\current-spikes\windows-native-player` on the owner's PC
(`app\` the WinUI app, `proxy\` the loopback proxy and a Jellyfin helper, `tools\` the media generator and run scripts, `logs\` and
`shots\` the evidence).

## Setup
| Item | Version |
|---|---|
| Windows | Windows 11 Pro 26H2, build 26300.9711 (no Insider channel set) |
| Graphics | the owner's discrete NVIDIA card (driver 32.0.16.2101) and the CPU's AMD graphics (driver 32.0.21045.16010) |
| HDR monitor (primary) | HDR10-only DisplayHDR 400, 3840×2160, 150 %, Windows HDR on. `DisplayInformation.GetAdvancedColorInfo()`: `HighDynamicRange`, max 390 nit, SDR white 204 nit, HDR metadata formats: `Hdr10` only |
| SDR monitor | 1920×1080, 100 %. `GetAdvancedColorInfo()`: `StandardDynamicRange`, max 270 nit, SDR white 80 nit |
| Audio | default output: the HDR monitor's HDMI speakers (stereo); a USB gaming headset. Spatial format on both: Dolby Atmos for Headphones (`SpatialAudioDeviceConfiguration.ActiveSpatialAudioFormat`) |
| .NET, Windows App SDK | SDK 10.0.403; Windows App SDK 2.5.1 as the app's component packages (WinUI 2.3.9, InteractiveExperiences 2.1.9, Foundation 2.3.12, Base 2.0.4), unpackaged, self-contained, TFM `net10.0-windows10.0.26100.0`. No other package |
| Codec packages at the start | HEVC Video Extensions (2.4.151, paid), HEVC Video Extensions from Device Manufacturer (2.5.33), AV1 Video Extension (2.0.35), VP9 Video Extensions (1.2.20), Web Media Extensions (2.1.51), MPEG-2 Video Extension, **Dolby Vision Extensions (2.20503.802, package `DolbyLaboratories.DolbyVisionAccess`)**, Dolby Digital Plus decoder for PC OEMs (1.2.591, installed by the PC maker) |
| libmpv for the comparison | Current built from main (`3f5353d`) with the shared D6 libmpv build (`apps/windows/build.ps1 -Mpv`) |
| FFmpeg (local tool, test media only) | 9.0.2 GPL build; nothing it made is shipped or committed |

## How the prototype works
- `MediaPlayerElement` with `AreTransportControlsEnabled="False"` and `SetMediaPlayer`; `MediaPlayer.CommandManager.IsEnabled = false`.
  Above it, in the same XAML tree: our subtitle layer (a `TextBlock` for text cues, a `Canvas` with `Image` for image cues), an acrylic
  facts panel and an acrylic control bar (play/pause, back 10 s, forward 30 s, seek slider, audio, subtitle, speed and chapter lists,
  picture in picture, full screen, CodecQuery and Store buttons), each with `AutomationProperties.Name`.
- Local files through `MediaSource.CreateFromStorageFile`, URLs through `MediaSource.CreateFromUri`, HLS through
  `AdaptiveMediaSource.CreateFromUriAsync` and `MediaSource.CreateFromAdaptiveMediaSource`; one `MediaPlaybackItem` per file.
- Facts logged per track: encoding properties with every `MF_MT_*` attribute by name (names from the Windows SDK 10.0.26100.0
  headers), `SupportInfo.DecoderStatus` and `MediaSourceStatus` at open and later, `OpenFailed` per track, `MediaFailed`.
- Subtitles: each timed metadata track set to `ApplicationPresented`; `CueEntered`/`CueExited` drive our layer; the first image cues
  are saved as PNG. Chapters: the `Chapter` track's `ChapterCue`s.
- SMTC: `SystemMediaTransportControlsInterop.GetForWindow(hwnd)`, driven by us (as the app does today, ADR 0019).
- Streams from 127.0.0.1: a small Python HTTP/1.1 server (local files with `Range`, or forwarding to the owner's Jellyfin with the
  session token in a header, never in a URL). It stands in for the app's relay (ADR 0020), which needs a playback in the core.
- Test media: synthetic files from FFmpeg (H.264, HEVC 8 and 10 bit, VP9, AV1, H.264 High 10; AAC, AC-3, E-AC-3, TrueHD, DTS,
  FLAC, Opus, PCM; SRT, the D2 spike's ASS file, a PGS track written by the D2 spike's writer; MKV chapters; HLG; HDR10+ with
  dynamic metadata that switches every 5 s on an unchanging picture, and the same picture with static HDR10 only; local HLS with
  fMP4 and TS segments). Public samples: the Jellyfin project's test videos (4K HEVC HDR10, 4K AV1 HDR10, Dolby Vision profiles 5,
  8.1 and 8.4, all MP4; CC BY-SA 4.0) and the WebM project's VP9 HDR10+ sample. The owner's library through the proxy: MKV files with
  Dolby Vision 5, Dolby Vision 8.1 with HDR10+ and E-AC-3 with Atmos, TrueHD Atmos 7.1, DTS, DTS-HD MA, PGS, DVD subtitles, styled
  ASS, H.264 High 10. No Dolby Vision profile 7 file exists on the owner's server or in a trusted public source, so profile 7 was
  not checked.

## Results

### 1. XAML over the video, Narrator (HDR monitor)
The owner played a 4K Dolby Vision 8.1 / HDR10+ MKV with the control bar, the open lists and the facts panel over the video,
dragged the seek bar, resized the window and used picture in picture and full screen: **no flicker and no airspace issue**. Narrator
read our controls by name (Play/Pause, Seek, Audio track, Subtitles, ...). Same as libmpv's `SwapChainPanel` (D2, ADR 0007).

### 2. Containers and video
| File | Result |
|---|---|
| MKV H.264 High + AAC | Plays; seek and buffered ranges fine |
| MKV HEVC Main and Main 10 | Plays (HEVC extension installed) |
| MKV VP9 + Opus stereo | Plays |
| MKV AV1 10 bit + Opus stereo | Plays (AV1 extension installed) |
| MP4 H.264 + AAC + E-AC-3 5.1 | Plays |
| MPEG-TS H.264 + AC-3 5.1 + AAC | Plays (subtype `MFVideoFormat_H264_ES`) |
| WebM VP9 Profile 2 HDR10+ | Plays. WebM is not in Microsoft's container table (notes, "Formats Media Foundation plays"); which installed component reads it was not looked into |
| MKV / real MKV, H.264 High 10 | `DecoderStatus` is **`FullySupported`** at `MediaOpened`, then `MediaFailed`: `DecodingError`, `0x80004005` (E_FAIL), at once |
| 4K Dolby Vision 5, MKV from the server | `MediaFailed`: `DecodingError`, `0xC00D3E8C` `MF_E_INVALID_FORMAT` ("The Media format is recognized but is invalid"), with and without the Dolby Vision Extensions. The same title as Jellyfin's fMP4 HLS remux and the public DV 5 sample in MP4 both open |

**Without the Store extensions** (the owner removed the paid HEVC package; the AV1 and the device-maker HEVC packages were removed with
`Remove-AppxPackage` at the owner's request): `CodecQuery` returned nothing for HEVC and AV1, and
- **MKV:** the file opens and plays its audio; the video track raises `OpenFailed` (`0xC00D5212` `MF_E_TOPO_CODEC_NOT_FOUND`, "No
  suitable transform was found to encode or decode the content"), `DecoderStatus` `UnsupportedSubtype`, `MediaSourceStatus`
  `FullySupported`. Playback state stays `Playing`.
- **MP4:** the whole file fails: `MediaFailed` `SourceNotSupported`, `0xC00D5212`.
- With them reinstalled, both played again. The device-maker HEVC package cannot be reinstalled by a user: the Store says "This app
  will not work on your device"; the paid package alone decodes HEVC (owner's PC, after the check).

**What the decodability check must use:** `DecoderStatus` alone is not enough (H.264 High 10). A native start has failed when, soon
after opening, a selected track raises `OpenFailed`, any `MediaFailed` arrives, or a selected track's `DecoderStatus` is not
`FullySupported`, and the playback state is no proof (audio-only `Playing` above).

Frame rates: MF reported half the real rate for FFmpeg-written MKVs (12 fps for 24, 30 for 60; `MF_MT_FRAME_RATE`), and the right
rate for the server's 23.976 and 25 fps MKVs and for MP4 and TS; not looked into. The core should take the frame rate from Jellyfin.

### 3. CodecQuery and the Store link, unpackaged
- `CodecQuery.FindAllAsync(kind, CodecCategory.Decoder, "")` works from the unpackaged app and **lists the Store extensions**:
  `HEVCVideoExtension` (subtypes `HEVC`, `HEVS`; listed twice while both HEVC packages were installed), `AV1VideoExtension` (`AV01`),
  `VP9VideoExtensionDecoder` (`VP80`, `VP90`), `MPEG2VideoExtension`, and Web Media Extensions' `FFmpegVideoDecoder` (Theora) and
  `FFmpegAudioDecoder` (Vorbis), all `IsTrusted` true.
- Per subtype (`CodecSubtypes.VideoFormatHevc`, `"AV01"`, ...) it returns one entry per installed decoder, and **none when the
  extension is missing**, so the app can name the missing extension.
- The Dolby Vision Extensions add **no** CodecQuery entry: the app cannot tell from CodecQuery whether they are installed.
- `Launcher.LaunchUriAsync(ms-windows-store://pdp/?ProductId=...)` returned `true` and the Store opened the product page from the
  unpackaged app (HEVC 9NMZLZ57R3T7, AV1 9MVZQVXJBQ9V, device-maker HEVC 9N4WGH0Z6VHQ; screenshot of the HEVC page taken).
- Built-in decoders seen: H.264, MPEG-4 Part 2, MJPEG, WMV, AAC, ALAC, FLAC, MP3, **Opus** ("Microsoft Opus Audio Decoder MFT"),
  MPEG-H. AC-3 and E-AC-3 decode through "Dolby Digital Plus Decoder MFT", which on this PC is the PC maker's package
  (`DolbyDDPDecMft.dll` in `DolbyLaboratories.DolbyDigitalPlusDecoderOEM`); Windows' own `mfaudiocnv.dll` only registers the IEC
  61937 converters for AC-3/E-AC-3, TrueHD and DTS (registry `HKLM\SOFTWARE\Classes\MediaFoundation\Transforms`).

### 4. HDR and Dolby Vision
Media types: MF exposes `MF_MT_TRANSFER_FUNCTION` 15 (PQ) or 16 (HLG) and `MF_MT_VIDEO_PRIMARIES` 9 (BT.2020) (values from
`mfobjects.h`), and static HDR metadata as `MF_MT_MAX_LUMINANCE_LEVEL`, `MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL` and
`MF_MT_MAX_MASTERING_LUMINANCE`. Dolby Vision files show as plain `HEVC`; for DV 5 the transfer and primaries are 0 (unknown).

**HDR monitor (owner, by eye):**
- HDR10 (public 4K HEVC sample) and HLG (synthetic): right.
- Dolby Vision 8.1 with HDR10+ (an MKV from the server), Windows' player against Current (libmpv) at the same scene: "both look
  the same".
- HDR10+: the owner could not tell whether the picture changed at the 5 s metadata switches. Not verified on the HDR monitor.

**SDR monitor (measured from screen captures, which are exact there):**
- **Windows' player clips PQ video at about SDR white instead of tone-mapping it.** The synthetic PQ ramp turns white from 47.6 % of
  its width, PQ code 487 of 1023, about 78 nit; the display's SDR white is 80 nit. The test pattern coded at 203 nit is blown out.
  A frame of the public HDR10 sample is bright and washed out next to libplacebo's tone-mapped render of the same frame (FFmpeg's
  `libplacebo` filter, the renderer libmpv uses, D2 and ADR 0007).
- HDR10+ is ignored there: the ramp's pixels are identical in both HDR10+ scenes and in the static-HDR10 twin.
- HLG is fine: it looks like the SDR pattern it was made from (HLG is backward compatible), slightly brighter than libplacebo's.
- Dolby Vision 8.1 (public sample): identical with and without the extension, and clipped like HDR10.

**Dolby Vision 5 (public sample, MP4), frame at 11.43 s, against libplacebo's render with and without the DV metadata:**
| | HDR monitor | SDR monitor |
|---|---|---|
| Dolby Vision Extensions installed | DV applied (purple/pink, as libplacebo with DV); the extension's decoder loaded in our process (it printed `DoViGenericTag : Logger Initialized` to stdout) | DV applied, lighter and pinker than libplacebo |
| Extensions removed | not captured | **base layer only: teal/green, wrong colours**, with `DecoderStatus` `FullySupported` and no error |

So, with the Dolby Vision Extensions, Windows' player applied profile 5's reshaping on an HDR10-only display and on an SDR one, which
the Store's "requires a Dolby Vision licensed device" (notes, "Store codec extensions") did not suggest. True Dolby Vision output
(a DV display mode) is not testable here. Current (libmpv) showed the server's DV 5 file with natural colours (screenshot).

### 5. Audio
| Track | Result |
|---|---|
| AAC 2.0, AAC 5.1, AC-3 5.1, E-AC-3 5.1, FLAC 5.1, Opus 2.0 | Play (`FullySupported`) |
| E-AC-3 with Atmos (JOC), real file | Plays, reported as 6 channels. Whether the objects reach Windows' spatial sound (Dolby Atmos for Headphones was active): not verified; neither the owner nor any API we have could tell. **Speed changes are ignored while it plays** (below) |
| TrueHD 5.1 (synthetic), TrueHD Atmos 7.1 (real), DTS 5.1, DTS-HD MA (real) | `DecoderStatus` `UnsupportedSubtype`, the track's `OpenFailed` `0xC00D5212`; video plays on **silently**, state `Playing` |
| Opus 5.1 in MKV | `UnsupportedEncoderProperties`, `OpenFailed` `0xC00D36B4` `MF_E_INVALIDMEDIATYPE` |
| PCM 5.1 in MKV | `DecoderStatus` `FullySupported`, but `OpenFailed` `0xC00D5212` |

AC-3 on 24H2 and later: this PC decodes AC-3 and E-AC-3 only through the PC maker's Dolby package (§3), as Microsoft's codec page
foresees ("many device manufacturers will pre-install an AC-3 codec"). A PC without one would lose AC-3 natively (not tested; the
package was not removed). Multichannel output to 5.1 or 7.1 speakers: not checked (the owner has stereo speakers and a headset).

### 6. Subtitles in MKV (`ApplicationPresented`)
- **SRT:** `TimedTextCue`s with the text per line; `<i>` and `<b>` arrive as `Subformats` (italic, bold) of the line.
- **ASS:** `TimedTextCue`s carrying the static styling: the style's font, size (as a percentage), colour, outline colour and width,
  bold and italic, an opaque box (`BorderStyle 3` as a background), top or bottom placement as a region, and an inline `\c` colour
  as a subformat. Lost: `\move` (static region), `\fad`, `\frz` rotation, karaoke timing (`\k`; text kept), and `\pos` becomes an
  approximate region (`\pos(960,540)` gave a region at 0 %, 50 % of 100 % × 50 %). The owner's real ASS track is exposed as a
  `Subtitle` track; no cue arrived in the window watched.
- **PGS: yes, as `ImageCue`s** (synthetic and real): `Position` 0,0 and `Extent` 100 % × 100 %, a 1920×1080 `SoftwareBitmap` in
  `Bgra8` premultiplied (the PGS canvas, also for 4K video) with the subtitle drawn in place and the rest transparent; `Duration`
  is the maximum value (the end comes with the next cue). Our layer drew them in the right place over a 4K video (owner).
- **DVD subtitles (VobSub)** in MKV are exposed as an `ImageSubtitle` track; no cue arrived in the window watched (not verified).
- The order of `TimedMetadataTracks` changed between two opens of the same file (ASS was index 3, then 1); each track's `Id` stayed
  the same. The engine must match tracks by `Id`, not by index.
- MP4: the in-band `tx3g` subtitle and FFmpeg-written chapters produced no timed metadata track at all.

### 7. Streams through 127.0.0.1
- **Progressive (`MediaSource.CreateFromUri`):** local files and the owner's Jellyfin `Videos/{id}/stream?static=true` through the
  proxy all played and seeked. MF sends `Range` requests (`bytes=N-M` and open-ended `bytes=N-`), opens and aborts several
  connections while seeking, and identifies as `NSPlayer/12.00.26100.9711 WMFSDK/12.00...`. No loopback restriction.
- **HLS with fMP4 segments (`AdaptiveMediaSource`) works,** although Microsoft's tag table marks `EXT-X-MAP` "Not Supported" (notes,
  "Streams through 127.0.0.1, HLS"): FFmpeg's fMP4 HLS (`#EXT-X-MAP:URI="init.mp4"`), **Jellyfin's transcode** (H.264 + AAC,
  `SegmentContainer=mp4`) and **Jellyfin's remux** (HEVC Main 10 HDR10 copied, AC-3) all opened (`AdaptiveMediaSourceCreationStatus`
  `Success`), played and seeked. HLS with TS segments too. `AdaptiveMediaSource` sends no `User-Agent`.
- Seeks far ahead: `MF_E_OPERATION_CANCELLED` (`0xC00D36ED`) on the segment in flight is MF cancelling its own request (playback
  went on). Once, a far seek into a Jellyfin remux of a 4K file waited more than 20 s for the server's first segment; MF gave up after
  about 10–12 s with `MF_E_NET_TIMEOUT` (`0xC00D4278`) and asked again. In later runs the same seeks recovered within 3 s. The engine
  needs a deadline of its own for that case (W4).
- A Dolby Vision 5 title remuxed by Jellyfin to fMP4 HLS opened and played (picture not judged).

### 8. SMTC, picture in picture, tracks, speed, chapters
- **SMTC with `CommandManager` off:** the system session (read through `GlobalSystemMediaTransportControlsSessionManager`, as the
  flyout does) showed our app, type Video, play/pause and seek enabled, our position and end time. A seek to 10:00 and pause/play
  from the session reached the app (`PlaybackPositionChangeRequested`, `ButtonPressed`).
- **CompactOverlay and full screen** (`AppWindow.SetPresenter`): worked, video kept playing (owner).
- **Track switching:** audio switched within about a second with no stall (owner); the scripted runs switched every audio track.
- **Speed:** `PlaybackSession.PlaybackRate` 2 and 0.5 gave exactly 2.00× and 0.50× on local files, over HTTP and on a server file.
  **With an E-AC-3 Atmos track selected**, `IsSupportedPlaybackRateRange(0.5, 2)` is false and the rate is accepted
  (`PlaybackRateChanged` fires) but ignored (1.00×); on the same file's AC-3 track speed works again.
- **MKV chapters** arrive as a `Chapter` timed metadata track of `ChapterCue`s (4 synthetic, 11 in a real file); jumping to them
  worked (owner). MP4: see §6.

### 9. Other findings that change ADR 0025 or its tasks
- `Windows.Media.Devices.MediaDevice` (`GetDefaultAudioRenderId`, `GetAudioRenderSelector`) worked from the unpackaged app, although
  Microsoft lists it among the WinRT APIs that need package identity (notes, "Unpackaged compared with packaged"). Not relied on.
- Background audio and the system muting a hidden window: not checked.
- `PlatformPresented` rendering of subtitles: not checked (our layer is the plan).

## What ADR 0025 takes from this
- Departure 1 (no device remux on Windows): confirmed.
- Departure 2 (Dolby Vision on libmpv first): confirmed, with new reasons: without the extension DV 5 shows wrong colours with no
  error; the extension cannot be detected through CodecQuery; DV 5 in MKV fails natively even with it.
- Departure 3 (PGS native): confirmed for PGS; DVD subtitles not verified.
- Departure 4 (server streams on libmpv): dropped; fMP4 HLS works natively (owner, 2026-10-28).
- New departure 8 (PQ video on an SDR display goes to libmpv) and the speed rule for E-AC-3 Atmos: the owner's decisions, 2026-10-28.

## Sources
- Windows SDK 10.0.26100.0 headers, read 2026-10-28: `um/mferror.h` (`MF_E_INVALID_FORMAT` 0xC00D3E8C, `MF_E_TOPO_CODEC_NOT_FOUND`
  0xC00D5212, `MF_E_INVALIDMEDIATYPE` 0xC00D36B4, `MF_E_UNSUPPORTED_BYTESTREAM_TYPE` 0xC00D36C4, `MF_E_OPERATION_CANCELLED`
  0xC00D36ED, `MF_E_NET_TIMEOUT` 0xC00D4278, with their message texts quoted above); `um/mfobjects.h` (`MFVideoTransFunc_2084 = 15`,
  `MFVideoTransFunc_HLG = 16`, `MFVideoPrimaries_BT2020 = 9`); `um/mfapi.h` and others for the `MF_MT_*` attribute names.
- Microsoft's pages as quoted in `docs/notes/native-player-first.md` (each with its ms.date; read 2026-10-28): "Supported codecs",
  "HLS tag support", "WinRT APIs not supported in desktop apps", the Store product pages.
- Jellyfin test videos, https://repo.jellyfin.org/test-videos/ (read 2026-10-28): "licensed under the Creative Commons
  Attribution-Sharealike license".
- WebM project HDR10+ sample, https://storage.googleapis.com/downloads.webmproject.org/vp9/decoder-test-streams/HDR10+_Sample_Video.webm
  (downloaded 2026-10-28).
