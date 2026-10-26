# 0021: The "Supported formats" page and its hardware test

Status: accepted (2026-10-26); layout and approach decided by the owner 2026-10-26 (see "Decided by the owner").

## Context
Users ask what a client plays before they ask the server to convert it. The Windows app had no way to show it: the engine's
capabilities were read only when a player started (`crates/app/src/player/mpv.rs`), and nothing said which codecs the graphics card
decodes. The engine plays with mpv's `hwdec=auto-safe`, which uses d3d11va on Windows (ADR 0007, ADR 0019), so whether a codec is
decoded in hardware depends on the card's Direct3D 11 decoder profiles and on mpv's own rules.

## Decided by the owner (2026-10-26)
1. **Layout: a Settings sub-page, not a dialog.** Under Settings → Playback, a clickable SettingsCard "Supported formats" with the
   chevron opens a page inside Settings, as Windows Settings opens its nested pages. The page's title is a breadcrumb ("Settings >
   Supported formats") and Back returns to Settings. A SelectorBar holds five tabs: Video, Audio, Containers, Subtitles, HDR. Each tab is
   a list with column headers: Codec (or Format), On this PC, Notes. Dolby goes inside the tabs: Dolby Vision under HDR; TrueHD, E-AC-3
   and Atmos under Audio. Above the tabs, a short description says that results also depend on the file, the server's route and the
   display. The owner's mock:
   ```
   Settings > Supported formats
   [ Video ] Audio  Containers  Subtitles  HDR
   Codec            On this PC     Notes
   H.264 (AVC)      Hardware
   HEVC (H.265)     Hardware       8-bit and 10-bit
   AV1              Software       No hardware decoder on this GPU
   VP9              Hardware
   MPEG-4 Part 2    Software       DivX, Xvid
   ```
2. **Hardware column: test this GPU** through Direct3D 11's video decoder profiles (`GetVideoDecoderProfileCount`,
   `GetVideoDecoderProfile`, `CheckVideoDecoderFormat`) for H.264, HEVC Main and Main10, VP9 profiles 0 and 2, AV1 Main, MPEG-2 and
   VC-1. A codec with a profile is "Hardware", one without "Software", within what `auto-safe` allows.
3. **Built in the cloud; the owner looks at it on their PC before the merge.**

## Decision

### Where the logic is
The core builds the whole page (`current-app`'s `formats`, exposed by `current-ffi` as `Core::supported_formats`): tabs, rows, the
support value and the notes. The Windows page runs two platform tests, passes their plain facts in, and renders what comes back. The
rows come from three inputs and never from a fixed list of what should work:
- **The engine's capabilities** (`EngineCapabilities`, from libmpv's `decoder-list` and `demuxer-lavf-list`, `current-mpv`'s `caps.rs`).
  A codec the engine does not report is never shown as playing on this PC. Tables in `formats.rs` only name and order rows; a value the
  engine reports that they lack gets a row under its own name (open values).
- **The server's transcode targets**, read from the transcoding profiles the device profile sends (`current_jellyfin::transcode_targets`
  over `profile.rs`'s `FMP4_*` and `TS_*`, not copies): what the engine cannot decode, the server converts to those.
- **The facts the UI passes in:** the graphics card's decoder profiles, and the display the window is on (the same `DisplayFacts` the
  player takes, ADR 0007).

The column's values: **Hardware**, **Software**, **Plays** (containers, subtitles; video when the card could not be tested),
**Server repackages** (a container the engine does not read: the server remuxes into HLS), **Server converts** (transcode),
**Not supported**, and for the HDR tab **HDR** and **Tone-mapped**.

### The engine's capabilities without playing
The app keeps the capabilities of the last engine start in the run (every player start records them). When none is known, a short-lived
**headless engine** is started on the blocking pool only to read them: `current-mpv`'s `Output::Headless`, which sets `vo=null` and
`ao=null` and no `force-window`, so no window and no audio device opens. It is shut down at once, and its answer is kept for the run;
a failed start is not kept, so opening the page again retries. It shares the process's one engine slot (ADR 0019), so a player starting
at the same moment waits for it as it waits for a closing one.

Considered: caching the capabilities on disk from the last run (stale after an update of the bundled libmpv, and empty on first start);
a fixed list per build (would claim what a build lacks, against the brief). The headless start costs one libmpv initialisation per run
(not measured; libmpv starts no VO with `vo=null`).

### The hardware test (Windows)
In C# (`Player/DecoderTest.cs`), through COM vtables and delegates as the player's interop is (no package, no unsafe code):
`CreateDXGIFactory1`, `IDXGIFactory1::EnumAdapters1(0)`, `D3D11CreateDevice` on that adapter with `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`,
`ID3D11VideoDevice`, then every profile from `GetVideoDecoderProfile` with `CheckVideoDecoderFormat` for `DXGI_FORMAT_NV12` (8-bit)
and `DXGI_FORMAT_P010` (10-bit). The UI passes the adapter's name and each profile's GUID and two booleans; which codec a GUID decodes
is the core's to say. It runs off the UI thread, once per app run, and again when `IDXGIFactory1::IsCurrent` reports that the adapters
changed, which is one call when the page opens.

The core's rule (`formats.rs`), following what the engine does:
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
the facts of the display the window is on (`DisplayWatcher.ReadOnce`) and shows "HDR" or "Tone-mapped"; without them, "Plays" and
"depending on the display". HDR10+ and Dolby Vision profiles 5, 7 and 8 say "Not checked on a device yet": no such files were played on
the owner's PC (docs/notes/windows-video.md, "ADR 0007 item 5"). HLG and HDR10 were.

## Limits
- **The test asks the driver, not a decode.** A listed profile can still fail on a given file (level, resolution, a driver bug), and
  mpv then falls back to software. The page does not claim more than "the card has a decoder for it".
- **One adapter:** DXGI's first, taken to be the default adapter mpv's D3D11 output picks when `--d3d11-adapter` is unset ("Will pick
  the default adapter if unset"; d3d11va is "affected by this choice", `DOCS/man/options.rst`, v0.41.0). That mpv's default is DXGI's
  adapter 0 was not checked in mpv's source. On a PC with two graphics cards, Windows' per-app graphics preference may change it (not
  verified).
- **MPEG-4 Part 2** (DivX, Xvid) is not in `current-mpv`'s codec table, so the device profile does not offer it and the server converts
  it; the page shows that. Adding it to the table would change direct play and is a separate task.
- **Containers and subtitles** are what the engine reports, not what the server would offer for a given file.
- macOS and Linux have no hardware test yet: they pass no facts, and video shows "Plays".

## Consequences
- `Codec::MPEG4` joins the open value (`mpeg4`), for the page's MPEG-4 Part 2 row.
- `current-jellyfin` exposes `transcode_targets` over its transcoding profiles.
- The first visit to the page in a run starts libmpv headless for a moment; later visits answer at once.

## Sources (read 2026-10-26)
- mpv v0.41.0: `DOCS/man/options.rst` (`--hwdec`, `--hwdec-codecs`, `--audio-spdif`, `--d3d11-adapter`), `video/decode/vd_lavc.c`
  (`hwdec_codecs` default, `hwdec_codec_allowed`), `filters/f_decoder_wrapper.c` (`audio-spdif` option and defaults).
  https://github.com/mpv-player/mpv/tree/v0.41.0
- FFmpeg n8.0: `libavcodec/dxva2.c` (`dxva_modes`, the DXVA GUIDs, `d3d11va_map_sw_to_hw_format`). https://github.com/FFmpeg/FFmpeg
- learn.microsoft.com: `ID3D11VideoDevice::GetVideoDecoderProfileCount`, `GetVideoDecoderProfile` (updated 2024-02-22),
  `CheckVideoDecoderFormat` (updated 2021-10-13); `D3D11CreateDevice`; `D3D11_CREATE_DEVICE_FLAG` (`D3D11_CREATE_DEVICE_VIDEO_SUPPORT =
  0x800`); `DXGI_FORMAT` (ms.date 2025-01-15: `DXGI_FORMAT_NV12 = 103`, `DXGI_FORMAT_P010 = 104`); `IDXGIFactory1::IsCurrent`;
  "Guidelines for app settings" (ms.date 2026-04-08: `IsClickEnabled` "makes the entire card clickable, which is useful for
  navigation-style entries"); "Selector bar" and "Breadcrumb Bar" (ms.date 2025-02-26); `XamlRoot.ContentIslandEnvironment` and
  `ContentIslandEnvironment.AppWindowId` ("The ID of the top-level Win32 HWND").
- mingw-w64 headers (master): `d3d11.idl` (the `D3D11_DECODER_PROFILE_*` GUIDs, `ID3D11VideoDevice`'s method order and IID,
  `D3D11_SDK_VERSION = 7`), `dxgi.idl` (`IDXGIFactory1`, `IDXGIAdapter1`, `DXGI_ADAPTER_DESC1`), `dxva.h` (`DXVA_ModeAV1_VLD_Profile0`).
  The Windows SDK's own headers were not read.
- microsoft-ui-xaml (main): `controls/dev/Breadcrumb/BreadcrumbBar_themeresources.xaml` (`BreadcrumbBarItemThemeFontSize`,
  `BreadcrumbBarItemFontWeight`, `BreadcrumbBarChevronFontSize`).
