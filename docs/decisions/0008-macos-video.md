# 0008: macOS video: AVPlayer first, the server for the rest, a subtitle renderer before a fallback engine

Status: proposed (2026-10-22), from the D3 spike; accepted when its PR merges. Evidence: docs/notes/macos-video.md.

## Context
ADR 0001 makes the player our own on every platform, styled and behaving natively. On macOS the candidate engine is AVPlayer, with a
fallback engine (D7) if AVPlayer leaves too large a gap. D3 tested AVPlayer under our own SwiftUI controls on macOS 26.6 (M2) with
synthetic files and Apple's public HLS examples.

## Decision

### AVPlayer is the macOS engine
- AVPlayer rendering through an `AVPlayerLayer` in an `NSViewRepresentable`, with our SwiftUI controls on Liquid Glass over it.
  The spike confirmed HDR10 and Dolby Vision profile 5 in HDR on an HDR display (owner's eye), PiP from our own button with return to
  our window, Now Playing with every remote command and the media keys, window full screen, native keys and menus, and VoiceOver reading
  every control by name.
- The engine sets `preventsDisplaySleepDuringVideoPlayback = true` (it defaults to NO on macOS) and `allowsExternalPlayback = true`,
  and hides our controls while PiP is active (the layer draws the system placeholder itself).
- **Subtitles are drawn by us**, not by AVPlayerLayer: `AVPlayerItemLegibleOutput` with `suppressesPlayerRendering`, rendered in
  SwiftUI so cues move above the control bar (AVPlayerLayer has no API to move its captions, which our bar covers). Mapping
  `kCMTextMarkupAttribute_*` styling and positions is part of the macOS engine work in M3. The same renderer is the place to add the
  formats AVPlayer lacks (below).
- **The engine reports capabilities from track decodability, never from item status.** AVPlayer reaches `.readyToPlay` and plays
  silently when it cannot decode a track (HEVC tagged `hev1`, AV1 without hardware, TrueHD, DTS, Opus 5.1). The engine checks
  `isPlayable` / `isDecodable` on the chosen tracks after loading and reports a failure to the controller, which falls back to the
  next route.

### Routes the planner prefers on macOS
1. **Direct play** when the container is MP4, M4V, MOV or MPEG-TS and every chosen track is in this set. Video: H.264; HEVC as
   `hvc1` (SDR, HDR10, HLG not tested); Dolby Vision 5 (8.1 is accepted by the MIME check, but no media was tested). AV1 only where `VTIsHardwareDecodeSupported` says so. VP9 only with
   the app's opt-in `VTRegisterSupplementalVideoDecoderIfAvailable`. Audio: AAC, AC-3, E-AC-3 (with JOC), ALAC, FLAC, LPCM, Opus with
   one or two channels. Subtitles: none, tx3g, or text subtitles the server delivers as WebVTT alongside.
2. **Server remux to HLS with fMP4 segments** (Jellyfin direct stream) for everything whose video is in the set above but whose
   container is not: above all MKV with H.264 or HEVC. HEVC must be tagged `hvc1`, HDR metadata is kept, text subtitles become WebVTT
   renditions. Audio outside the set (TrueHD, DTS, Opus 5.1) is transcoded on its own, to E-AC-3, or to FLAC in fMP4 to stay
   lossless. That is cheap, and the video is still copied.
3. **Transcode** only when the video codec is outside the set (AV1 on Macs without AV1 hardware, VP9 without the opt-in, legacy codecs),
   for quality or bitrate limits, or for burned-in subtitles while D7 hasn't removed that need.

The device profile sent to Jellyfin is built from these lists and from what the engine detects at run time (hardware AV1, the VP9
opt-in, HDR eligibility).

### What decides D7 (the gap list)
AVPlayer plus a remuxing server covers containers, audio and HDR. What is left:
1. **Styled ASS/SSA subtitles:** HLS carries WebVTT only, and ASS converted to WebVTT loses font, colour, size, position, rotation,
   karaoke and fades. Today's only alternative is burn-in, a full video transcode that also loses HDR or needs tone mapping.
2. **Bitmap subtitles (PGS, VobSub):** no container AVFoundation opens carries them and HLS has no bitmap type, so they need burn-in
   (the same full transcode).
3. **AV1 on Macs without AV1 hardware** (M2 and earlier, as tested; M3 not tested): AVPlayer has no software path, so a server
   transcode is needed.
4. **Local files in MKV/WebM without a server** (roadmap "Later": local files, offline): there is no demuxer.
5. Smaller: `hev1` HEVC and multichannel Opus or DTS need a remux or an audio transcode, which the server already does. Dolby Vision
   profile 7 and 8 in MKV: not tested.

**D7 starts with items 1 and 2 without replacing AVPlayer:** our own subtitle renderer, driven by AVPlayer's clock, drawing ASS
through libass and PGS/VobSub bitmaps over the video. It is fed by subtitle streams the server extracts without touching the video.
This removes the full-transcode cases and keeps HDR. Licences and the server's ability to extract bitmap subtitle streams are checked
in D7 (not verified here). D7's decision: ADR 0010. A full fallback engine (FFmpeg demux and decode, with VideoToolbox, into an
`AVSampleBufferDisplayLayer`, or libmpv) is needed only for items 3 and 4. It can wait until local or offline playback is planned,
since the server covers item 3 for streaming. MediaExtension format readers (macOS 14+) could add MKV to AVFoundation, but Apple
writes that they are "not recommended for network-facing applications" (`MTProfessionalVideoWorkflow.h`), and they exist on macOS only.

## Consequences
- M3 builds the macOS player on AVPlayer, with our own subtitle view from day one and a capability check on the decoded tracks.
- The planner keeps the per-platform codec lists above. Jellyfin's HLS output must use fMP4 with `hvc1` for HEVC, which the HLS
  authoring spec requires.
- D7 is narrowed to a subtitle renderer first; a second engine on macOS is deferred.
- **AirPlay works for HLS that the receiver can reach** (re-checked 2026-10-22 on a third-party AirPlay 2 monitor; notes, case 4).
  The spike's failure was our own UI: auto-hide rebuilt the `AVRoutePickerView`, and a new picker for the same player ended the
  active route. What M3 must do:
  - Create one `AVRoutePickerView` per player and keep it for the player's lifetime. Hide the controls with opacity, never by
    removing the view that holds the picker.
  - Set no Info.plist key for AirPlay. Per TN3179, AirPlay needs no local network access, and `NSBonjourServices` is only for
    Bonjour the app does itself. The app still needs `NSLocalNetworkUsageDescription` to reach servers on the LAN, and an
    Apple-issued signing identity so local network privacy tracks it reliably.
  - Keep `allowsExternalPlayback` at true (the default). `usesExternalPlaybackWhileExternalScreenIsActive` doesn't exist on macOS.
  - Draw our own "playing on" placeholder while `isExternalPlaybackActive`, because AVPlayerLayer shows only black. Our controls and
    Now Playing drive the receiver, and stopping AirPlay continues locally.
  - Offer AirPlay video only for URLs the receiver can fetch. Local files and loopback URLs fail on this receiver, in QuickTime Player
    as well. So local or offline playback and anything we proxy on 127.0.0.1 can't be AirPlayed as video there.
  - AirPlay of an HLS URL is understood to hand the URL, including any token, to the receiver (inferred, not confirmed). That
    affects how Jellyfin URLs are formed.
  Not verified: an Apple TV as receiver, HLS from a LAN server, AirPlay without an `AVPlayerLayer`, and HDR on the receiver.
- Also not verified: SDR external displays, the layer's `preferredDynamicRange` values compared by eye, the Jellyfin HLS output for
  real files, M3's AV1 hardware, and Dolby Vision profiles other than 5.
