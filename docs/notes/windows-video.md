# D2 spike notes: Windows video (2026-10-22)

Decision: `docs/decisions/0007-windows-video.md`. The prototype was removed from the branch; its last commit is `181dce5`
(`spikes/windows-video/`: the WinUI app, the media generator, the measurement harness and the media session probe).

## Setup
| Item | Version |
|---|---|
| Windows | Windows 11 Pro 25H2, build 26200.9711 |
| CPU | AMD Ryzen 9 9950X3D (16 cores, 32 threads) |
| GPU | NVIDIA GeForce RTX 5090, driver 32.0.16.1742 (2026-09-30); AMD Radeon integrated graphics, driver 32.0.21045.5002 (2026-08-17) |
| Display 1 (primary) | Samsung Odyssey G70B, 3840x2160 at 144 Hz, 150 % scaling, Windows HDR on; DXGI: `RGB_FULL_G2084_NONE_P2020`, 10 bpc, peak 390 nit; SDR white 204 nit. On the RTX 5090 |
| Display 2 | Samsung S24E450, 1920x1080 at 60 Hz, 100 % scaling, SDR; DXGI: `RGB_FULL_G22_NONE_P709`, 8 bpc, 270 nit. Cabled to the integrated GPU, yet DXGI lists it under the RTX 5090 (hybrid graphics; observed, not looked into) |
| .NET | SDK 10.0.402, C#, WinUI 3 |
| Windows App SDK | 2.5.1 (WinUI 2.3.9, InteractiveExperiences 2.1.9), unpackaged, self-contained; Microsoft.Windows.SDK.BuildTools 10.0.28000.2705; TFM `net10.0-windows10.0.26100.0` |
| libmpv | galentwww/mpv-windows-builder release `20261003`, `libmpv-lgpl-20261003-x64.zip`, SHA-256 `223db996...a73eb63` (matches the release). mpv `v0.41.0-dev-ge8673660a` (master of 2026-08-28, 1012 commits after v0.41.0), client API 2.5, libplacebo 7.371.0 (`41ac2980`), FFmpeg 9.0.1 statically linked into one `mpv-2.dll` (52 MB). Never committed |
| FFmpeg (local tool, for test media only) | 9.0.2 "full" build from gyan.dev, GPL (`--enable-gpl --enable-version3`); nothing it produced is shipped |

### libmpv licences (from the release and the DLL)
- `build-info.json`: `"gpl": false`, `"ffmpegLicense": "LGPL version 2.1 or later"`; the meson log shows `gpl: false`.
- FFmpeg's configure line has none of `--enable-gpl`, `--enable-version3`, `--enable-nonfree`, and none of those strings is in the DLL.
- `THIRD_PARTY_NOTICES.md` lists mpv, libplacebo, FFmpeg, fribidi, libiconv, mpg123 and soxr as LGPL-2.1-or-later, LAME as
  LGPL-2.0-only, uchardet as MPL-1.1, the rest as permissive (BSD, MIT, ISC, zlib, Apache-2.0, FTL). Under ADR 0003, uchardet needs a
  decision; D6 decides it for our own build.
- The builder's README still describes an older GPL configuration; the release notes and `build-info.json` describe this build.

### Test media
Made by `make_media.py` (prototype commit) with FFmpeg and libx265, 70 s each, 60 fps:
`sdr-1080p60`, `sdr-2160p60` (HEVC Main, BT.709) and `hdr10-1080p60`, `hdr10-2160p60` (HEVC Main10, BT.2020 + PQ, mastering display
P3 primaries / 1000 nit, MaxCLL 1000, MaxFALL 400), 15 Mbit/s at 1080p and 40 Mbit/s at 4K. The HDR picture is PQ-coded directly: a
0-10000 nit ramp, grey patches at 80-4000 nit, colour patches, an SDR test pattern at 203 nit white and a moving box of about 1000 nit.
Two audio tracks (stereo AAC 440 Hz tone; 5.1 AC-3 660 Hz beeps), an ASS track (styles, `\move`, `\fad`, rotation, karaoke) and a PGS
track written by the script (FFmpeg has no PGS encoder). A real file from Jellyfin was not used.

## How the prototype works
- mpv options: `vo=gpu-next`, `gpu-api=d3d11`, `gpu-context=d3d11`, `d3d11-output-mode=composition`,
  `d3d11-composition-size=<panel pixels>` (set before `mpv_initialize`), `hwdec=auto-safe`, `force-window=yes`, `idle=yes`,
  `keep-open=yes`, and `osc`, default bindings, VO keyboard input and ytdl off.
- After start the UI polls `display-swapchain` (it appeared about 100-200 ms after `mpv_initialize`), takes a reference, calls
  `ISwapChainPanelNative::SetSwapChain` and `IDXGISwapChain2::SetMatrixTransform(1/CompositionScale)`. mpv's swap chain:
  `FLIP_SEQUENTIAL`, 3 buffers, alpha mode unspecified, `R10G10B10A2_UNORM` after libplacebo's first reconfiguration.
- Colour: see ADR 0007. On the HDR display libplacebo logged "New swap chain configuration received from hint: format:
  R10G10B10A2_UNORM, color space: RGB_FULL_G2084_NONE_P2020"; `video-target-params` for HDR10 input was `rgb10a2 bt.2020 pq`,
  `max-luma 390`.
- Controls: a Fluent acrylic bar (seek slider with times, back 10 s, play/pause, forward 30 s, audio and subtitle menus, full screen)
  over the video; hides after 3 s of playback unless the pointer is over it or a menu is open. Mica title bar with the system caption
  buttons (`ExtendsContentIntoTitleBar` + `SetTitleBar`). Full screen through `AppWindow.SetPresenter(FullScreen)`.
- Media session: `SystemMediaTransportControlsInterop.GetForWindow(hwnd)`; title from `media-title`, artwork from mpv's
  `screenshot-to-file`, timeline updates every 5 s and after every seek.

## Measurements
### Case 1: playback load (60 s after a 3 s warm-up, hwdec d3d11va in every run, colour mode auto on the HDR display)
CPU is the app's share of the whole machine (32 logical CPUs). GPU is Windows' per-process "GPU Engine" utilisation, sampled once a
second: the app's 3D and video decode engines, and dwm.exe's 3D engine.

Windowed, composition 1578x855 at 150 %:
| File | VO drops | Decoder drops | Delayed | Mistimed | CPU % | App 3D % | Video decode % | DWM 3D % |
|---|---|---|---|---|---|---|---|---|
| sdr-1080p60 | 0 | 0 | 0 | 0 | 0.29 | 1.3 | 3.9 | 6.0 |
| hdr10-1080p60 | 0 | 0 | 0 | 0 | 0.35 | 1.7 | 4.2 | 6.0 |
| sdr-2160p60 | 0 | 0 | 0 | 0 | 0.36 | 2.1 | 13.0 | 5.9 |
| hdr10-2160p60 | 0 | 0 | 0 | 0 | 0.39 | 2.5 | 13.0 | 6.0 |

Full screen, composition 3840x2159:
| File | VO drops | Decoder drops | Delayed | Mistimed | CPU % | App 3D % | Video decode % | DWM 3D % |
|---|---|---|---|---|---|---|---|---|
| sdr-1080p60 | 0 | 0 | 0 | 0 | 0.25 | 2.0 | 4.4 | 1.4 |
| hdr10-1080p60 | 0 | 0 | 0 | 0 | 0.34 | 2.6 | 4.5 | 1.4 |
| sdr-2160p60 | 0 | 0 | 0 | 0 | 0.26 | 2.6 | 14.1 | 1.5 |
| hdr10-2160p60 | 0 | 0 | 0 | 0 | 0.51 | 3.6 | 14.3 | 1.5 |

The windowed runs had no `display-fps` (mpv cannot see the output in composition mode); the full-screen runs passed 144 Hz in with
`display-fps-override`. Interactive runs showed a few VO drops (3-7) around start-up and seeks, outside the measured window.

### Case 2: HDR output
- HDR display, Windows HDR on: swap chain `R10G10B10A2_UNORM` + `RGB_FULL_G2084_NONE_P2020`, target `bt.2020 pq`, peak 390 nit,
  SDR white 204 nit. SDR files on the HDR display also go out as PQ with SDR at the SDR white level.
- The owner checked by eye (2026-10-22) and reported that everything worked: HDR highlights, the alternative modes (HDR passthrough,
  scRGB, forced SDR), SDR video with HDR on, switching Windows HDR off and on during playback, and moving the playing window between the
  two monitors. No per-item notes were taken.
- SDR monitor: HDR10 input is tone-mapped to `bt.709 gamma2.2`, 203 nit; swap chain stays `RGB_FULL_G22_NONE_P709`; 0 drops in 30 s.

### Case 3: XAML over video
Controls, menus and the info panel draw over the video with no airspace issue. The owner reported no flicker, Narrator reading the
controls by name, and natural keyboard focus (2026-10-22).

### Case 4: resize, maximize, full screen
Scripted run while playing `hdr10-2160p60`: five rounds of maximize, full screen, restore and 60 resize steps of 16 ms (programmatic,
not the user's sizing loop). Composition sizes always matched the panel.
| Monitor | Duration | VO drops | Decoder drops | Delayed | Sizes seen |
|---|---|---|---|---|---|
| 150 % (4K) | 36.5 s | 23 | 0 | 0 | max 3840x1955, full 3840x2159, windowed 1578x855 |
| 100 % (1080p) | about 36 s | 16 | 0 | 0 | max 1920x941, full 1920x1079, windowed 984x503 |
Full screen is one row short at both scales. Live resizing by dragging a window edge was checked by the owner only (reported fine).

### Case 5: subtitles and audio
ASS rendered with its styling (outline, colour override, `\move`, top-left positioning); PGS rendered bitmaps with outline at the
bottom and top. Switching to the 5.1 AC-3 track reopened WASAPI in about 70 ms ("AO: [wasapi] 48000Hz 5.1(side) 6ch float").

### Case 6: media keys and the media flyout
Read through `GlobalSystemMediaTransportControlsSessionManager` (`smtc-probe.ps1` in the prototype commit): session
`VideoSpike.exe`, title, subtitle, type Video, artwork present, play/pause and seek enabled. Toggling from the session and the
Play/Pause media key both reached the app; a seek request from the session moved playback, and the reported position followed once
the timeline was refreshed after each seek. The owner confirmed the flyout (2026-10-22).

### Case 7: fallbacks
Not needed: no case failed in composition mode, so neither the D3D11 render API nor the child-window route was tried.

## What failed or surprised
- `target-colorspace-hint=auto` does nothing in composition mode: mpv's D3D11 context reports no display. The host must pass the
  display in (ADR 0007).
- `display-fps` is empty unless set; `override-display-fps` is deprecated in this mpv for `display-fps-override`.
- A seek from the media flyout left a stale position until the timeline was refreshed on `playback-restart`.
- `dotnet workload list` on a fresh SDK ran the .NET first-run step and created an untrusted ASP.NET HTTPS development certificate in
  the owner's user certificate store.
