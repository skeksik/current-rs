# 0010: Desktop video engine

Status: accepted (2026-10-19), from the M1 "Desktop video" spike. The open checks and tuning are M2 follow-ups (Consequences).

## Context
M1 needs a Windows desktop engine behind ADR 0007's headless `PlayerEngine`, with our own Compose controls over the video. Compose
Multiplatform cannot draw over native video, so every route is a workaround (docs/platform-notes.md, "Desktop and web playback"). The
roadmap orders the candidates: (1) libmpv drawn into Compose as a GPU texture, as MediaMP does; (2) libmpv in a native window with a
transparent Compose controls window; (3) vlcj. ADR 0007 requires real HDR10 output; ADR 0009 allows only permissive and LGPL-2.1 code.

## Candidates
1. **libmpv as a texture inside Compose (MediaMP 0.5.2): works, but SDR only.** MediaMP renders libmpv through mpv's D3D11 render API
   into `RGBA_8888` textures that Compose's own Direct3D 12 renderer draws, so the controls are plain Compose content in the same window.
   It played everything we tried with few drops (below), but HDR is tone-mapped to SDR by design and Skia presents through an SDR swap
   chain, so it cannot output HDR10. It also reaches Skiko internals by reflection and needs a libmpv with the D3D11 render API, which is
   merged in mpv master (2026-10-15) but in no release yet. Ruled out by HDR alone, as agreed before the spike.
2. **libmpv in a child window, Compose controls in an owned transparent window: chosen.** The video window is a Compose window whose
   content is a heavyweight AWT canvas; libmpv renders into its HWND (`wid`) with `vo=gpu-next`, `gpu-api=d3d11`, `hwdec=auto-safe`
   and `target-colorspace-hint=yes`. The controls are an undecorated, transparent `ComposeDialog` owned by the video window that follows
   the canvas's screen bounds.
3. **vlcj: not tried.** GPLv3+ or a paid licence (ADR 0009), and the same two-window overlay problem as candidate 2.

## Decision
Candidate 2. On the test machine it played 1080p60 and 4K60 HEVC, SDR and HDR10, without dropped frames in steady playback, with real
HDR10 output, ASS and PGS, fast audio switching, and moves between monitors and GPUs. Its problems are in how the window host drives AWT,
each with a fix the spike tried:
- **Fullscreen must be our own.** Compose's `WindowPlacement.Fullscreen` is AWT's full-screen exclusive mode on Windows (Skiko 0.150.1
  calls `GraphicsDevice.setFullScreenWindow`). Leaving it at 150 % display scale, the frame came back 1280×720 *physical* pixels while
  AWT laid out 1280×720 *logical*: the video was clipped and the controls window stuck out past it. Borderless fullscreen on the same
  HWND (strip the frame styles, cover the monitor with `SetWindowPos`, restore both) kept all geometry consistent and dropped no frame
  on the way out. Applying the frame's bounds again after leaving Compose's fullscreen also fixed the geometry, but still dropped frames
  leaving it (one run each). From a **maximized** window, restyling the HWND directly left AWT with the fullscreen bounds after leaving
  (4 of 6 self-test rounds, with direct calls and with F/Esc alike), so content and controls were laid out for the wrong rectangle; the
  host first restores the window through AWT, goes borderless, and maximizes again through AWT on the way out (6 of 6 consistent).
- **The controls window follows the canvas through component listeners.** Sampled every 250 ms in 5 self-test runs, it matched the
  canvas's native rectangle except right after maximizing, where it was 1 physical pixel off at 150 % (rounding) until the next move.
- **Media keys need our own libmpv build** with `win32-smtc` (or our own System Media Transport Controls binding); the build used here
  has none and the app never appears as a media session.
- **Input is the host's job, not Compose's defaults.** Player shortcuts go in the preview phase (`onPreviewKeyEvent`) of both windows,
  so a focused button (Space) or the seek bar (arrows) does not handle the same key again; seeks are `exact` (mpv snaps relative seeks
  to keyframes otherwise, which looked like random jumps); a click on the video acts on release, with our own double-click detection
  (`detectTapGestures` with `onDoubleTap` delays every single tap by the double-tap timeout).
- **The controls window is not resizable** (an undecorated dialog keeps resize borders) and is clipped to Windows 11's rounded bottom
  corners while floating.
- **The controls window follows the frame's content area, not the canvas**, and is validated after every `setBounds`. AWT resizes the
  content area at once, while the canvas follows after Compose's next layout and came out 2 px taller at 150 % (covering the frame's
  bottom border). Without `validate()` the Compose content kept its old size during a drag-resize; with it, half way through a real
  mouse drag the window and its content were both 1674×912 px.
- **Idle hiding ignores pointer moves that did not move**: hiding the controls window makes Windows send a mouse move to the window
  below, which brought the controls straight back in a loop.

The engine is libmpv in-process (JNA in the spike) behind `PlayerEngine`; the window host (video window, canvas, controls window,
borderless fullscreen) is desktop-specific; the controls stay ordinary Compose code in `composeUi`.

## Measurements
Windows 11 Pro (build 26200), AMD Ryzen 9 9950X3D, NVIDIA GeForce RTX 5090 (driver 32.0.16.1742) on a 3840×2160 144 Hz HDR display at
150 % with Windows HDR on, and an AMD Radeon integrated GPU (driver 32.0.21045.5002) on a 1920×1080 60 Hz SDR display at 100 %. JDK 21
(Zulu 21.0.11), Compose Multiplatform 1.12.1 (Skiko 0.150.1), libmpv 0.41.0 and FFmpeg 8.0.1 from MediaMP's runtime jar, prototype at
`spike/desktop-video`, 2026-10-19. Synthetic files: 90 s HEVC at 60 fps from `make-test-media.ps1` (1080p 20/25 Mb/s, 4K 60/80 Mb/s,
SDR 8-bit and HDR10 Main10). Real files over Jellyfin 12.2 direct play: a 4K HDR10 HEVC film (20 Mb/s, TrueHD 7.1 and E-AC-3, PGS) and a
1080p HEVC 10-bit episode with typeset ASS (Opus). Windowed runs at 1280×720 logical. CPU % is the player process (all cores = 100 %);
GPU % is its share of the 3D and video-decode engines from Windows' GPU Engine counters. `hwdec=auto-safe` picked `d3d11va` every time.

| Case | Candidate 2 (child window) | Candidate 1 (texture) |
|------|----------------------------|-----------------------|
| 1080p60 HEVC SDR / HDR10, ~56 s each | 0 dropped, 0 delayed; CPU 0.8 %; GPU decode ~5 %, 3D 4-7 % | not measured |
| 4K60 HEVC SDR, ~56 s | 0 dropped; CPU 0.9 %; decode 14 %, 3D 4 % | 2 dropped at start, then 0; CPU 0.7 %; decode 16 %, 3D 5 % |
| 4K60 HEVC Main10 HDR10, ~56 s | 0 dropped; CPU 0.9 %; decode 15 %, 3D 5 % | 3 dropped at start, then 0; CPU 0.7 %; decode 23 %, 3D 8 % |
| 4K60 HEVC Main10 HDR10, 167 s looped | 0 dropped, 0 delayed, A/V sync within 0.01 ms; CPU avg 0.9 % (max 2 %) | not measured |
| 4K HDR10 film, Jellyfin, ~46 s | 0 dropped; CPU 0.9 %; decode 13 % | ~70 s while an FFmpeg encode ran: 5 dropped at start; CPU 1 %; decode 9 % |
| HDR10 output, Windows HDR on | swap chain in `bt.2020/pq`, display peak 390 nits reported | tone-mapped to SDR (by design) |
| Second monitor (SDR, 60 Hz, other GPU, 100 %) | output switched by itself to `bt.709/gamma2.2` at 203 nits; same HWND, mpv not restarted; geometry consistent | not measured |
| ASS (synthetic styles; typeset episode) | styles, rotation, positioning and fonts drawn | menus work; not checked by eye |
| PGS (4K film) | drawn at the right size over HDR output | not measured |
| Audio track switching (AAC 2.0, AC-3 5.1, E-AC-3 2.0) | new track playing 19-92 ms after the switch (`audio-pts` advancing); 1 dropped frame per switch | not measured |
| Resizing (5 sizes in 3.5 s), maximize | 0-7 dropped across the five resizes, 1-2 on maximize (5 runs), while the swap chain is rebuilt; controls window aligned | not measured |
| Fullscreen, Compose's (exclusive) | 0-2 dropped entering, 2-5 leaving (3 runs); wrong frame size after leaving (see Decision) | not measured |
| Fullscreen, borderless (Win32) | 2 dropped entering, 0 leaving (1 run); fills the monitor, controls on top, geometry consistent | not measured |
| Windows media keys | the app is not listed among Windows' media sessions (only the browser was), so keys go elsewhere; libmpv built without `win32-smtc` | same libmpv |
| Jellyfin direct play | file loaded 0.4-2.2 s after mpv started, including a start position mid-file; 0 drops | starts and plays |
| Stability | 9 playback runs and 6 self-tests, no crash | 3 runs, no crash |

mpv resets `frame-drop-count` on seeks, so drops were counted as the sum of increases in the per-second samples.

## Licences
- **Prototype libmpv and FFmpeg**: MediaMP's `mediamp-mpv-runtime-windows-x64` 0.5.2 (POM: Apache-2.0 and LGPL-2.1-or-later). Read
  from the DLLs: libmpv's configuration has `-Dgpl=false` and `-Dwin32-smtc=disabled`; `avcodec` and `avutil` report "LGPL version 2.1 or
  later" and their configuration has no `--enable-gpl`, `--enable-version3` or `--enable-nonfree`. Its FFmpeg is configured with
  `--disable-everything` and 51 decoders: no VP8, MPEG-1, H.263, FLV1, Theora, MJPEG, DVB subtitles, XSUB, WavPack, APE, TTA, TAK, and no
  MPEG-PS/VOB, FLV or WAV demuxers, so it falls short of ADR 0007's format list.
- **Prebuilt LGPL libmpv for Windows is LGPLv3**: zhongfly's `mpv-dev-lgpl` links FFmpeg statically under LGPLv3 (its README), and mpv's
  own rolling LGPL builds are described as LGPLv3 (release notes of `git-release`). Neither meets ADR 0009.
- **Shipped builds**: our own LGPL-2.1 build of libmpv (`-Dgpl=false`, `win32-smtc` on) and FFmpeg (without `--enable-gpl` and
  `--enable-version3`, with the codec list of ADR 0007), as replaceable DLLs, with sources and build scripts published per release.
- **Spike-only dependencies** (removed with the prototype): `org.openani.mediamp:mediamp-mpv-runtime-windows-x64:0.5.2` (Apache-2.0 +
  LGPL-2.1+, its DLLs), `org.openani.mediamp:mediamp-mpv-desktop:0.5.2` (Apache-2.0, candidate 1 only), `net.java.dev.jna:jna:5.19.1`
  (LGPL-2.1+ or Apache-2.0). FFmpeg (Gyan's full build, GPL) was used only as a local tool to make test files and is not part of any build.
- **Patents** stay a per-codec check (ADR 0009): with `d3d11va` the GPU decodes HEVC, but FFmpeg's software HEVC and AAC decoders still
  ship in the DLLs.

## Consequences
- M2 builds the desktop engine on libmpv: our LGPL-2.1 libmpv/FFmpeg build and its CI, the JVM binding (JNA, or Java's FFM API on a newer
  JDK), and the window host with borderless fullscreen. macOS and Linux desktops need their own check of the `wid` route.
- Two top-level windows stay visible to the system (focus, title bar state, Alt+Tab); the hand checks below say how much that shows.
- Every resize and fullscreen switch costs a few frames while mpv rebuilds its swap chain.
- Media keys and the Windows media flyout come with `win32-smtc` in our build; until then they do not work.
- HDR-to-SDR tone mapping (Windows HDR off, SDR monitors) needs a visual check against reference players.
- M2 follow-ups from the spike: check by hand Alt+Tab, the taskbar and the title bar state with two windows, and clicking outside
  borderless fullscreen; compare tones against a reference (the film's SDR release or the OS player) with Windows HDR on and off before
  tuning `target-peak`, `tone-mapping` and `hdr-contrast-recovery`; keyframe seeks while a seek key repeats, an exact seek when it stops.

## Hand checks
Reported 2026-10-19 on the first prototype (`spike/desktop-video-old`: Compose's fullscreen, keys after the focused control, keyframe
seeks), each with what the current prototype does about it:

| Reported | Cause | Now |
|----------|-------|-----|
| Controls window misaligned and sticking out after leaving fullscreen | AWT's exclusive full-screen mode restores the frame at the wrong size (see Decision) | borderless fullscreen by default; geometry consistent in the self-test |
| Fullscreen window minimized or hidden when clicking outside it; Alt+Tab from fullscreen lags | exclusive full-screen mode minimizes on focus loss (the log shows iconified/restored pairs) | borderless fullscreen is a normal window; to re-check by hand |
| H mode turns fullscreen off when the controls hide | hiding the owned controls window moves focus, which ends exclusive mode | to re-check by hand with borderless fullscreen |
| Controls window can be resized by hand; its corners stick out past the rounded window | undecorated dialog still resizable; square overlay over a rounded window | not resizable; bottom corners clipped while floating |
| One seek action seeks twice, or by a "random" amount | keys handled by the focused seek bar and by our handler; mpv's keyframe snapping on relative seeks | preview-phase shortcuts and `exact` seeks; self-test: one Right arrow with the seek bar focused moved 4.7 s (5 s minus timing) |
| Space activates the last pressed button | Space reached the focused button before our handler | self-test: Space with a button focused only toggled play/pause |
| Clicking the video pauses with a delay | single taps waited for the double-tap timeout | self-test: pause 167 ms after the click (including the robot's own delays); double click still goes fullscreen without changing play/pause |
| Slightly different tones from VLC (HDR film, Windows HDR on) | not established. mpv tone-maps to the peak it reads for the display (390 nits here); VLC may leave that to the display | compare with `-Pspike.mpvOptions=target-peak=<display peak>` and with a reference player before tuning |
| Media keys do not work | no `win32-smtc` in this libmpv | expected; needs our own build |

Second round, 2026-10-19, on the current prototype:

| Reported | Cause | Now |
|----------|-------|-----|
| Audio switching | | works correctly by ear |
| Overlay still shades the window's bottom border | the controls window followed the canvas, 2 px taller than the content area | follows the content area |
| Controls do not resize with a drag-resize (content cut off at the right) | the dialog's bounds changed, its Compose content was not laid out again | `validate()` after `setBounds`; self-test drag through the real border: content matches half way and after |
| H mode: controls keep coming back while the pointer rests on the video | synthetic mouse move after hiding counted as activity | only real pointer movement counts; self-test: hidden after 8 s with the pointer still (shown once again early on) |
| Seeks are accurate but lag when pressed repeatedly | each `exact` seek decodes from the previous keyframe; over Jellyfin a seek took about 0.6 s (overlay) and repeated presses queue | engine tuning for M2: keyframe seeks while a key repeats, an exact seek when it stops |
| Windows HDR off: VLC much brighter | mpv tone-maps to SDR with a 203-nit target (`bt.709/gamma2.2/203` in the overlay); VLC's mapping differs. Which is closer to the master is not known | needs a reference (the film's SDR release, or the OS player) before tuning `target-peak`, `tone-mapping`, `hdr-contrast-recovery` |

Third round: entering fullscreen from a maximized window and leaving with F, Esc or a double click misaligned window and controls (the
button happened to work). Cause and fix: see Decision (fullscreen from a maximized window); reproduced and fixed in the self-test.

Not checked by hand before acceptance: Alt+Tab, taskbar and title bar state with two windows; clicking outside borderless fullscreen;
tones against a reference. They are M2 follow-ups (Consequences).

## Prototype
`spikes/desktop-video` up to commit `f07c629` on branch `spike/desktop-video` (in this ADR's PR history), removed before merging. Its
README held the run steps; `make-test-media.ps1` made the synthetic files; the self-test drove resizing, monitors, fullscreen, tracks and
real mouse and keyboard input. This ADR and docs/platform-notes.md are what remains.
