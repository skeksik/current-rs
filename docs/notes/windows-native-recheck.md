# Recheck of the W7 fixes on the owner's PC

test/windows-native-recheck, from `origin/main` at f7b847b (W7's fixes #118–#125 and "How to play" #128–#130 merged; #126, the
crash fix, still open). Checked on 2026-11-06 on the owner's Windows 11 PC against the owner's own Jellyfin server, with W7's setup
and test files (`docs/notes/windows-native-player-check.md`, "Setup"). Claude drove the app through screenshots and read the info
panel and the native engine's trace lines, as in W7. The owner then checked by hand what screenshots cannot: ALAC's sound, Windows
HDR off and on, the tool tips, a mouse drag across the monitors, and smoothness on one more file (rows marked "owner"). The owner asked for
this check before any further change, so nothing in the product changed here.

Built with `bindings/csharp/build.ps1` and `apps/windows/build.ps1 -Mpv` (shared D6 libmpv build). The crash check used main with
#126's one file put on top locally (not committed), then main as it is. The stutter numbers used a local build with trace lines
(not committed), as in `docs/notes/windows-native-stutter.md`.

## Results
| W7 item or PR | Check | Result |
|---|---|---|
| #130 Settings | Settings → Playback → "How to play" lists Auto, Native, App engine, Server remux, Server transcode | **Passed** |
| #130 Settings | App engine chosen, app restarted: still App engine; the next items (a native-capable HEVC MKV, the 4K HDR10 file) played on the app engine, "Settings say how to play." | **Passed** |
| #130 player | A How to play button next to Quality; its menu lists the same five, the choice in use checked | **Passed** |
| #130 player | Its tool tip (owner) | **Failed**: none, as on Volume, Subtitles, Audio tracks and Quality (finding 5). Narrator not checked |
| #130 player | App engine → Native while playing: moved at the position, "You chose how to play this.", "Player · native player" | **Passed** |
| #130 player | Server transcode while playing (4K HDR10 at 2:11) | **Passed as designed**: the native engine's first segment took longer than 8 s (`stream_timeout`; Jellyfin transcoded 4K to 4K HEVC at about 61 fps, real time), libmpv played it at the position |
| #128/#130 player | Then Auto, then Native, from that server stream | **Failed**: nothing moved (finding 1) |
| W7 §8, #124 | 4K HDR10 at a third of the file, 1080p 10 Mbit/s while playing | **Passed**: "Server transcode · HLS · mp4 · native player", bar 2:30 of 5:59 (the file's position and duration) |
| W7 §8, #122 | Paused, two quality changes (1080p 6, 720p 4 Mbit/s), then play | **Passed**: Windows' player played the stream ("HLS stream plays"), no `stream_timeout` |
| W7 §8 | A far seek back on that stream (2:43 → 0:2x) | **Passed**: native, within a few seconds |
| W7 ALAC, #118 | "MP4, H.264, ALAC" | **Passed**: app engine, "The native player can't decode ALAC audio."; sound heard, also after switching How to play to the other choices (owner) |
| W7 §11, #119 | "PGS 2 - yellow" on the 1080p PGS file | **Passed**: yellow (W7: cyan); native |
| W7 §11, #119 | The top cue on "1920×804 (2.39:1), PGS (1920×1080 canvas)" | **Passed**: shown at the picture's top (W7: never shown) |
| W7 §10, #120 | External SRT played 14–27 s: cue 4 gone by 20 s, cue 7 alone; then the external WebVTT chosen | **Passed**: one cue at a time. This run started on the English SRT, not the German WebVTT W7 had |
| W7 §13, #120 | Two SRT tracks: English → German → Off → German | **Passed**: the current cue each time, nothing with Off, no stale cue on the way back |
| W7 §10, #125 | Panel line of an external track | **Passed**: "German - WEBVTT - External", once |
| W7 §19, #125 | TrueHD chosen (app engine), then AAC again | **Passed**: "Kept here after the track change, though the native player could play it now." |
| W7 §1, #125 | Player line on the native route | **Passed**: "Player · native player" |
| W7 §20, #118 #121 #125 | Supported formats | **Passed**: ALAC "App engine", MPEG-2 and MP3 without "Not verified", MOV "Windows' player", Dolby Vision 5 and 8 without "Not checked"; AC-3, E-AC-3 and Atmos still "Through the Dolby decoder this PC has" (this PC has one) |
| W7 §7 | The crash sequence of #126 (the HLG file moved across the monitors while playing, then the 4K HDR10 file moved), main as it is | **Failed**: crashed in round 3, `Microsoft.Graphics.Display.dll+0x1c151`, reading 0xfffffffd (the same place as W7's and #126's dumps) |
| W7 §7, #126 | The same sequence, main with #126's `DisplayWatcher.cs` | **Passed**: no crash in 36 round trips, no new dump |
| W7 §7, #126 | The owner dragging the playing 4K HDR10 window by its title bar to the SDR monitor and back, several times, fast and slow (main with #126's file) | **Passed** (owner): no crash, no new dump |
| W7 §7 | Windows HDR off and on again (Win+Alt+B) while "MKV, HEVC Main 10, HDR10, 4K" played (owner) | **Passed** (owner): to the app engine with HDR off, back to Windows' player with HDR on. W7's open point closed |
| Stutter | Start-up stall with an SRT shown ("MKV, H.264, SRT inside") | **Unchanged**: 2.13 s lost in the first 3 s, the file read to 58 s ahead in 3 s |
| Stutter | 4K HDR10 60 fps, no subtitles, 80 s | **Unchanged**: 14 slips, 1.91 s lost (0.04–0.32 s each), with 50–59 s buffered and never `Buffering` |
| Stutter | "MKV, HEVC Main 10, HDR10, 4K" (30 s, 60 fps, no subtitles) on Windows' player (owner, by eye) | **Stutters** too: the same 4K60 HDR10 case as the long file |

## Findings
1. **How to play does not leave a server stream for a local route.** After Server transcode was chosen (and had moved to libmpv),
   choosing Auto and then Native in the player's menu checked the new choice but moved nothing: the route stayed "Server transcode ·
   HLS · mp4 · app engine", and the reasons kept "You chose how to play this." From the code (not traced): `choose_how_to_play`
   keeps the route when the route check's first candidate is the one playing (`crates/playback/src/controller.rs`, the `stays`
   test), and `candidates` returns only server routes while a server stream is planned (same file, `candidates`). So from a server
   stream every local choice "stays". Auto has no chosen reason (`chosen_reason` is None), so the old reason is not replaced either.
   Expected (ADR 0025 choice 3 as amended): Auto or Native starts again at that step, at the position.
2. **Wording.** A server transcode the user chose reads "Direct play failed on the server." among its reasons, which is not what
   happened.
3. **The panel's server line lags.** After two paused quality changes it still showed the previous one (6.1 Mbit/s with 720p 4
   Mbit/s chosen), also after playing for 10 s. Not looked into.
4. **The owner's question: why the menus look translucent over the video and the control bar and the info panel do not.** The bar
   and the panel are elements of the player page with `AcrylicInAppFillColorDefaultBrush` (`PlayerView.xaml`). In-app acrylic
   cannot sample the video, which is external content to the compositor (the comment in `PlayerView.xaml` quotes "Visual layer
   overview" on this), so it shows its tint only. The menus are `MenuFlyout`s, which open as their own popup windows and appear to
   get a backdrop that the desktop compositor draws over everything under them, the video included (inferred from what is seen; the
   flyout's template and `FlyoutBase.SystemBackdrop` in our Windows App SDK were not read). Hosting the bar and the panel in a
   windowed popup with a system backdrop might match the menus; not tried.
5. **Only the buttons with a keyboard shortcut have a tool tip** (owner: none on hover except skip back and forward, play, playback
   info and full screen). Those five declare `KeyboardAccelerators`, and "By default, when keyboard accelerators are declared, all
   controls (except MenuFlyoutItem and ToggleMenuFlyoutItem) present the corresponding key combinations in a tooltip"; for an
   `AppBarButton` "the keyboard accelerator is appended to the control's default tooltip" ("Keyboard accelerators",
   learn.microsoft.com, ms.date 2026-08-28, read 2026-11-06). Volume, Subtitles, Audio tracks, Quality and How to play have
   neither a shortcut nor `ToolTipService.ToolTip` (`PlayerView.xaml`, and nothing in `PlayerView.xaml.cs`), so they show none.
   #130 said How to play's tool tip was set; it is not. Proposed: a tool tip on each, in the core's words for How to play ("How to
   play: Auto").

## Not verified
- The How to play button's Narrator name (and every player button's).
- AC-3 on a PC without the PC maker's Dolby decoder (W7).
- libmpv's placement of the cropped PGS file side by side with the native route's.
