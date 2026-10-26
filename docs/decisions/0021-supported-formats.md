# 0021: The "Supported formats" page and its hardware test

Status: accepted (2026-10-26). The owner's decisions and the manager's conditions are listed apart; the rest is proposed, for the
owner's look on their PC before the merge (marked so).

## Context
Users ask what a client plays before they ask the server to convert it. The Windows app had no way to show it: the engine's
capabilities were read only when a player started (`crates/app/src/player/mpv.rs`), and nothing said which codecs the graphics card
decodes. The engine plays with mpv's `hwdec=auto-safe` (ADR 0007, ADR 0019), which tries the whitelisted hardware decoders of
`hwdec_autoprobe_info` with the direct ones first, `d3d11va` first of all on Windows (below), so whether a codec is decoded on the card
depends mostly on the card's Direct3D 11 decoder profiles and on mpv's own rules.

## Decided by the owner (2026-10-26)
1. **A Settings sub-page, not a dialog.** Under Settings → Playback, a clickable SettingsCard "Supported formats" with the chevron opens
   a page inside Settings, as Windows Settings opens its nested pages. Its title is a BreadcrumbBar ("Settings > Supported formats"), and
   Back returns to Settings. Above the tabs, a short description says that results also depend on the file, the server's route and the
   display.
2. **Five tabs in a SelectorBar**: Video, Audio, Containers, Subtitles, HDR, each a list with column headers: Codec (or Format), On this
   PC, Notes. **Dolby inside the tabs:** Dolby Vision under HDR; TrueHD, E-AC-3 and Atmos under Audio.
3. **The Hardware column tests this GPU** through Direct3D 11's video decoder profiles (`GetVideoDecoderProfileCount`,
   `GetVideoDecoderProfile`, `CheckVideoDecoderFormat`) for H.264, HEVC Main and Main10, VP9 profiles 0 and 2, AV1 Main, MPEG-2 and
   VC-1: a codec with a profile is "Hardware", one without "Software", within what `auto-safe` allows.
4. **Built in the cloud; the owner looks at it on their PC before the merge.**

The owner's mock, with the MPEG-4 Part 2 row as this PR shows it (the mock had "Software"; see the manager's conditions):
```
Settings > Supported formats
[ Video ] Audio  Containers  Subtitles  HDR
Codec            On this PC        Notes
H.264 (AVC)      Hardware
HEVC (H.265)     Hardware          8-bit and 10-bit
AV1              Software          No hardware decoder on this graphics card
VP9              Hardware
MPEG-4 Part 2    Server converts   DivX, Xvid. The server converts it to HEVC (H.265) or H.264 (AVC)
```

## The manager's conditions (2026-10-26)
- No new package: the GPU test is hand-written interop.
- All logic in the core, words included: the page only renders what the core gives.
- MPEG-4 Part 2 stays out of the engine's codec table (adding it changes the device profile, a separate decision) and shows honestly as
  "Server converts".
- The test covers the adapter mpv decodes on, the page names it, and the name is never logged.
- The headless probe never holds the UI; its failure is a note on the page, not a dialog; a play that starts during it wins.
- One async call across the bridge, no subscription.

## Decision

### Where the logic is
The core builds the whole page (`current-app`'s `formats`, exposed by `current-ffi` as `Core::supported_formats`): tab titles, column
headers, rows, the support value and its word, and the notes. The Windows page runs two platform tests, passes their plain facts in,
and renders what comes back. The rows come from three inputs and never from a fixed list of what should work:
- **The engine's capabilities** (`EngineCapabilities`, from libmpv's `decoder-list` and `demuxer-lavf-list`, `current-mpv`'s `caps.rs`).
  A codec the engine does not report is never shown as playing on this PC. Tables in `formats.rs` only name and order rows; a value the
  engine reports that they lack gets a row under its own name (open values).
- **What the device profile asks of the server**, read from the profiles themselves (`current_jellyfin::transcode_targets` and
  `external_subtitle_formats` over `profile.rs`, not copies).
- **The facts the UI passes in:** the graphics card's decoder profiles, and the display the window showing the page is on (the same
  `DisplayFacts` the player takes, ADR 0007).

The column's values: **Hardware**, **Software**, **Plays**, **Server repackages**, **Server converts**, **Not supported**, **HDR**,
**Tone-mapped**, and **Couldn't check** when the engine could not be started: then every row says so, with a note, and no row claims a
format does not play.

### Proposed, for the owner's look
- **"Plays"** for containers and subtitles, and for video when the graphics card could not be tested (rather than guessing Hardware
  or Software).
- **"Server repackages"** for a container the engine does not read. Jellyfin turns direct stream off for every client
  (`MediaInfoHelper`: "direct-stream http streaming is currently broken"), so a remux arrives as a transcode into one of the profile's
  HLS transcoding profiles whose job copies the streams the device takes (`AllowVideoStreamCopy`, `AllowAudioStreamCopy`, default true;
  `GetVideoTranscodeProfile` ranks profiles by whether the video can be copied) (Jellyfin v12.2, commit ca0f16eb, read 2026-10-25;
  docs/notes/playback.md, "How the server decides").
- **The subtitle rule**, from Jellyfin's `StreamBuilder.GetSubtitleProfile` and `GetExternalSubtitleProfile` and
  `MediaStream.SupportsSubtitleConversionTo` (v12.2, commit ca0f16eb, read 2026-10-26). For a format the engine does not render, the
  server looks for an `External` profile of the same kind (text or picture, `MediaStream.IsTextFormat`) without conversion, then with
  one, which only text allows ("Can't convert from this": `ass`, `ssa`; "Can't convert to this": the same); otherwise it falls back to
  `SubtitleDeliveryMethod.Encode`, burning the subtitle into the picture, which converts the video. So the page says: a text format other
  than ASS and SSA "The server converts it to SubRip (SRT)" (or the profile's other external text formats); ASS, SSA and picture-based
  formats "The server burns it into the picture, which means converting the video". That the server may extract the track (its
  `EnableSubtitleExtraction`, on by default) is assumed.
- **HDR without display facts:** "Plays", with "HDR or tone-mapped to SDR, depending on the display".
- **The headless probe** (next section).

### The engine's capabilities without playing (proposed)
The app keeps the capabilities of every player's engine start. When none is known, a short-lived **headless engine** is started only to
read them: `current-mpv`'s `Output::Headless`, which sets `vo=null` and `ao=null` and no `force-window`, so no window and no audio device
opens. It runs in a task of its own on the blocking pool, never on the UI thread, and is shut down as soon as its lists are read. Its
answer is kept for the run even when the page that asked was closed meanwhile; a failed start is not kept, so opening the page again
retries.

It shares the process's one engine slot (ADR 0019). libmpv's initialisation cannot be cut short, so a play that starts during a probe
waits for it (the probe's whole life, not a fixed timeout; `a_headless_probe_and_a_player_start_both_succeed`, run by CI's "mpv
(Windows)" job, starts both together and prints the times); then the play wins: a player's capabilities replace the probe's, the probe
never replaces a player's, and a probe that failed behind a player's start answers with the player's.

Considered: caching the capabilities on disk from the last run (stale after an update of the bundled libmpv, and empty on first start);
a fixed list per build (would claim what a build lacks).

### The hardware test (Windows)
In C# (`Player/DecoderTest.cs`), through COM vtables and delegates as the player's interop is (no package, no unsafe code):
`CreateDXGIFactory1`, `IDXGIFactory1::EnumAdapters1(0)`, `D3D11CreateDevice` on that adapter with `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`,
`ID3D11VideoDevice`, then every profile from `GetVideoDecoderProfile` with `CheckVideoDecoderFormat` for `DXGI_FORMAT_NV12` (8-bit)
and `DXGI_FORMAT_P010` (10-bit). The UI passes the adapter's name and each profile's GUID and two booleans; which codec a GUID decodes
is the core's to say. It runs off the UI thread, once per app run, and again when `IDXGIFactory1::IsCurrent` reports that the adapters
changed, which is one call when the page opens.

The core's rule (`formats.rs`), following what the engine does:
- **mpv's methods.** `hwdec_autoprobe_info` (`video/decode/vd_lavc.c`, v0.41.0) lists `d3d11va` first with `HWDEC_FLAG_AUTO |
  HWDEC_FLAG_WHITELIST`, and `hwdec_compare` puts direct methods before copying ones ("List non-copying entries first, so --hwdec=auto
  takes them"). The page answers for `d3d11va`. When it cannot take a file, auto-safe can still fall back to a whitelisted copying
  method (`d3d11va-copy`, `dxva2-copy`, `nvdec-copy`), so a "Software" row may in practice be decoded by the card through a copy (not
  verified).
- **mpv's codec list.** `auto-safe` is "exactly the same as `auto`", which enables "any whitelisted hw decoder", and "Even if enabled,
  hardware decoding is still only white-listed for some codecs"; `--hwdec-codecs` defaults to
  `h264,vc1,hevc,vp8,vp9,av1,prores,prores_raw,ffv1,dpx` (`DOCS/man/options.rst`, `video/decode/vd_lavc.c`, mpv v0.41.0). The engine
  does not change it. So **MPEG-2 is decoded in software** even on a card with an MPEG-2 profile; its note says so.
- **FFmpeg's d3d11va profiles.** A codec counts as decoded on the card when one of the profiles FFmpeg's d3d11va uses for it is listed
  and the driver takes its output format: H.264 (`H264_VLD_NOFGT`, `H264_VLD_FGT`, Intel's `ModeH264_E`; NV12), HEVC (`HEVC_VLD_MAIN`
  NV12, `HEVC_VLD_MAIN10` P010), VP9 (`VP9_VLD_PROFILE0` NV12, `VP9_VLD_10BIT_PROFILE2` P010), AV1 (`AV1_VLD_Profile0`, NV12 for 8-bit
  and P010 for 10-bit), VC-1 (`VC1_D2010`, `VC1_VLD`), MPEG-2 (`MPEG2_VLD`, `MPEG2and1_VLD`) (`dxva_modes` and
  `d3d11va_map_sw_to_hw_format` in `libavcodec/dxva2.c`, FFmpeg n8.0). **VP8 has no D3D11 path in FFmpeg**, so it is software.
- HEVC, VP9 and AV1 say "8-bit and 10-bit", or which depth falls back to software.

### Audio
The engine sets no `audio-spdif` (`current-mpv`'s `start_options`), and mpv's `audio-spdif` has no default value
(`filters/f_decoder_wrapper.c`, v0.41.0), so **nothing is passed through to a receiver**: AC-3, E-AC-3, TrueHD and DTS are decoded to
PCM on this PC, and their notes say so. Our Windows build includes FFmpeg's spdif muxer (docs/notes/windows-mpv-build.md), so passthrough
could be turned on later; it is not today. **Dolby Atmos** plays as ordinary surround: its height information is lost when TrueHD or
E-AC-3 is decoded to PCM. That last part is not verified against FFmpeg's decoders, and the page's note says "not verified".

### HDR
From ADR 0007's mapping: on a display in HDR mode the engine outputs HDR (PQ, BT.2020); on any other it tone-maps to SDR. The page reads
the facts of the display the window is on (`DisplayWatcher.ReadOnce`) and shows "HDR" or "Tone-mapped", with a note naming that
display; without them, "Plays" and "depending on the display". HDR10+ and Dolby Vision profiles 5, 7 and 8 say "Not checked on a device
yet": no such files were played on the owner's PC (docs/notes/windows-video.md, "ADR 0007 item 5"). HLG and HDR10 were.

## Limits
- **The test asks the driver, not a decode.** A listed profile can still fail on a given file (level, resolution, a driver bug), and
  mpv then falls back to software. The page does not claim more than "the card has a decoder for it".
- **One adapter: the one mpv decodes on.** With `--d3d11-adapter` unset, mpv's `mp_d3d11_create_present_device` finds no adapter by
  name and calls `D3D11CreateDevice` with a null adapter and `D3D_DRIVER_TYPE_HARDWARE` (`video/out/gpu/d3d11_helpers.c`, v0.41.0);
  "Pass NULL to use the default adapter, which is the first adapter that is enumerated by IDXGIFactory1::EnumAdapters"
  (`D3D11CreateDevice`, learn.microsoft.com). d3d11va decodes on that device ("will be affected by this choice", `--d3d11-adapter`,
  `DOCS/man/options.rst`). So the test uses DXGI's adapter 0, and on a PC with two graphics cards that is the one tested. The page names
  it ("Tested on: <name>"); the name is runtime data only, never logged (the smoke run prints a count of profiles). Windows' per-app
  graphics preference on a two-GPU PC may change which adapter is first (not verified).
- **MPEG-4 Part 2** (DivX, Xvid) is not in `current-mpv`'s codec table, so the device profile does not offer it and the server converts
  it; the page shows that. Adding it to the table would change direct play and is a separate decision.
- **Containers and subtitles** are what the engine reports, not what the server would offer for a given file.
- macOS and Linux have no hardware test yet: they pass no facts, and video shows "Plays".

## Consequences
- `Codec::MPEG4` joins the open value (`mpeg4`), for the page's MPEG-4 Part 2 row.
- `current-jellyfin` exposes `transcode_targets` and `external_subtitle_formats` over its profiles.
- The first visit to the page in a run starts libmpv headless for a moment; later visits answer at once.

## Sources (read 2026-10-26)
- mpv v0.41.0: `DOCS/man/options.rst` (`--hwdec`, `--hwdec-codecs`, `--audio-spdif`, `--d3d11-adapter`), `video/decode/vd_lavc.c`
  (`hwdec_codecs` default, `hwdec_codec_allowed`, `hwdec_autoprobe_info`, `hwdec_compare`), `video/out/gpu/d3d11_helpers.c` (`mp_get_dxgi_adapter`,
  `mp_d3d11_create_present_device`), `filters/f_decoder_wrapper.c` (`audio-spdif` option and defaults).
  https://github.com/mpv-player/mpv/tree/v0.41.0
- FFmpeg n8.0: `libavcodec/dxva2.c` (`dxva_modes`, the DXVA GUIDs, `d3d11va_map_sw_to_hw_format`). https://github.com/FFmpeg/FFmpeg
- learn.microsoft.com: `ID3D11VideoDevice::GetVideoDecoderProfileCount`, `GetVideoDecoderProfile` (updated 2024-02-22),
  `CheckVideoDecoderFormat` (updated 2021-10-13); `D3D11CreateDevice`; `D3D11_CREATE_DEVICE_FLAG` (`D3D11_CREATE_DEVICE_VIDEO_SUPPORT =
  0x800`); `DXGI_FORMAT` (ms.date 2025-01-15: `DXGI_FORMAT_NV12 = 103`, `DXGI_FORMAT_P010 = 104`); `IDXGIFactory1::IsCurrent`;
  "Guidelines for app settings" (ms.date 2026-04-08: `IsClickEnabled` "makes the entire card clickable, which is useful for
  navigation-style entries"); "Selector bar" and "Breadcrumb Bar" (ms.date 2025-02-26); `XamlRoot.ContentIslandEnvironment` and
  `ContentIslandEnvironment.AppWindowId` ("The ID of the top-level Win32 HWND").
- Jellyfin v12.2 (commit ca0f16eb7c195f720a1493ed469a27c4657db0c6): `MediaBrowser.Model/Dlna/StreamBuilder.cs`
  (`GetSubtitleProfile`, `GetExternalSubtitleProfile`), `MediaBrowser.Model/Entities/MediaStream.cs` (`IsTextFormat`,
  `SupportsSubtitleConversionTo`); `Jellyfin.Api/Helpers/MediaInfoHelper.cs` as read for docs/notes/playback.md (2026-10-25).
- mingw-w64 headers (master): `d3d11.idl` (the `D3D11_DECODER_PROFILE_*` GUIDs, `ID3D11VideoDevice`'s method order and IID,
  `D3D11_SDK_VERSION = 7`), `dxgi.idl` (`IDXGIFactory1`, `IDXGIAdapter1`, `DXGI_ADAPTER_DESC1`), `dxva.h` (`DXVA_ModeAV1_VLD_Profile0`).
  The Windows SDK's own headers were not read.
- microsoft-ui-xaml (main): `controls/dev/Breadcrumb/BreadcrumbBar_themeresources.xaml` (`BreadcrumbBarItemThemeFontSize`,
  `BreadcrumbBarItemFontWeight`, `BreadcrumbBarChevronFontSize`).
