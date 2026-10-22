# 0007: Windows video: libmpv in D3D11 composition mode under a WinUI 3 SwapChainPanel

Status: accepted (2026-10-22), from the D2 spike (docs/roadmap.md). Measurements and setup: `docs/notes/windows-video.md`.

## Context
ADR 0001 puts our own player controls over the video on every platform, and docs/architecture.md picks libmpv (`vo=gpu-next`,
D3D11) as the Windows engine. The open question was whether libmpv can draw into a WinUI 3 `SwapChainPanel` with XAML controls over it
and real HDR10 output. mpv 0.41.0 (2025-12-21) added a D3D11 "composition" output mode for this; nobody had shown it end to end with
HDR. The spike built a WinUI 3 prototype (Windows App SDK 2.5.1, .NET 10) over a prebuilt LGPL libmpv (mpv master of 2026-08-28,
past 0.41.0) and tested it on a 4K HDR monitor and a 1080p SDR monitor.

## Decision
- **Route:** libmpv with `vo=gpu-next`, `gpu-api=d3d11`, `gpu-context=d3d11`, `d3d11-output-mode=composition`, `hwdec=auto-safe`
  (d3d11va in practice). mpv creates a composition swap chain and presents it on its own VO thread; the Windows UI reads the
  `display-swapchain` property and hands the pointer to `ISwapChainPanelNative::SetSwapChain`. Our XAML controls are ordinary elements
  over the panel; there is no airspace problem because XAML composes the swap chain itself.
- **The UI reports the display; the engine decides the colour output.** In composition mode mpv cannot see the monitor (its D3D11
  context returns no target colour space, no SDR white level and no output for timing), so `target-colorspace-hint=auto` does nothing
  and `display-fps` stays unknown. The Windows UI reports, for the monitor the window is on, whether it is in HDR mode, its peak and
  SDR white luminance (`Microsoft.Graphics.Display.DisplayInformation.CreateForWindowId`) and its refresh rate, again whenever the
  window changes monitor or `AdvancedColorInfoChanged` fires. The engine maps that to mpv options:
  - HDR display: `target-colorspace-hint=yes`, `target-colorspace-hint-mode=target`, `target-trc=pq`, `target-prim=bt.2020`,
    `target-peak=<display peak>`, `hdr-reference-white=<SDR white>`. libplacebo then switches the swap chain to `R10G10B10A2_UNORM`
    with `DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`, and SDR video is placed at the user's SDR white.
  - SDR display: `target-colorspace-hint=no` and `target-*` on `auto`: mpv tone-maps HDR to SDR.
  - Always `display-fps-override=<refresh rate>`.
  This mapping is logic, so it lives in the core (`current-mpv`), not in the C# UI (ADR 0001, ADR 0002).
- **Fallbacks not needed.** Composition mode passed every case, so neither mpv's D3D11 render API (unreleased, mpv 0.42) nor a child
  window with an overlay window was built. The render API stays the option if we ever need to own the swap chain ourselves.

## What crosses the bridge (`current-mpv` / `current-ffi` and the Windows UI)
| Direction | What | Rules |
|---|---|---|
| UI to engine | Composition size in physical pixels: panel `ActualSize` x `CompositionScale` | Must be set before the engine starts (mpv's VO fails without it); later changes are asynchronous (`d3d11-composition-size` triggers a resize on the VO thread) |
| UI to engine | Display facts: HDR or SDR, peak nits, SDR white nits, refresh rate | On start, on monitor change, on `AdvancedColorInfoChanged` |
| Engine to UI | The swap chain (`IDXGISwapChain*`) and a "swap chain changed" notice | mpv's property is a borrowed pointer: the engine must hand out its own reference (`AddRef`), and say when the VO was recreated. With `force-window=yes` the VO, and so the swap chain, exists from start-up |
| UI only | `SetSwapChain` and `IDXGISwapChain2::SetMatrixTransform(1/CompositionScale)` | UI thread; repeat the transform on `CompositionScaleChanged` |
| Engine to UI | State: pause, position, duration, tracks, title, decoder and output info | Events are read on one engine thread and marshalled to the UI thread; the UI never blocks on the engine |
| Shutdown | UI detaches the panel and drops its reference, then the engine stops its event thread and destroys mpv | `mpv_terminate_destroy` blocks until the VO thread is gone |

## Limits
- mpv's own Windows integration is unavailable in composition mode (no window, input, exclusive full screen, automatic ICC profile,
  frame statistics from the output), so the UI and core provide what we need. mpv logs "Failed to query swap chain's output
  information" at start; harmless here, but `video-sync=display-resample` was not tested.
- Each presenter change (maximize, full screen, restore) costs about one dropped frame (16-23 drops over 20 changes and 300 resize
  steps while playing HDR10 4K60).
- Full screen leaves the panel one pixel row short (3840x2159, 1920x1079); to be fixed in the player layout.
- The prebuilt libmpv used here statically links everything into one 52 MB `mpv-2.dll` and includes uchardet (MPL-1.1) and LAME
  (LGPL-2.0-only) by its notices. It was used for the spike only (owner's approval); D6 builds our own (ADR 0003).
- Unpackaged, the media session shows the executable's name ("VideoSpike.exe"): the app needs an explicit AppUserModelID and Start
  menu shortcut (D5). Not verified.
- Tested with synthetic files only (HEVC SDR and HDR10, 1080p60 and 4K60, ASS, PGS, two audio tracks) on one NVIDIA RTX 5090 machine.
  Dolby Vision, HDR10+, 23.976 fps on high refresh rates and other GPUs are untested.

## What M2 must do
1. `current-mpv`: libmpv over its C API (`unsafe` allowed there, ADR 0002) with the options above; an event thread; property
   observation; the colour mapping from display facts; an owned swap chain handle with change notices; composition size updates.
2. `current-ffi`: pass the swap chain as an opaque integer handle plus a release call; display facts and composition size in.
3. Windows player UI: `SwapChainPanel` host (matrix transform, size and scale changes), display facts from `DisplayInformation` and
   monitor changes from `AppWindow.Changed`, full screen through the `AppWindow` full-screen presenter, auto-hiding Fluent controls
   with names for Narrator, keyboard shortcuts, cursor hiding in full screen (not in the spike), and media keys and the media flyout
   through `SystemMediaTransportControlsInterop.GetForWindow`, updating the timeline after every seek.
4. D6: pin an mpv release that has composition mode (0.41.0 or later; check the `display-fps-override` name in that release) and decide
   uchardet and LAME (licence exception or leave them out).
5. Test with real files from a Jellyfin server: direct play over HTTP, Dolby Vision profiles, HDR10+, 23.976 and 24 fps on 60, 120 and
   144 Hz displays, and `video-sync=display-resample`.

## Sources (read 2026-10-22)
- mpv v0.41.0 release (2025-12-21): https://github.com/mpv-player/mpv/releases/tag/v0.41.0
- mpv at commit e8673660ab7e (2026-08-28, the build used): `DOCS/man/options.rst` (`--d3d11-output-mode`, `--d3d11-composition-size`,
  `--target-colorspace-hint`, `--hdr-reference-white`), `DOCS/man/input.rst` (`display-swapchain`), `video/out/d3d11/context.c`
  (composition mode returns no target colour space or reference white), `video/out/gpu/d3d11_helpers.c`
  (`CreateSwapChainForComposition`, flip sequential), `player/command.c` (`d3d11-composition-size` triggers an external resize):
  https://github.com/mpv-player/mpv/tree/e8673660ab7ee5d4ea8f93e4bf3a6e170ab2a19a
- Advanced Color and HDR with DirectX, "XAML supports DirectX interop using SwapChainPanel" (page updated 2022-10-19):
  https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range
- C# interop classes, `Windows.Media.SystemMediaTransportControlsInterop` (page updated 2026-09-10):
  https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-com-interop-csharp
- `ISwapChainPanelNative` (IID 63aad0b8-7c24-40ff-85a8-640d944cc325): `include/microsoft.ui.xaml.media.dxinterop.idl` in
  Microsoft.WindowsAppSDK.WinUI 2.3.9. `DisplayInformation.CreateForWindowId`, `GetAdvancedColorInfo` and `SdrWhiteLevelInNits`:
  `Microsoft.Graphics.winmd` in Microsoft.WindowsAppSDK.InteractiveExperiences 2.1.9 (package metadata; the reference pages were not read)
