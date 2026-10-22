# 0007: Headless engines, shared controller, custom player UI

Status: accepted (feature target and staging updated 2026-10-18; amended 2026-10-21)

## Decision
Three layers, whichever engine is used:
1. **Engine** (`PlayerEngine`, per platform, headless): load, play/pause/seek, tracks, state, errors, renders to a video surface. No built-in controls.
2. **Controller** (`PlayerController` in `feature:player`, shared Kotlin): owns the plan, fallback logic, progress reporting/resume, track selection,
   skip-intro/next-episode. Exposes `PlayerUiState`.
3. **Player UI**: the app's own custom controls, in SwiftUI (Apple) or Compose (others). Only the video surface is platform-specific.

**Route selection** (`PlaybackPlanner`, shared Kotlin): Direct Play, Direct Stream, Remux, or server Transcode (HLS), chosen from media info,
engine capabilities, quality limit and user permissions. Jellyfin's playback-info request accepts a device profile and flags for direct play/stream/transcoding
(PlaybackInfoDto: deviceProfile, maxStreamingBitrate, enableDirectPlay, enableDirectStream, enableTranscoding, allowVideoStreamCopy, allowAudioStreamCopy,
audioStreamIndex, subtitleStreamIndex), so the server does part of the negotiation. Confirmed against Jellyfin 12.2 on 2026-10-18
(without a DeviceProfile the server offers no transcode URL); details in docs/platform-notes.md.
Stash (optional, ADR 0008) is a second source: the raw file with byte ranges, or Stash's own transcodes (HLS, MP4, WEBM). The planner
decides from the file's container and codecs, not from Stash's stream labels, and the controller reports progress to whichever server the media came from.

**Native first, fallback second.** Try the platform's native engine; if the plan says it cannot play the media, or it errors at runtime, retry on the fallback
engine, then a server remux, then a server transcode. The choice is automatic per title; users never pick an engine.

## Feature target
The player aims for the functionality of a full-featured custom engine, on every platform as far as the platform allows:
- **Routes:** automatic per title (native engine, fallback engine, server remux, server HLS transcode) from the server's playback info and the
  engine's capabilities; a quality limit; remembered quality and zoom; buffering and HLS segment-length settings.
- **Video:** hardware decoding wherever offered (H.264 and HEVC; AV1 and VP9 when the hardware has them), software decoding otherwise;
  4K60 at high bitrates without dropped frames.
- **HDR:** HDR10 (PQ) and HLG in BT.2020 with real HDR output on capable displays, falling back as the system and display allow. HDR10+ plays
  as static HDR10 (dynamic metadata is not kept).
- **Dolby Vision:** full Dolby Vision only through the platform's native player (or a server remux that feeds it); the fallback engine renders
  a compatible base layer; profiles without one go to the native player or a server HLS stream.
- **Audio:** the fallback engine decodes everything to PCM, keeping multichannel where the platform and output allow (the system may downmix).
  E-AC-3 with Atmos plays natively through the platform player when container, device and output allow; TrueHD with Atmos is decoded to PCM
  and loses its object metadata.
- **Formats without transcoding:**
  - Containers: Matroska, WebM, MP4/M4V/M4A/MOV/3GP/3G2, AVI/DivX, MPEG-TS/M2TS, MPEG-PS (MPG, VOB), MPEG video (M2V), FLV, ASF/WMV,
    Ogg/OGM/OGV, RealMedia, MP3, FLAC, raw AAC, WAV; HLS as the streaming protocol.
  - Video: H.264, HEVC, AV1, VP9, VP8, MPEG-4 Part 2 (DivX/Xvid), MS-MPEG4 v1-3, MPEG-2, MPEG-1, H.263, FLV1, RealVideo 9/10, SVQ3, VC-1,
    WMV1-3, Theora, MJPEG (decoder only, not offered for direct play).
  - Audio: AAC (LC, LATM), AC-3, E-AC-3, TrueHD/MLP, DTS/DTS-HD, MP1/MP2, MP3, Opus, Vorbis, FLAC, ALAC, WMA (Pro, Lossless), PCM
    (integer and float), WavPack, APE, TTA, TAK, Cook/RealAudio.
  - Subtitles: SRT/SubRip (SUB converted to WebVTT by the server), ASS/SSA rendered with their own styles (font, size, outline, shadow,
    position), WebVTT, MP4 text, and image-based PGS, DVD/DVB and XSUB (embedded; sidecar PGS files only offline).
- **Subtitles:** embedded and external; appearance settings for text subtitles; a delay offset; preferred audio and subtitle languages;
  online subtitle search when the server offers it.
- **Platform:** Picture in Picture; AirPlay on Apple (native player first, a compatible server stream otherwise); Now Playing and media keys;
  chapters in the seek bar; an audio delay setting for external speakers.
- **Episodes:** auto-skip intros from server segments; auto-play the next episode with a Next Up prompt, timed by the outro segment or a user setting.
- **Diagnostics:** a playback info overlay (engine, route, protocol, stream type, codecs, resolution, bitrate, frame rate, dynamic range,
  audio track, container) and a problem report whose logs are stripped of secrets.

Where a native engine falls short of this (checked per platform in its spike), that platform gets a fallback engine. On the web, what browsers
cannot play goes to the server instead.

**Staging.** M2: the desktop engine (from the M1 spike) and web (browser video + hls.js, server transcode as fallback). M4: Android (Media3, plus a
fallback where needed) and Apple (AVPlayer plus a fallback engine). Fallback engines are built on existing libraries (libmpv, FFmpeg and the like)
rather than from scratch. Candidate licences checked: mpv is GPLv2+ by default and LGPLv2.1+ only when built without GPL-only files (intended for libmpv);
vlcj is GPLv3+ (commercial licence available) and vlcj 5 requires LibVLC 4. Re-check licences against App Store / distribution needs before choosing.

## Platform notes
See docs/platform-notes.md: Picture in Picture, Now Playing and remote commands, subtitles and tvOS remote handling all become the app's responsibility with a custom UI.

## Amendment 2026-10-21
**Apple: the system player's functionality with our own UI.** On Apple platforms the player UI stays the app's own on every engine,
and it still provides everything the system player does. Each item is the app's job with a custom UI; the APIs are recorded in
docs/platform-notes.md ("Apple system integration with a custom player"):
- remote commands and Now Playing: Lock Screen, Control Center, the macOS menu bar, media keys;
- Picture in Picture on every engine, not only AVPlayer (the fallback engine renders into a sample-buffer layer);
- the AirPlay route picker;
- skip intro and credits;
- display criteria: matching frame rate and dynamic range, tvOS especially;
- keeping the display awake during playback.

**Now Playing content.** Title; a second line (series · S1E2, or the year); artwork; duration and position; ±N s skip commands; a
change-position (scrubbing) command; play/pause; a next-episode command when there is a next episode. An existing third-party Jellyfin
client on Apple platforms shows the title only, with no second line and no next command.

**Route transparency.** The planner's output is shown to the user:
- A playback information view lists the engine, the method (direct play, remux, direct stream, transcode), protocol, container, video
  (codec, profile, resolution, frame rate, dynamic range, bitrate), audio (codec, channels, language), subtitle delivery (drawn by the
  engine, native, burned in), the planner's reason for the choice, and each earlier failed attempt with its reason.
- Its fields never contradict each other during a fallback: they all describe the attempt that is playing.
- A format/HDR badge on the player explains the route in one sentence when tapped (e.g. why a Dolby Vision + HDR10+ file plays its
  HDR10 base layer on the fallback engine). The user can switch route from there.
- It pairs with Copy diagnostics (ADR 0015): a report attaches the route and the reasons.

**Fallback chain.** The planner records why each step fell back. A failure shows a card with "Try other engine", Retry and Close,
never a dead end.

**Remux to local HLS (to be evaluated in the M2 spikes).** Repackaging a compatible MKV into local HLS/fMP4 on the device would let the
native engine play it, keeping the native system integration, without a server transcode. A candidate route between direct play and
server transcode; not decided.

**Remembered choices.** Audio and subtitle track, quality and zoom are remembered, with a scope setting: Off, Season or Show. A "smart"
subtitle mode shows subtitles only when the audio language differs from the preferred one.

The player's UX rules for every platform (keyboard, pointer, touch, subtitles, Next Up, PiP): docs/player-ux.md.
