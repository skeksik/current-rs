# D3 spike: macOS video, notes

Decision: ADR 0008. Run on 2026-10-22. The prototype (a SwiftUI app plus a command-line probe) is in commit
`7308ead37a98914da6a62e4a97e4ca00c112c3bd` on `spike/macos-video` (`spikes/macos-video/`); it was deleted in the next commits.

## Setup
| Item | Value |
|---|---|
| Mac | Mac14,15 (MacBook Air 15-inch, Apple M2) |
| macOS | 26.6.2 (25G83) |
| Xcode / SDK / Swift | Xcode 27.1 (27A9275), macOS 27.0 SDK, Swift 6.4, Swift 6 language mode (strict concurrency), deployment target macOS 26 |
| Displays | Built-in Liquid Retina (not XDR), 2880x1864; the owner's external HDR display for case 1 (model not recorded); no SDR external display |
| Test media | Synthetic, made with a local FFmpeg 9.0.2 (Homebrew, GPL build, a local tool only) by `make-media.sh`; Apple's public HLS examples (below). Nothing private was played |
| Build | SwiftPM, wrapped into an ad-hoc signed `.app` (no team, no signing settings); not sandboxed |

Prototype: AVPlayer rendering through an `AVPlayerLayer` hosted in an `NSViewRepresentable`; our own controls over it (Liquid Glass
bar with play/pause, ±10 s, scrubber, times, audio and subtitle menus, AirPlay `AVRoutePickerView`, PiP button, full screen),
auto-hiding after 3 s of no mouse movement while playing (the cursor hides too); menu bar Playback, Audio and Subtitles menus;
`MPNowPlayingInfoCenter` and `MPRemoteCommandCenter`; an info panel with the screen's EDR values, the layer's dynamic range and a
log. `VideoProbe` opens each file with AVFoundation, lists tracks, decodes the first audio buffer to PCM with `AVAssetReader`, and
plays 6 s through `AVPlayer` into an `AVPlayerItemVideoOutput` to count decoded frames.

Apple examples used (page read 2026-10-22, https://developer.apple.com/streaming/examples/):
`https://devstreaming-cdn.apple.com/videos/streaming/examples/adv_dv_atmos/main.m3u8` (AVC, HEVC SDR, HEVC HDR10, Dolby Vision
profile 5 `dvh1.05.xx`, AAC, AC-3, E-AC-3 JOC, WebVTT, CEA-608) and `.../bipbop_adv_example_hevc/master.m3u8`. For case 1 the
main playlist was narrowed into one playlist per video range (DV only, HDR10 only, SDR only) served from 127.0.0.1.

## Results per case

### 1. HDR / EDR: works
- HDR10 (`hvc1`, Main10, PQ, BT.2020, mastering metadata) in MP4 and in our own HLS/fMP4, and Dolby Vision profile 5 and HDR10 from
  Apple's HLS example, play. `AVAssetTrack` reports `containsHDRVideo`, transfer `SMPTE_ST_2084_PQ`, and the decoder outputs 10-bit
  `x420` buffers. `AVPlayer.eligibleForHDRPlayback` is true, also on the built-in display.
- **The owner judged by eye on the external HDR display: Dolby Vision and HDR10 both show real HDR** (highlights clearly above UI
  white).
- Built-in display (not XDR): `NSScreen.maximumExtendedDynamicRangeColorComponentValue` and
  `maximumPotentialExtendedDynamicRangeColorComponentValue` both read 2.00 (reference 0.00) during SDR and HDR playback alike, so these
  values don't show whether HDR is engaged. The PQ ramp clipped to white from about two thirds of its width, which means tone mapping into the
  limited headroom.
- What AVPlayerLayer needed: nothing beyond the layer itself. The prototype set `CALayer.preferredDynamicRange = .automatic`
  (macOS 26+; the header says the default is `.standard`). `wantsExtendedDynamicRangeContent` is deprecated in the macOS 26 SDK in
  favour of `preferredDynamicRange`. **Not verified:** whether HDR still shows with the layer at `.standard`, or what `.high` and
  `.constrainedHigh` change on the HDR display (the info panel has a switch; not compared by eye). Behaviour on an SDR external display:
  not tested (none available).

### 2. Picture in Picture with our UI: works
- `AVPictureInPictureController(contentSource: .init(playerLayer:))` starts from our button; `isPictureInPicturePossible` and
  `isPictureInPictureActive` are KVO-observable. The owner confirmed that the PiP window plays, its buttons work, and its return
  button brings the video back. Our `restoreUserInterfaceForPictureInPictureStop` handler activates the app, brings the window
  front and calls the completion with `true`.
- While PiP is active, **AVPlayerLayer itself draws a system placeholder** ("This video is playing in picture in picture"). Our
  first version drew its own placeholder too, which doubled it. We hide only our control bar during PiP.
- `canStartPictureInPictureAutomaticallyFromInline` is unavailable on macOS (header).

### 3. Now Playing and remote commands: works
- With `nowPlayingInfo` (title, duration, elapsed time, rate, media type video, queue count) and `playbackState`, the spike appears in
  Control Center and the menu bar Now Playing item. The owner confirmed that play/pause, skip ±10 s (`skipForward/BackwardCommand` with
  `preferredIntervals = [10]`), dragging the position (`changePlaybackPositionCommand`), next and previous track, and the F7/F8/F9
  media keys all control the spike.
- Every remote command handler ran on the main thread (logged for each call).
- Display sleep: **`AVPlayer.preventsDisplaySleepDuringVideoPlayback` defaults to NO on macOS** (header: "Default is YES on iOS,
  tvOS and in Mac Catalyst apps. Default is NO on macOS."). With it set to YES, `pmset -g assertions` showed a
  `PreventUserIdleDisplaySleep` assertion held by the app while playing, and none after pausing.

### 4. AirPlay: not verified
- `AVRoutePickerView` (with `player` set) sits in our bar, opens the system's native route menu and lists the owner's AirPlay-capable
  monitor. With `allowsExternalPlayback = true`, playback on the monitor stopped within seconds for every item: Apple's public HLS
  playlists, local MP4 files, and HLS served from 127.0.0.1. Video from another app (YouTube) worked on the same monitor.
- Errors logged: `AVFoundationErrorDomain -11870` (underlying `-17226`) for local files; `-11868 "The content cannot be played on the
  device"` (underlying `-17223`) for HLS from 127.0.0.1. AirPlay video hands the stream URL to the receiver, which cannot reach the
  Mac's loopback address; that explains the 127.0.0.1 case but not the public streams or the files.
- The owner asked to leave AirPlay unverified for now and to revert the debugging changes made after the problem was found; they are
  reverted. The QuickTime Player comparison on the same monitor was not run. **To check later:** a QuickTime Player baseline, the Local
  Network privacy permission for an ad-hoc signed app, and an Apple TV as the receiver. For Jellyfin, AirPlay of an HLS URL sends the
  URL, including any `api_key`, to the receiver.

### 5. Formats: direct play versus remux or transcode
Probe results on this Mac. "Silent" means `AVPlayerItem` reaches `.readyToPlay` and time advances, but the track is not rendered:
**AVPlayer does not fail an item whose video or audio track it cannot decode.** The asset or track `isPlayable` / `isDecodable`
properties do report it.

| Media | Result |
|---|---|
| Containers: MP4, M4V, MOV, MPEG-TS (`.ts`), HLS (TS and fMP4) | Open and play |
| Containers: MKV (any codecs), WebM | `-11828 Cannot Open` ("This media format is not supported"); `video/x-matroska` and `video/webm` are not in `AVURLAsset.audiovisualMIMETypes()` |
| H.264 (`avc1`) | Plays |
| HEVC Main and Main10 as `hvc1` (SDR and HDR10) | Plays (hardware decode) |
| HEVC as `hev1` in MP4 | Silent video: track not playable; audio plays. `isPlayableExtendedMIMEType("hev1…")` is false |
| Dolby Vision profile 5 (`dvh1.05`, HLS) | Plays in HDR. `dvh1.08.06` (profile 8.1) reports playable by MIME type; no profile 8 media was tested |
| AV1 10-bit in MP4 | Silent video on M2: `VTIsHardwareDecodeSupported(AV1)` is false and there is no software path in AVPlayer |
| VP9 in MP4 | Silent video by default; **plays after `VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_VP9)`** (an app opt-in, macOS 11+) |
| AAC, ALAC, LPCM (MOV), FLAC in MP4, Opus mono or stereo in MP4 | Play |
| AC-3, E-AC-3 (5.1) in MP4; E-AC-3 JOC (Atmos) in Apple's HLS | Play (decoded to PCM on this Mac; passthrough to a receiver not tested) |
| Opus 5.1 in MP4 (channel mapping family 1) | Audio track not exposed at all; video plays silent |
| TrueHD (`mlpa`) in MP4 | Track listed, not playable or decodable; video plays silent |
| DTS in MP4 (FFmpeg writes it as `mp4a`) | Track not exposed; silent. DTS in MOV (`dtsc`): listed, not playable; silent |
| Several audio tracks with languages (MP4) | Appear in the audible `AVMediaSelectionGroup` ("English", "French Dolby Digital"); switching works |

MIME checks (`AVURLAsset.isPlayableExtendedMIMEType`) agree: `avc1`, `hvc1`, `dvh1.05`/`dvh1.08`, `ac-3`, `ec-3`, `fLaC`, `Opus`,
`alac`, `wvtt` are true; `hev1`, `av01`, `vp09`, `mlpa`, `dtsc`, `dtsx`, `stpp` are false. VideoToolbox hardware decode on M2: H.264,
HEVC, ProRes yes; AV1 and VP9 no (VP9 yes after the supplemental opt-in).

Jellyfin serving HLS for the same files: **not verified** (the owner skipped it). The prototype has a Jellyfin sheet that sends
`PlaybackInfo` with a device profile built from this table and plays the server's `TranscodingUrl`; it was never run against a server.

### 6. Subtitles: what AVPlayer renders, and the gap
- **tx3g (`mov_text`) in MP4** renders. AVFoundation adds a synthetic "Forced" option for each tx3g track to the legible group
  ("English", "English Forced", ...).
- **WebVTT in HLS** renders, with cue timing aligned to fMP4 video through `X-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000`. The system
  renderer honours cue settings (`line`, `position`, `align`, `size`, so top-left and middle placement, half-width boxes) and
  `<i>`, `<b>`, `<u>`. A `<c.yellow>` class span showed in the default colour (classes need `AVTextStyleRule` or CSS the player
  doesn't have). Ruby: not verified.
- WebVTT inside MP4 (`wvtt` sample entry): reports playable by MIME type, but FFmpeg 9.0.2 cannot mux it ("Could not find tag for codec
  webvtt"), so not tested.
- **The system renderer draws subtitles under our control bar:** AVPlayerLayer has no API to inset or move captions (none found in
  the macOS 27.0 SDK headers for `AVPlayerLayer`, `AVPlayerItem`, `AVPlayer`). The prototype switches to
  `AVPlayerItemLegibleOutput` with `suppressesPlayerRendering = true` and draws the cues in SwiftUI, above the bar while it is visible
  and near the bottom edge when it is hidden. This works for tx3g and HLS WebVTT. The cost: the delegate gets attributed strings with
  `kCMTextMarkupAttribute_*` styling and positions that we must map ourselves (the spike drew plain text, so italic and bold were lost).
- **What AVPlayer can't do at all:** SRT, ASS/SSA, PGS and VobSub arrive only in containers AVFoundation can't open (MKV). HLS only
  carries WebVTT or IMSC1 text. So a server must convert text subtitles to WebVTT or burn subtitles into the video. ASS converted to
  WebVTT by FFmpeg kept the text only, losing font, size, colour, `\pos` placement, rotation, karaoke and fades (shown side by side in the
  prototype). FFmpeg can't encode bitmap subtitles from text, so PGS and VobSub were not made. They are excluded by reasoning: no
  container AVFoundation opens carries them, and HLS has no bitmap subtitle type.

### 7. Native fit: works, with the fixes below
- Keyboard: Space plays and pauses, ←/→ skip ±10 s, ⌘←/⌘→ previous and next item, Esc leaves full screen (and only does that), double
  click toggles full screen. These keys go through an `NSEvent` local monitor that skips text fields and sheets. Typing spaces in the
  Open Location field worked and didn't toggle playback. The Playback menu shows the same shortcuts (Space, ◀, ▶). **Not verified:**
  whether ←/→ should skip 10 s or step frames as in the system's own players. The spike didn't compare them.
- Menu bar: File (Open…, Open Location…, Jellyfin…), Playback, Audio and Subtitles (inline pickers that mirror the bar's menus).
  View ▸ Enter Full Screen (⌃⌘F) comes from the system. Window full screen works from our button, the green button and the menu.
- Liquid Glass: `glassEffect(.regular)` over bright video made white symbols hard to read. `.regular.tint(.black.opacity(0.35))` with a
  dark colour scheme for the bar fixed legibility. The `.buttonStyle(.borderless)` symbols and the system `Slider` look native.
- Accessibility: the accessibility tree first showed the audio and subtitle menu buttons as "Volume High" and "Captions" (the SF
  Symbol names), because `.accessibilityLabel` on a `Menu` with an `Image` label had no effect. A `Label(…).labelStyle(.iconOnly)` fixed
  it. `AVRoutePickerView` exposes two elements: our "AirPlay" label and its own "show external playback menu" button. **The owner
  turned VoiceOver on and confirmed that every control is read by name**, including the slider ("Playback position", with the time as
  its value).

## Other findings
- Launching with bare file arguments (`open --args file.mp4`) makes AppKit treat them as documents to open, and the SwiftUI `Window`
  scene then shows no window. The prototype uses `-media <path>` instead.
- `AVPlayer`'s `timeControlStatus`, item `status` and PiP properties were observed with KVO closures marked `@Sendable` that hop to
  the main actor. That compiled cleanly under Swift 6 strict concurrency with no warnings.

## Sources (all read 2026-10-22)
- macOS 27.0 SDK headers (Xcode 27.1): `AVFoundation/AVPlayer.h` (`preventsDisplaySleepDuringVideoPlayback`, `eligibleForHDRPlayback`),
  `AVFoundation/AVPlayerItem.h` (`appliesPerFrameHDRDisplayMetadata`), `AVFoundation/AVPlayerItemOutput.h`
  (`AVPlayerItemLegibleOutput`, `suppressesPlayerRendering`), `QuartzCore/CALayer.h` (`preferredDynamicRange`, `toneMapMode`,
  deprecation of `wantsExtendedDynamicRangeContent`), `AVKit/AVPictureInPictureController.h`, `AVKit/AVRoutePickerView.h`,
  `MediaPlayer/MPNowPlayingInfoCenter.h`, `VideoToolbox/VTUtilities.h` (`VTRegisterSupplementalVideoDecoderIfAvailable`),
  `MediaToolbox/MTProfessionalVideoWorkflow.h` and `MediaExtension/MEFormatReader.h` (MediaExtension, macOS 14+).
- HLS Authoring Specification for Apple Devices,
  https://developer.apple.com/documentation/http-live-streaming/hls-authoring-specification-for-apple-devices: 1.5 (HEVC MUST be
  fMP4), 1.7 (HDR HEVC MUST be HDR10, HLG or Dolby Vision), 1.9 (Dolby Vision profile 5), 1.10 (prefer `hvc1`/`dvh1` over
  `hev1`/`dvhe`), 2.5 (multichannel codecs: AAC, ALAC, FLAC, AC-3, E-AC-3, E-AC-3 JOC), 2.25 (FLAC and ALAC MUST be fMP4), 4.2 and 5.2-5.4
  (subtitles: WebVTT with `X-TIMESTAMP-MAP`, or IMSC1 text in fMP4).
- HTTP Live Streaming examples, https://developer.apple.com/streaming/examples/.
