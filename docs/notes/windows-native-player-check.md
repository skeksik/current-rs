# W7: Windows' own player in Current, on the owner's PC (ADR 0025)

test/windows-native-player-check, from `origin/main` at 7a50ec5 (W2 to W6: PRs 107, 108, 110, 112, 114, 115, 116). Checked on
2026-10-31 on the owner's Windows 11 PC against the owner's own Jellyfin server. Claude drove the app through screenshots (computer
use) and read the playback info panel, the engine's trace lines and the system media session. The owner judged picture and sound,
changed Windows settings, pressed the keyboard's media keys and recorded the screen once. Decision: `docs/decisions/0025-native-player-first.md`;
the W1 spike before it: `docs/notes/windows-native-player.md`.

## Setup
| Item | Version |
|---|---|
| Windows | Windows 11 Pro 26H2, build 26300.9711 |
| HDR monitor (primary) | HDR10-only DisplayHDR 400, 3840×2160 at 150 %, 144 Hz, Windows HDR on. Panel: "HDR · peak 390 nits · SDR white 204 nits" |
| SDR monitor | 1920×1080 at 100 %, 60 Hz. Panel: "SDR · peak 270 nits · SDR white 80 nits" |
| Graphics | the owner's discrete NVIDIA card (Supported formats: "Tested on" it) |
| Codec packages | as W1 (§ Setup there): HEVC Video Extensions (paid, 2.4.151), AV1 (2.0.35), VP9 (1.2.20), MPEG-2 (1.2.32), Web Media Extensions (2.1.51), Dolby Vision Extensions (2.20503.802), the PC maker's Dolby Digital Plus decoder (1.2.591) |
| App | `bindings/csharp/build.ps1`, then `apps/windows/build.ps1 -Mpv` with the shared D6 libmpv build; `Current.exe` from `apps/windows/out/app` |

**How it was watched.**
- **The info panel** (Ctrl+I) for the route and its reasons on every item.
- **The engine's trace lines** (`Trace.TraceInformation` and `TraceWarning` in `NativeEngine.cs`). The app has no trace listener, so
  they go to OutputDebugString; a small script outside the repository read them through the DBWIN protocol. Only lines starting
  "Current:" were kept.
- **The system media session**, read and driven through `GlobalSystemMediaTransportControlsSessionManager` (W1's probe), as the
  Windows media flyout does.
- **CodecQuery**, asked from PowerShell with the subtypes the core asks for.
- **The Windows event log** for the crash.

**Test media.** Synthetic files from the local FFmpeg 9.0.2 (a test pattern with its format and a running clock drawn on it, test tones,
W1's PGS writer, W1's styled ASS file), plus the public Jellyfin test videos W1 downloaded (4K HEVC HDR10 and Dolby Vision profiles 5
and 8.1, CC BY-SA 4.0), remuxed into MKV where named. One folder per file on the server, named "W7 nn ...". Nothing was committed;
the generator lives outside the repository (`%USERPROFILE%\Developer\Projects\current-spikes\windows-native-check`). No file from the
owner's library was used.

## Results
| # | Check | Result |
|---|---|---|
| 1 | HEVC MKV and H.264 MP4 natively | **Passed** |
| 2 | H.264 High 10 → app engine, "could not start" | **Passed, differently**: decided before starting |
| 3 | TrueHD, DTS → app engine | **Passed** |
| 4 | AC-3, E-AC-3: route and CodecQuery | **Passed** on this PC; a table rule is wrong for PCs without a Dolby decoder |
| 5 | Dolby Vision 5 and 8.1 → app engine | **Passed** |
| 6 | HDR10 SDR → app, HDR → native; HLG native on both | **Passed** (routes; picture right by eye) |
| 7 | Re-plan on a monitor move or an HDR switch | **Failed**: the app crashed once; the switch back after HDR on not verified |
| 8 | Server stream (HLS) natively; slow far seek falls back | **Failed** |
| 9 | "Always app engine", "Play with app engine" | **Failed**: not in the Windows UI |
| 10 | SRT inside, external SRT, external WebVTT | **Passed**, but one cue stuck (item 13's finding) |
| 11 | PGS on 1080p, 4K, cropped | **Failed**: colours swapped; top cue lost on cropped video |
| 12 | Subtitles lift above the bar and come down | **Passed** (position); smoothness not judged |
| 13 | Two tracks and Off without a reload | **Passed**, but stale or stuck cues seen twice |
| 14 | Same track on reopening; MF's Ids | **Passed**: Ids are numbers, stable across opens |
| 15 | Windows caption settings while playing | **Passed as designed**: applied from the next open |
| 16 | Styled ASS and DVD subtitles → app engine | **Passed** |
| 17 | Subtitle brightness on the HDR monitor | **Passed** (owner) |
| 18 | Full screen on both engines; media keys; SMTC | **Passed** |
| 19 | Pause, then audio or quality change: stays paused | **Passed**, but a paused server stream on the native engine hangs (item 8) |
| 20 | Supported formats: routes, notes, Store link | **Passed** with wording findings; the Store link not tested |
| — | Playback smoothness (owner) | **Failed**: native 4K HDR10 at 60 fps stutters |
| — | ALAC in MP4 (a W6 "not verified" row) | **Failed**: native and silent |

### 1. HEVC MKV and H.264 MP4
"MKV, HEVC Main 10, SDR 1080p, AAC" and "MP4, H.264 High, AAC": both **Native**, "Native playback is fully compatible", on
Windows' player surface. Also native and moving: MKV with 8K H.264 (7680×4320, level 6.1), MKV with MPEG-2 video and AC-3, MKV
with H.264 and MP3 (the last two are W6 "Not verified on a device" rows; now seen on this PC).

Panel finding: on every native route the panel says "Player · software decoder". The core prints that whenever the engine reports no
hardware decoder (`crates/app/src/player/info_panel.rs`), and the native engine never reports one. On the native route the line is
wrong, not just empty.

### 2. H.264 High 10
"MKV, H.264 High 10, AAC": **App engine**, "The native player can't decode this H.264 (AVC) video." The core's table allows H.264 at
most 8-bit natively (W3, from W1 §2), so the file never started on Windows' player. The checklist's "… could not start" reason did not
appear, and no file in this check made a native start fail after opening: 8K H.264, MPEG-2, MP3 and ALAC all opened natively (ALAC:
below). The fallback chain did run once, from the native engine's server stream to the app engine's (item 8).

### 3. TrueHD and DTS
"MKV, H.264, TrueHD 5.1" and "MKV, H.264, DTS 5.1": **App engine**, "The native player can't decode Dolby TrueHD audio." / "… DTS
audio." Sound on both (owner).

### 4. AC-3 and E-AC-3
"MKV, H.264, AC-3 5.1" and "… E-AC-3 5.1": **Native**, fully compatible. CodecQuery from PowerShell on this PC:

| Subtype | Decoders |
|---|---|
| `AudioFormatDolbyAC3` | 2: "Dolby Digital Plus Decoder MFT"; "Dolby AC-3 and E-AC-3 IEC-61937 converter MFT" |
| `AudioFormatDolbyDDPlus` | 2: the same two |
| `AudioFormatDts` | 0 |
| `AudioFormatPcm` | 0 |
| `AudioFormatFlac`, `AudioFormatOpus` | 1 each (Microsoft's) |

So W3's and W4's worry is real: Windows' own IEC 61937 converter (W1 §3, `mfaudiocnv.dll`) is returned for AC-3 and E-AC-3, while
for DTS it is not. `windows_engine` counts any answer above 0 (`crates/app/src/player/platform_caps.rs`), so on a PC without the
PC maker's Dolby decoder AC-3 and E-AC-3 would still be sent natively and fail there (not tested: the decoder was not removed). The
supported formats page would also say "Through the Dolby decoder this PC has". The answers carry names, so the UI could leave out the
converter; how to tell a converter from a decoder reliably is not verified.

### 5. Dolby Vision
"MKV, HEVC, Dolby Vision profile 5" and "… profile 8.1 (HDR10)", each as MKV and as MP4 (public samples): all **App engine**, "The app
engine shows Dolby Vision best on this device." Profile 5 showed the pink and purple of applied Dolby Vision, as in W1 §4.

### 6. HDR10 and HLG
| File | HDR monitor | SDR monitor |
|---|---|---|
| "MKV, HEVC Main 10, HDR10, 4K" | **Native** | **App engine**, "The app engine tone-maps HDR video for this SDR display." |
| "MKV, HEVC Main 10, HLG, 1080p" | **Native** | **Native** |

By eye (owner): HDR10 right. HLG looks dim on the HDR monitor; the owner did not know whether it should. W1 judged the same synthetic
file right there. HLG puts its reference white near SDR white, so it can look dimmer than HDR10's highlights; not a failure, recorded as
seen.

### 7. Display changes during playback
- **Monitor moves** (Win+Shift+Arrow, HDR10 playing): HDR → SDR re-planned to the **App engine** at the position, and SDR → HDR
  back to **Native**. Passed, twice.
- **Windows HDR switched off** (Win+Alt+B, HDR10 playing natively): re-planned to the **App engine**; the panel read "Display · SDR ·
  peak 409 nits · SDR white 80 nits". After the owner switched HDR back on, the panel still read SDR. Whether that read was taken before
  or after the second switch is not known, so **the move back to Native after HDR on is not verified**. A new HDR10 file afterwards
  played natively with "Display · HDR".
- **Crash (failed).** The owner moved a window playing the HLG file to the SDR monitor and back, and the app closed. Windows' event log,
  2026-10-31 08:11:00: `Current.exe` 0.1.0.0, faulting module `Microsoft.Graphics.Display.dll` 10.0.27200.1054 (the Windows App SDK's,
  shipped next to `Current.exe`), exception `0xc0000005`, offset `0x1c151`. `DisplayWatcher` disposes its `DisplayInformation` and
  creates a new one each time the window's monitor changes (`AppWindow.Changed`), which a drag across the monitor boundary can do several
  times quickly. That this is the cause is **not verified**; Claude's keyboard moves (one monitor change each) did not crash. No dump
  was kept.

### 8. Server streams (HLS) on the native engine (failed)
A lower quality in the player's menu makes Jellyfin transcode to HLS with fMP4 segments. In this check it chose HEVC ("Server · video to
hevc · audio copied · mp4").
- **The native engine never started a server transcode.** Three tries out of three (4K HDR10 at 1080p 10 Mbit/s; 1080p H.264 at 720p
  4 Mbit/s, twice, once after a restart of the app): about 8 s after the change the engine reported `stream_timeout`
  (`HlsFirstSegmentDeadline`), and the core moved the stream to the app engine, which played it. The server was transcoding at about
  225–390 fps, so a slow first segment is not the explanation. W1 §7 played Jellyfin's transcode natively, but an H.264 one and from
  the start of the file. What differs here (HEVC output, a start in the middle of the file, the relay) was not narrowed down.
- **A server stream loaded on the native engine while paused hangs.** On a paused H.264 file, a quality change gave "Server
  transcode · HLS · mp4" on the native engine and stayed paused (item 19). Play then did nothing: the picture went black, the bar stayed
  at 0:00 with the play icon, a second press did nothing, and no `stream_timeout` came. From reading the code (not verified by a
  trace): the load arms the deadline, but the wait also ends when a buffered range reaches past the position (`NativeEngine.cs`, the
  check after `AwaitSegment`), which can happen while paused; nothing arms it again for the later play. After about a minute the player showed "Can't play this item. The
  server can't be reached. Check your connection and try again." The server was reachable all along, and the app engine was not tried.
- **The far seek on a slow stream** was not reached: no server stream played natively long enough to seek.
- **The panel's wording** after the fallback: "Server transcode · HLS · mp4", then "• Server transcode could not start." That reads as
  if the playing route had failed. It does not say that the native engine's attempt failed and the app engine plays it now.

### 9. "Always app engine" and "Play with app engine" (failed)
Settings → Playback has no "Player" setting, and the player's menus have no "Play with app engine". The core, `current-ffi` and the C#
adapters have both (W2, W3: `SetPlayerChoiceAsync`, `PlayWithAppEngineAsync`), but nothing in `apps/windows` calls them. W3 and W4
left the UI to later tasks, and no task added it.

### 10. Text subtitles
- **SRT inside an MKV** (English): **Native**, italic and bold runs drawn, two lines. Passed.
- **External SRT (en) and external WebVTT (de)** next to an MKV: both listed ("SUBRIP - External", "WEBVTT - External"), the German
  WebVTT chosen first (the server's default), both drawn natively with italic and bold. The trace showed "added a subtitle file while
  playing" each time; the reopen path did not run. Passed, except one stuck cue: WebVTT cue 4 (14–16 s) stayed on screen from 14 s to
  past 40 s, shown above each later cue. After a seek to the start, cues came and went normally (a lag of a few tenths of a second, not
  measured). Not reproduced.
- **WebVTT inside an MKV:** generated, not played. The supported formats page says the app engine plays it.
- The panel lists an external track as "German - WEBVTT - External · external" (the word twice).

### 11. PGS (failed)
| File | Placement | Colour |
|---|---|---|
| "MKV, H.264 1080p, PGS (1920×1080 canvas)" | right: bottom centre; the "top of the frame" cue at the top | **wrong** |
| "MKV, H.264 4K, PGS (1920×1080 canvas)" | right, same scale as 1080p | **wrong** |
| "MKV, H.264 1920×804 (2.39:1), PGS (1920×1080 canvas)" | bottom cue right; **the cue at the top of the canvas never shows** | (white cues only) |
| "MKV, HEVC 4K HDR10, PGS and SRT" | right; native | — |

- **Colours:** the cue drawn yellow (255, 215, 0) shows cyan on the native route; FFmpeg's render of the same file and libmpv (the
  same cue converted to DVD subtitles) show it yellow. Red and blue are swapped somewhere between the `ImageCue`'s `SoftwareBitmap`
  (W1: BGRA8, premultiplied) and our `Image`. White cues hide it, which is why W1 and W5 did not see it.
- **Cropped video:** W5 fits the canvas to the picture's width with its bottom on the picture's bottom. On a 1920×804 picture a 1080-high
  canvas then starts about 100 px above the window's top, so a cue placed high in the canvas is outside the window. Centring the canvas
  on the picture would keep it, in the letterbox above the picture. What libmpv does with such a file is not verified.

### 12. Above the control bar
With the bar shown, the cue's box sat just above it; while the bar faded out, the cue was lower, over the fading bar. Passed for the
position. Whether the movement looks smooth was not judged (screenshots only). libmpv's cues stay over the bar (styled ASS seen over the
fading bar), as the open roadmap line says.

### 13. Switching tracks without a reload
"MKV, H.264, two SRT tracks (eng, ger)": English → German → Off while playing. The clock ran on (14.2 s → 21.3 s), no "opens the
file again" trace line, the right text each time, nothing shown with Off. Passed. But once, right after Off → German at about 7 s, the
view showed German cue 5 (17–20 s): the cue that was on screen when that track was turned off in the previous play. With item 10's
stuck cue, cues outlive a track or presentation change.

### 14. The same track on reopening; MF's Ids
Trace lines on each open (`LogSubtitleTracks`):
- "MKV, H.264, SRT inside": `track 0: id '2', kind Subtitle, language 'en'`.
- "MKV, H.264, two SRT tracks": `track 0: id '3' … 'en'`, `track 1: id '4' … 'de'`, the same on three opens.
- PGS files: `id '2'` (PGS only) and `id '3'` PGS, `id '4'` SRT (PGS and SRT), kind `ImageSubtitle` for PGS.

**MF's Ids are decimal numbers.** They kept the files' order here, but they are not simply the stream index: the single SRT of a
three-stream file was '2' while the two SRTs of a four-stream file were '3' and '4' (FFmpeg numbers Matroska tracks from 1, not
checked in these files). W5's rule (sort by Id numerically when all are numbers) picked the right track every time: German chosen,
the file reopened, German text shown from the first cue. Passed.

### 15. Windows caption settings
The owner changed size and colour in Settings → Accessibility → Captions while an SRT played: no change until the file was opened
again, then the new style. As W5 designed (`PropertiesChanged` not used). Passed as designed.

### 16. Styled ASS and DVD subtitles
Choosing the track re-planned at the position to the **App engine**, "The subtitles need rendering the native player can't do.", for
"MKV, H.264, styled ASS" and "MKV, H.264, DVD subtitles". Passed.

### 17. Subtitle brightness with HDR on
"MKV, HEVC 4K HDR10, PGS and SRT" on the HDR monitor: **Native**; the subtitles looked right next to the picture (owner).

### 18. Full screen, media keys, the system media session
- Full screen on the native route (4K HDR10) and on the app engine (TrueHD file): the picture filled the screen, controls on top.
- The keyboard's play/pause key worked (owner).
- The system media session on both engines: app `Skeksik.Current`, type Video, artwork, play/pause and seek enabled. Play/pause from
  the session toggled the player both ways; a seek to 0:30 landed. The session's end time was 5:25 for a file the player shows as 5:59
  (the long HDR10 file), and 1:00 (right) for a one-minute file; not looked into.

### 19. Pause, then change audio or quality
"MKV, H.264, AAC 2.0 / AC-3 5.1 / E-AC-3 5.1 / TrueHD 5.1", paused at 4.25 s natively:
- TrueHD chosen: moved to the **App engine**, still paused (the clock held at 3.75 s; the move went back 0.5 s, probably to a
  keyframe).
- AAC chosen again: stayed on the app engine (W2's rule: a track the current engine plays does not move the file). Paused. The panel
  kept "The native player can't decode Dolby TrueHD audio.", which no longer describes the playback.
- A lower quality: "Server transcode" on the native engine, still paused. Passed for "stays paused"; resuming then hung (item 8).

### 20. Supported formats
The page names a route per row, consistently with what played: H.264, HEVC, AV1, VP9 → Windows' player; VC-1 → app engine; AC-3,
E-AC-3, Atmos → Windows' player "Through the Dolby decoder this PC has"; TrueHD, DTS, Vorbis, PCM → app engine; SRT and PGS → Windows'
player; ASS, VobSub, DVB, MP4 text → app engine; WebVTT → app engine "As a separate file it plays in Windows' player"; HDR10, HLG →
"Windows' player, HDR"; Dolby Vision → "App engine, HDR". Findings:
- **MOV** says "App engine, Windows' player can't play it", but MP4 files reach the core as `mov` (the panel showed "Source · mov" for
  the H.264 MP4) and play natively.
- **Dolby Vision profiles 5 and 8** still say "Not checked on a device yet"; W1 and this check have checked both.
- **MPEG-2 and MP3** say "Not verified on a device": now seen playing natively here. **ALAC** says the same and plays silently (below).
- **MPEG-4 Part 2** says "Server converts" although W1 §3 saw Windows' built-in decoder for it; the core's table has no row for it.
- **The Store link** did not appear: every extension is installed, so there was no row to link. Not tested.

### Playback smoothness (failed)
The owner saw the native route stutter ("sometimes it's only image, and sometimes it's also sound") with the 4K HDR10 file (HEVC Main
10, 3840×2160, 60 fps, about 40 Mbit/s) with Windows HDR on. A 9-second screen recording of it playing natively has about 24 distinct
frames for a 60 fps source, with still stretches up to about 1 s (FFmpeg `mpdecimate` and `freezedetect`; the recorder itself may drop
frames with HDR on). The 1080p HEVC SDR file did not stutter. The owner also saw stutter in the first seconds after a start: about 3 s on
the 1080p SRT file, about 8 s on the 4K HDR10 file with PGS. With HDR off the HDR10 file plays on libmpv (departure 8), so "only with
HDR on" may compare Windows' player with libmpv rather than HDR on with off. Not narrowed down: decoder, HDR composition, or the relay at
40 Mbit/s. W1's notes record no stutter but did not look for it.

### ALAC in MP4 (failed)
"MP4, H.264, ALAC": **Native**, "fully compatible", the picture moving, **no sound** (owner). Trace: "the native engine has no track
for the audio stream asked for". `ApplyAudio` logs that and returns; nothing reports a `TrackFailed`, so the file stays on a silent
native route. ALAC is listed by CodecQuery (W1 §3); whether Windows' player exposes no audio track for it in MP4 or the engine's
mapping misses it is not known.

## Proposals for the owner (they touch ADR 0025)
1. **Server streams on the native engine.** Departure 4 was dropped because W1 played Jellyfin's fMP4 HLS natively. In this check no
   server transcode started natively (item 8), and a paused one hung. Proposed: until a fix shows native server streams starting
   reliably, server streams go to the app engine first (departure 4 back, for now), or the transcoding profile offered to the native
   engine is limited to what W1 played (H.264). The ADR is unchanged here; the owner decides. **Decided (owner, 2026-10-31): "Fix,
   then investigate".** ADR 0025 stays as it is: the fix task first ends the paused hang (the deadline must cover a play after a
   paused load, and the failure must fall back to the app engine), then finds why Windows' player does not start Jellyfin's
   transcode. The choice between keeping Windows' player first, libmpv first or an H.264-only transcode for the native engine is
   made with that cause known.
2. **A per-playback player choice** (the owner, 2026-10-31, on how switching feels): "Setting to lock the supported player for
   current playback (native, remux, app engine or transcoding) it should show only supported players if that's possible and should not
   automatically switch if it's chosen." This widens choice 3 (today "Play with app engine" for this playback only) to a choice among
   the routes the core finds possible for the file, and turns off automatic re-plans (display, speed, track) for that playback.
   It also answers the roadmap's "Settings to turn off switching back to Windows' player", which waited for this check. The owner's
   answer (2026-10-31): "it should be a dropdown with auto and player alternatives", and then: "Replace Always play with app engine
   setting with a setting that allow you to choose how to play (auto, native, remux, app engine, transcode) if chosen way is
   unsupported by playback, it should fallback to the supported that is below chosen, user should be able to set this setting
   globally in settings and for the current playback in player". **Decided.** As read here:
   - One choice, "how to play": Auto, Native, Remux, App engine, Transcode. It is set globally in Settings → Playback (the default
     for every playback, replacing "Player": Automatic / Always app engine) and for the current playback in the player (replacing
     "Play with app engine").
   - Auto is ADR 0025's order with its automatic re-plans. Any other value starts at that route; if the file cannot play there, it
     falls to the next supported route below it in the order native → remux → app engine → server, never above.
   - With a value other than Auto, no automatic switch back up the order (the owner's earlier words: "should not automatically switch
     if it's chosen"). Whether a forced move down still happens when the chosen route stops working mid-playback (a display change,
     a speed, a track) is for the task to state and the owner to confirm.
   - Open for the task: whether the player's list shows only the routes possible for the file ("show only supported players if
     that's possible", the earlier answer) and how Remux shows on Windows, where there is none (departure 1); and whether "Transcode"
     means the server's stream as the server decides (remux or transcode, ADR 0025 step 4) or always a transcode.
   ADR 0025 choice 3 is amended by the task that builds it, quoting the owner.

## Not verified
- The move back to Windows' player after Windows HDR is switched on again (item 7).
- A native start that fails after opening, with "… could not start" (item 2): no file made one.
- A far seek on a slow server stream (item 8).
- The Store link (item 20): no extension was missing.
- Smoothness of the subtitles' lift (item 12).
- AC-3 on a PC without the PC maker's Dolby decoder (item 4).
- Physical multichannel speakers, E-AC-3 Atmos objects, a speed change (the app has no speed control), true Dolby Vision output: as
  W1.
- WebVTT inside an MKV: generated, not played.

## Sources
- Windows' event log (Application, 2026-10-31): events 1000 (Application Error) and 1001 (Windows Error Reporting) for `Current.exe`,
  quoted above.
- CodecQuery answers: `Windows.Media.Core.CodecQuery.FindAllAsync(Audio, Decoder, subtype)` with `CodecSubtypes` properties, on this PC.
- The code read: `apps/windows/Current.Shell/Player/NativeEngine.cs` (the deadline, `Fail`, `ApplyAudio`, `LogSubtitleTracks`),
  `DisplayWatcher.cs`, `CodecAnswers.cs`; `crates/app/src/player/platform_caps.rs` (`windows_engine`) and `info_panel.rs`, at 7a50ec5.
