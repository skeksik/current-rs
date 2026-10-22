# Playback, HDR, engines and licences (from platform notes)

> Reference only, copied unchanged from the earlier Kotlin Multiplatform project (`skeksik/Current` at `66452b2`, 2026-10-22).
> Facts carry the dates they were checked on. ADR numbers, module names and "the app" refer to that project. Nothing here is a
> decision for this repo; see `README.md` in this folder.

## Apple playback (custom UI)
- PiP in a custom player: `AVPictureInPictureController` with an `AVPlayerLayer` or `AVSampleBufferDisplayLayer`; app supplies its PiP button, restores its UI in the delegate,
  must keep a strong reference, may only start PiP on user interaction (App Store rejects otherwise); needs audio session / background modes; on tvOS Now Playing must go through `MPNowPlayingSession`.
  Source: https://developer.apple.com/documentation/avkit/adopting-picture-in-picture-in-a-custom-player
- tvOS custom player: you must implement all remote commands yourself via `MPRemoteCommandCenter` plus gesture recognizers, and update Now Playing info;
  `AVPlayerViewController` handles these automatically and can be extended (custom transport items, info tabs, contextual actions such as Skip).
  Sources: https://developer.apple.com/documentation/avfoundation/supporting-remote-interactions-in-tvos , https://developer.apple.com/documentation/avkit/customizing-the-tvos-playback-experience
  Open decision for tvOS: fully custom UI vs `AVPlayerViewController` with custom items.
- Now Playing: `MPNowPlayingInfoCenter` exists on iOS, macOS (also has `playbackState`) and tvOS. Source: https://developer.apple.com/documentation/mediaplayer/mpnowplayinginfocenter
- Subtitles: a developer forum thread reports native subtitles work with `AVPlayerLayer` but are lost with `AVSampleBufferDisplayLayer`; `AVPlayerItemLegibleOutput` gives attributed text that must be laid out manually.
  This is a user report, not Apple guidance. Source: https://developer.apple.com/forums/thread/700128

## Apple system integration with a custom player (read 2026-10-21 in Apple's documentation, read as JSON from developer.apple.com; nothing run)
Input for ADR 0007's amendment 2026-10-21: what our own player UI must do itself on every engine.
- **Now Playing.** `MPNowPlayingInfoCenter` (iOS, tvOS 5+, macOS 10.12.2+): the system shows its `nowPlayingInfo` on the Lock Screen
  and in Control Center; the app does not control the layout. It takes `MPMediaItemPropertyTitle`, `...Artist`, `...AlbumTitle`,
  `...Artwork`, `...PlaybackDuration` and others, plus `MPNowPlayingInfoPropertyElapsedPlaybackTime` and `...PlaybackRate`, from which
  the system computes elapsed time itself (no frequent updates needed). On macOS `playbackState` must be set every time playback starts
  or stops, "otherwise remote control functionality may not work as expected".
  - tvOS: once an `MPNowPlayingSession` (tvOS 14+, iOS 16+, not macOS) is used, the system ignores the default info center, so the whole
    app moves to sessions; PiP needs the player tied to a session. `AVPlayerViewController` owns its own session. Both come from the
    `MPNowPlayingSession` page and "Adopting Picture in Picture in a custom player"; that the session takes a non-AVPlayer engine is not
    stated (the page speaks of `AVPlayer` instances).
- **Remote commands.** `MPRemoteCommandCenter.shared()` (iOS, tvOS 7.1+, macOS 10.12.2+) responds to "remote control events sent by
  external accessories and system controls": `skipForwardCommand`/`skipBackwardCommand` (`preferredIntervals` in seconds),
  `changePlaybackPositionCommand` (iOS 9.1+, tvOS 9+, macOS 10.12.2+), `nextTrackCommand`, play/pause. Each can be disabled.
- **Picture in Picture.** `AVPictureInPictureController` (iOS 9+, tvOS 14+, macOS 10.15+); `init(contentSource:)` with
  `ContentSource(sampleBufferDisplayLayer:playbackDelegate:)` and `AVPictureInPictureSampleBufferPlaybackDelegate` (iOS, tvOS 15+,
  macOS 12+) is the path for a player that is not `AVPlayer`. Check `isPictureInPictureSupported()` first (otherwise `init` returns
  nil); observe `isPictureInPicturePossible`; keep a strong reference; restore the UI in
  `restoreUserInterfaceForPictureInPictureStopWithCompletionHandler`. iOS and tvOS need the audio session and background modes. "Only
  begin PiP playback in response to user interaction and never programmatically" (App Store review rejects otherwise); the automatic
  start on leaving the app is `canStartPictureInPictureAutomaticallyFromInline` (iOS 14.2+, not tvOS or macOS), "only for content that
  you intend to be the user's primary focus".
- **AirPlay picker.** `AVRoutePickerView` (iOS, tvOS 11+, macOS 10.15+): a button that opens the system's list of nearby receivers,
  with video-capable devices higher when the app prefers video.
- **Display criteria.** `AVDisplayManager.preferredDisplayCriteria` exists on tvOS 11.2+ and visionOS only, and applies only when user
  settings allow; details under "HDR and Dolby Vision: Apple, AVPlayer". There is no such API on iOS or macOS.
- **Display awake.** `AVPlayer.preventsDisplaySleepDuringVideoPlayback` defaults to true on iOS and tvOS and to false on macOS. Another
  engine: `UIApplication.isIdleTimerDisabled` on iOS and tvOS (reset it when playback stops; Apple names "programs where the app needs to
  continue displaying content when user interaction is minimal"), and on macOS an IOKit assertion
  `IOPMAssertionCreateWithName` with `kIOPMAssertionTypePreventUserIdleDisplaySleep`.
- Sources: https://developer.apple.com/documentation/mediaplayer/mpnowplayinginfocenter ,
  https://developer.apple.com/documentation/mediaplayer/mpnowplayinginfocenter/playbackstate ,
  https://developer.apple.com/documentation/mediaplayer/mpnowplayinginfopropertyelapsedplaybacktime ,
  https://developer.apple.com/documentation/mediaplayer/mpnowplayingsession ,
  https://developer.apple.com/documentation/mediaplayer/mpremotecommandcenter ,
  https://developer.apple.com/documentation/mediaplayer/mpremotecommandcenter/changeplaybackpositioncommand ,
  https://developer.apple.com/documentation/mediaplayer/mpskipintervalcommand/preferredintervals ,
  https://developer.apple.com/documentation/avkit/adopting-picture-in-picture-in-a-custom-player ,
  https://developer.apple.com/documentation/avkit/avpictureinpicturecontroller/contentsource-swift.class ,
  https://developer.apple.com/documentation/avkit/avpictureinpicturecontroller/canstartpictureinpictureautomaticallyfrominline ,
  https://developer.apple.com/documentation/avkit/avroutepickerview ,
  https://developer.apple.com/documentation/avkit/avdisplaymanager/preferreddisplaycriteria ,
  https://developer.apple.com/documentation/avfoundation/avplayer/preventsdisplaysleepduringvideoplayback ,
  https://developer.apple.com/documentation/uikit/uiapplication/isidletimerdisabled ,
  https://developer.apple.com/documentation/iokit/kiopmassertiontypepreventuseridledisplaysleep

## Desktop and web playback (checked 2026-10-18; input for the M1 video spikes)
**Compose over native video.** Compose Multiplatform 1.12 has no supported way to draw Compose over native video:
- Desktop: "The Swing component within `SwingPanel` will always be layered above the Compose Multiplatform component". The experimental interop
  blending flag (`-Dcompose.interop.blending=true`) "cannot overlay another DirectX component" on Windows, which rules out mpv's and VLC's
  Direct3D output. Sources: https://kotlinlang.org/docs/multiplatform/compose-desktop-swing-interoperability.html ,
  https://github.com/JetBrains/compose-multiplatform-core/pull/915
- Web: `HtmlElementView` (`WebElementView` in 1.9 and 1.10) places the HTML element over the canvas, where it takes all input
  (KDoc in `compose/ui/ui/src/webMain/.../InteropView.web.kt`). Compose for web is Beta.
  Sources: https://github.com/JetBrains/compose-multiplatform/blob/master/CHANGELOG.md , https://kotlinlang.org/docs/multiplatform/supported-platforms.html

**Desktop engine candidates** (Windows first), in the order to try them:
1. libmpv drawn into Compose as a GPU texture, as MediaMP does (https://github.com/open-ani/mediamp, Apache-2.0, pre-1.0, v0.5.2): mpv's
   D3D11 render API shares frames with Skia's D3D12 device without CPU copies. MediaMP reaches Skiko internals by reflection, and the
   output is 8-bit SDR. Checked 2026-10-19 (M1 spike, ADR 0010):
   - The render API (https://github.com/mpv-player/mpv/pull/17764) was merged into mpv master on 2026-10-15; the latest release is still
     0.41.0, so MediaMP ships its own patched libmpv. Its design notes (`mediamp-mpv/mpv-d3d11-rendering.md`, in Chinese) describe
     `RGBA_8888` shared textures and reflection on Skiko's `Direct3DRedrawer`; its player forces tone mapping of HDR to SDR
     (`MpvMediampPlayer.jvm.kt`). MediaMP 0.5.2 is built against CMP 1.12.0 and resolves to Skiko 0.150.1 with CMP 1.12.1.
   - In the spike it played 4K60 HEVC with 2-3 dropped frames at start and none after, so performance is not the problem; HDR10 output is.
2. libmpv in a native child window (`wid`) with our controls in a separate transparent window: the only route that keeps HDR; the controls
   window must follow the video window (vlcj's docs report lag when dragging or resizing with this pattern:
   https://capricasoftware.co.uk/tutorials/vlcj/4/overlay). Chosen in ADR 0010; checked 2026-10-19 in the spike:
   - With `vo=gpu-next`, `gpu-api=d3d11` and `target-colorspace-hint=yes`, mpv switched its swap chain to `bt.2020/pq` on an HDR display
     with Windows HDR on, and to `bt.709/gamma2.2` (tone-mapped, 203 nits) when the window moved to an SDR monitor on another GPU, without
     restarting. Read the result from the `video-target-params` property.
   - Compose's `WindowPlacement.Fullscreen` is AWT's full-screen exclusive mode on Windows and Linux: Skiko 0.150.1's `PlatformOperations`
     calls `GraphicsDevice.setFullScreenWindow` (macOS uses its native fullscreen). Leaving it at 150 % display scale, the frame came back
     with its logical size applied as physical pixels while AWT kept the logical layout, so the child canvas was clipped. A borderless
     fullscreen on the same HWND (`SetWindowLongPtr` without the frame styles, `SetWindowPos` to the monitor rectangle) avoided it.
   - mpv rebuilds its swap chain on every size change: a few dropped frames per resize or fullscreen switch, none in steady playback.
   - The controls window must follow the frame's content area (AWT resizes it at once; the canvas follows only after Compose's next
     layout, and came out 2 px taller at 150 %), call `validate()` after `setBounds` (otherwise the Compose content kept its old size
     during a drag-resize), and leave a maximized frame through AWT (restyling a maximized HWND directly left AWT with stale bounds).
   - Hiding a window makes Windows send a mouse move to the window below although the cursor did not move; idle detection must compare
     positions. Compose's `detectTapGestures` with `onDoubleTap` delays every single tap by the double-tap timeout.
3. vlcj 4 + LibVLC 3: mature, but vlcj is GPLv3+ or paid, and rendering into Compose goes through CPU copies (LibVLC calls callback
   rendering "considerably less efficient").
- Not suitable: Compose Media Player on Windows (Media Foundation: no MKV/WebM, first audio stream only), JavaFX Media (no MKV, AC-3, DTS),
  GStreamer's Java bindings (last release 2021), JavaCV (a decoder, not a player).

**Licences**
- mpv: GPLv2+ by default, LGPL when built with `-Dgpl=false` (https://github.com/mpv-player/mpv/blob/master/Copyright). An app under any
  licence may load an LGPL `libmpv-2.dll` if it stays a separate, replaceable DLL and we ship its licence and source. MediaMP's own Linux
  runtime is GPL.
- Prebuilt LGPL libmpv for Windows is LGPLv3, not 2.1 (checked 2026-10-19): https://github.com/zhongfly/mpv-winbuild's `mpv-dev-lgpl`
  "statically links to ffmpeg under LGPLv3" (README), and mpv's rolling builds (https://github.com/mpv-player/mpv/releases/tag/git-release)
  say "LGPL builds are LGPLv3". MediaMP's `mediamp-mpv-runtime-windows-x64` 0.5.2 jar is LGPL-2.1+ (POM: Apache-2.0 and
  LGPL-2.1-or-later; read from the DLLs: libmpv `-Dgpl=false`, FFmpeg "LGPL version 2.1 or later" without `--enable-gpl`,
  `--enable-version3` or `--enable-nonfree`), but its FFmpeg is configured with `--disable-everything` and a short list (51 decoders; no
  VP8, MPEG-1, Theora, MJPEG, WavPack, APE, DVB subtitles, no MPEG-PS/VOB, FLV or WAV demuxers) and its libmpv has `-Dwin32-smtc=disabled`.
  A shipped build is therefore our own (ADR 0009, ADR 0010).
- vlcj: GPLv3+ or commercial (https://github.com/caprica/vlcj); LibVLC itself is LGPLv2+. Shipping vlcj makes the desktop build GPLv3.
- FFmpeg: LGPL unless GPL parts are enabled. Patent licensing for HEVC, AAC and AC-3 decoders is a separate question (https://ffmpeg.org/legal.html).

**Windows media keys.** System Media Transport Controls are a WinRT API
(https://learn.microsoft.com/en-us/windows/apps/develop/media-playback/system-media-transport-controls). libmpv has them built in
(`--media-controls`, Windows only, off by default for libmpv, since 0.39: https://github.com/mpv-player/mpv/blob/master/DOCS/man/options.rst).
The JVM libraries (JavaMediaTransportControls, mediasession-kt) were last released in 2024. Checked 2026-10-19: with a libmpv built
without `win32-smtc`, `media-controls=yes` is accepted but the process does not appear among the sessions of
`GlobalSystemMediaTransportControlsSessionManager`, so media keys go to another app.

**Web video**
- Candidates: (1) `<video>` (+ hls.js) behind the canvas, shown through a transparent hole drawn with `BlendMode.Clear` (Compose Media
  Player's web surface does this: https://github.com/kdroidFilter/ComposeMediaPlayer); it relies on undocumented DOM order and Skiko's
  alpha canvas. (2) Video drawn by Compose through the experimental WebGL render target (`rememberWebGLRenderTarget`, CMP 1.13.0-alpha01+,
  https://github.com/JetBrains/compose-multiplatform-core/pull/3323): CORS on every media URL, one GPU upload per frame, SDR only.
  (3) A plain DOM player screen, with controls that are not Compose.
- HLS: native in Chrome desktop 142+ and Safari, not in Edge or Firefox (caniuse). hls.js (v1.7.3, Apache-2.0) needs Media Source
  Extensions and CORS on every HLS resource; check `Hls.isSupported()` before relying on native playback (https://github.com/video-dev/hls.js).
  Kotlin/Wasm can import it as an npm module (`@JsModule`, or `js("import(...)")`: https://kotlinlang.org/docs/wasm-js-interop.html).
- Direct play in desktop browsers (caniuse; probe at runtime with `canPlayType` and `MediaSource.isTypeSupported`, as jellyfin-web does):
  MP4/WebM with H.264, VP9 or AV1 (Safari: AV1 with hardware only) and AAC, MP3, Opus or FLAC. HEVC only with hardware decoding
  (Safari always). MKV in Chromium-based browsers and in Firefox 145+ (which buffers the whole file). No AC-3, E-AC-3, DTS or TrueHD, so
  those need an audio transcode. Subtitles: WebVTT only; ASS and PGS need burn-in or a JS renderer.
- CORS: a plain `<video src>` plays cross-origin media without it; fetch (Ktor, image loading), hls.js, WebGL textures and cross-origin
  `<track>` need it (https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/CORS).
- Media Session API (web): metadata, action handlers, hardware media keys; MDN marks it "Limited availability", not Baseline.
  Source: https://developer.mozilla.org/en-US/docs/Web/API/Media_Session_API
- Checked 2026-10-19 in the web UI spike (ADR 0011; Playwright's Chromium 156 and Edge 155 on Windows):
  - Candidate (1) works with CMP 1.12.1, but the canvas is painted white where nothing is drawn. The player clears the *root* canvas with
    `drawBehind { drawRect(..., blendMode = BlendMode.Clear) }`; an offscreen `graphicsLayer` only clears its own layer. A `<video>` in a
    fixed layer under the Compose viewport then shows through, and the Compose controls on top get the input.
  - Probes:
    - `canPlayType('video/x-matroska; codecs="avc1.640029, mp4a.40.2"')` is "probably" in both browsers; MSE `isTypeSupported` for MKV is
      false.
    - HEVC (Main and Main10) through MSE is true with hardware (headed browser; false in headless Chromium).
    - Edge reports AC-3 and E-AC-3 as "probably" (Windows decoders); Chromium does not.
    - `application/vnd.apple.mpegurl` is "maybe" in Chromium 156.
    - Playwright launches Chromium with `--force-color-profile=srgb` by default, so `(dynamic-range: high)` is false even on an HDR
      display; pass `ignoreDefaultArgs: ['--force-color-profile=srgb']` to test HDR.
    - Full probe table recorded in installed Chrome 155, Edge 155 and Firefox 158 (ADR 0011, "Web playback routes"):
      - Both: MP4/WebM/MKV with H.264 (High 10 too), HEVC Main/Main10, AV1 8/10-bit, VP9 profiles 0/2, VP8; AAC, MP3, Opus, FLAC,
        Vorbis. No MOV or MPEG-TS through `src`, no DTS or TrueHD anywhere.
      - MSE in fMP4: no MP3 (`mp4a.6B`).
      - `decodingInfo` reports HDR10 (PQ) only for HEVC Main10, not AV1 10-bit or VP9 profile 2, and HLG for none.
      - `decodingInfo` reports 2160p smooth for H.264, HEVC, AV1 and VP9.
      - Edge adds AC-3 and E-AC-3 in `src` and MSE.
      - With Windows HDR on, Chrome and Edge report `(dynamic-range: high)`. Firefox does not; it has no HDR video output on Windows,
        though `decodingInfo` says it decodes HDR10 and HLG.
      - Firefox 158 plays MKV and MOV, HEVC Main/Main10, AV1 and VP9, but not 10-bit H.264, AC-3/E-AC-3, or FLAC in MKV.
      - `maxChannelCount` was 2 everywhere (stereo output).
    - Safari on macOS (recorded by the owner, 2026-10-19):
      - No MKV. MP4, WebM, MOV and MPEG-TS play (TS only with H.264 and AC-3/E-AC-3, and AAC in TS answered "").
      - HEVC answers only as `hvc1`, not `hev1`.
      - No AV1 on that Mac.
      - AC-3, E-AC-3, FLAC and ALAC play in MP4. FLAC in MSE answers only as `fLaC`, not `flac`.
      - MP3 inside MP4 answered "" under `mp4a.6B`, `mp4a.69` and `mp3`.
      - Native HLS is "maybe", and MSE is present.
      - `decodingInfo` reports HDR10 and HLG for HEVC Main10, but the display reported no HDR.
    - Raw answers of all four browsers: `docs/web-probes/`.
  - hls.js loaded with `import('hls.js')` from a `js()` function works in Kotlin/JS (webpack), Kotlin/Wasm (webpack) and Vite. It
    becomes a separate chunk of about 177 KB (gzip), loaded on the first HLS play.
  - A cross-origin `<track>` needs `crossorigin="anonymous"` on the `<video>` (Jellyfin answers with CORS); without it, Chromium refused the
    subtitle file.
  - Media Session metadata and action handlers set from Kotlin are present (`navigator.mediaSession.metadata`), and the owner saw the title
    and play/pause in the Windows media flyout.
  - hls.js gives up on a segment after 10 s without a first byte (`fragLoadTimeOut`), while Jellyfin holds a segment request until ffmpeg
    has written it. A software-only server re-encoding 4K HEVC 10-bit to H.264 had no first segment before its reverse proxy's 504 (even
    capped at 1080p, 20 Mb/s); with NVENC, segments came in 0.1–0.2 s. A web engine needs longer first-byte timeouts and error recovery
    (`startLoad()`, `recoverMediaError()`), and 4K transcodes need hardware acceleration on the server or a quality limit.
  - HDR through hls.js:
    - For an HDR source, Jellyfin's master playlist lists the copied stream (`VIDEO-RANGE=PQ`, `hvc1.2.4...`) next to tone-mapped
      `VIDEO-RANGE=SDR` variants (HEVC and H.264) at the same bandwidth. hls.js 1.7.3 picked SDR unless told otherwise.
    - `videoPreference: { preferHDR: true, allowedVideoRanges: ['SDR', 'PQ'] }` made Chrome decode HEVC Main 10 with PQ.
    - `chrome://media-internals` (`edge://media-internals`) shows the decoder's profile and colour space per player.
    - `(video-dynamic-range: high)` was false in Chrome and Edge with an HDR display, while `(dynamic-range: high)` was true.
  - Compose for web's canvas did not follow a runtime `devicePixelRatio` change (browser zoom): its backing size stayed at the old ratio,
    so zoomed text is blurry (owner; reproduced with CDP `Emulation.setDeviceMetricsOverride`, CMP 1.12.1).


## Licences, stores and patents for player engines (checked 2026-10-18)
- What a shipped library asks of us: permissive (MIT, BSD, ISC, Apache-2.0) only notices (Apache-2.0 also its NOTICE file); MPL-2.0 source of
  its own files only; LGPL-2.1/3.0 the library's source and our changes, and users must be able to relink (a replaceable dynamic library, or
  our app as objects or source when linked statically), with no terms that forbid modifying or debugging it; GPL makes the whole app GPL,
  however it links. GPLv2-only code cannot be combined with Apache-2.0 code. LGPLv3/GPLv3 "Installation Information" applies only to software
  conveyed with a consumer device, not to an app on its own. Sources: https://www.gnu.org/licenses/gpl-faq.html ,
  https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt , https://www.gnu.org/licenses/lgpl-3.0.txt , https://www.apache.org/licenses/GPL-compatibility.html
- Stores: the FSF considers the App Store's usage rules incompatible with the GPL, so Apple builds carry no GPL code. LGPL engines ship there in
  practice (VLCKit, MPVKit's non-GPL products). Google Play, the Microsoft Store (its policies cover open-source licences) and direct downloads
  accept GPL and LGPL. F-Droid needs FLOSS built from source. All three stores forbid pornographic apps; a client for the user's own
  Stash server is nonetheless listed in the US App Store (16+), so adult content is judged per store at review (ADR 0014). Sources: https://developer.apple.com/app-store/review/guidelines/ , https://learn.microsoft.com/en-us/windows/apps/publish/store-policies ,
  https://f-droid.org/docs/Inclusion_Policy/
- Apple fallback candidates: MPVKit (LGPLv3 by default, GPL only in its `-GPL` products; static libraries, so relinking means publishing our
  app as objects or source), VLCKit (LGPL-2.1+), an FFmpeg + VideoToolbox + Metal engine of our own (LGPL). KSPlayer is GPL-3.0 (LGPL only with a
  paid licence). Sources: https://github.com/mpvkit/MPVKit , https://github.com/videolan/vlckit , https://github.com/kingslay/KSPlayer
- Android: Media3 is Apache-2.0; its FFmpeg extension decodes audio only (AC-3, E-AC-3, DTS, TrueHD among others); AV1 through dav1d (BSD-2).
  mpv-android builds GPLv3, so an LGPL libmpv would need our own build. Source: https://github.com/androidx/media/tree/release/libraries
- Shared pieces: libass (ISC), libplacebo (LGPL-2.1+, can convert Dolby Vision profile 5; composes profile 7 only in its unreleased master,
  see "HDR and Dolby Vision: desktop"). Without a Dolby licence the safe route is the HDR10/SDR base layer.
  Sources: https://github.com/libass/libass , https://github.com/haasn/libplacebo
- Patents are separate from copyright: through OS decoders (VideoToolbox, MediaCodec, Media Foundation) the OS or device maker is the licensee;
  shipping FFmpeg's own HEVC or AAC decoders makes us the distributor of a decoder, so prefer OS and hardware decoders for those
  (https://ffmpeg.org/legal.html , https://accessadvance.com/licensing-programs/hevc-advance/ , https://www.via-la.com/licensing-programs/aac/).
  No Dolby or DTS logos or claims without a licence; neutral codec names (E-AC-3, DTS-HD MA) in the playback info overlay.

## HDR and Dolby Vision: Apple, AVPlayer (checked 2026-10-19, documentation only, no device)
Input for the shared `VideoRangeType` decision (ADR 0007, ADR 0011 "HDR"): what each engine can detect and play.
- **Device and display.**
  - `AVPlayer.eligibleForHDRPlayback` (class property; iOS, iPadOS, tvOS 13.4+, macOS 10.15+): "whether the current device can present
    content to an HDR display". It is one Boolean with no per-format answer. It is not key-value observable; watch
    `eligibleForHDRPlaybackDidChangeNotification` (posted when a display is connected or disconnected, among other changes).
  - `AVPlayer.availableHDRModes` (bitfield `hlg`, `hdr10`, `dolbyVision`; iOS, tvOS 11.2+, never on macOS) is deprecated in 26.0: "Use
    eligibleForHDRPlayback instead". Its page also says Mac Catalyst apps play no HDR on Intel Macs (HLS picks the SDR variant, files are
    tone-mapped to SDR).
  - Screen headroom: `UIScreen.potentialEDRHeadroom` / `currentEDRHeadroom` (iOS, tvOS 16+). On macOS,
    `NSScreen.maximumPotentialExtendedDynamicRangeColorComponentValue` (10.15+; above 1.0 means EDR, fixed when the `NSScreen` is created)
    and `maximumExtendedDynamicRangeColorComponentValue` (current value; changes post `didChangeScreenParametersNotification`).
  - tvOS mode switching:
    - A custom player sets the window's `AVDisplayManager.preferredDisplayCriteria` from the asset's `preferredDisplayCriteria`. Load it
      asynchronously; the synchronous property is deprecated in tvOS 16.
    - `AVPlayerViewController` does this itself (`appliesPreferredDisplayCriteriaAutomatically`).
    - The TV switches only when the user has turned on Match Content (`isDisplayCriteriaMatchingEnabled`, tvOS 11.3+).
    - Sample-buffer players build `AVDisplayCriteria(refreshRate:formatDescription:)` (tvOS 17+).
  - Decoders: `VTIsHardwareDecodeSupported(_:)` (iOS, tvOS 11+, macOS 10.13+) per codec type, e.g. `kCMVideoCodecType_HEVC`,
    `kCMVideoCodecType_DolbyVisionHEVC`, `kCMVideoCodecType_AV1`.
  - Media: `AVURLAsset.isPlayableExtendedMIMEType(_:)` takes a MIME type with codecs. `AVMediaCharacteristic.containsHDRVideo` (iOS, tvOS
    14+, macOS 11+) marks HDR tracks. HLS variants expose `AVAssetVariant.VideoAttributes.videoRange` (`AVVideoRange` `sdr`, `hlg`, `pq`)
    and `codecTypes` (iOS, tvOS 15+, macOS 12+).
- **What the HLS Authoring Specification requires** (revision 2025-06-24):
  - HEVC and AV1 only in fMP4 (rules 1.5, 1.39). HDR HEVC must be HDR10, HLG or Dolby Vision (1.7). `VIDEO-RANGE` is required unless
    everything is SDR (9.16).
  - Dolby Vision: profile 5 up to level 7 (1.9). The iOS amendment allows profile 5 or 10 up to level 9 (1.9a); visionOS allows profile 5,
    and profile 20 (MV-HEVC) for stereo. Profiles 7 and 8 are not mentioned. AirPlay 2 TVs: offer both Dolby Vision and HDR10 (9.24).
  - Sample entries with parameter sets out of band SHOULD be used: `hvc1` or `dvh1`, not `hev1` or `dvhe` (1.10).
- **Profile 8 in HLS:** Apple's HLS 2nd edition draft (`draft-pantos-hls-rfc8216bis-22`, May 2026) gives this example for Dolby Vision
  8.4: `CODECS` with the base layer `hvc1.2.4.L153.b0` and `SUPPLEMENTAL-CODECS` `dvh1.08.07/db4h` (codec, then a compatibility brand).
- **Profile 8.4 files:** Apple's Dolby Vision guide (2021, iOS 14.1+, written for iPhone camera video in `.mov`):
  - `AVPlayer` with `AVPlayerLayer`, and `AVSampleBufferDisplayLayer`, play profile 8.4 with no extra code when `eligibleForHDRPlayback`
    is true.
  - A `VTDecompressionSession` must keep `kVTDecompressionPropertyKey_PropagatePerFrameHDRDisplayMetadata` true and use 10-bit buffers
    (`x420`).
  - `AVPlayerItem.appliesPerFrameHDRDisplayMetadata` (default true; iOS, tvOS 14+, macOS 11+) turns the per-frame metadata off.
- **HDR10+:** no Apple document mentions it. AVFoundation, VideoToolbox and the HLS spec name HDR10, HLG and Dolby Vision only.
- **Containers:** the types AVFoundation opens come from `AVURLAsset.audiovisualContentTypes` (`audiovisualTypes()` is deprecated in
  27.0). Safari on macOS played no MKV (ADR 0011). macOS 15+ (not iOS or tvOS) has the MediaExtension framework: format readers and
  decoders embedded in a host app, available to apps that opt in. Dolby Vision from MKV therefore reaches AVPlayer only through a server
  remux to fMP4 HLS (see "Jellyfin 12.2").
- Sources: https://developer.apple.com/documentation/avfoundation/avplayer/eligibleforhdrplayback ,
  https://developer.apple.com/documentation/avfoundation/avplayer/availablehdrmodes ,
  https://developer.apple.com/documentation/avfoundation/avdisplaycriteria , https://developer.apple.com/documentation/avkit/avdisplaymanager ,
  https://developer.apple.com/documentation/uikit/uiscreen/potentialedrheadroom ,
  https://developer.apple.com/documentation/appkit/nsscreen/maximumpotentialextendeddynamicrangecolorcomponentvalue ,
  https://developer.apple.com/documentation/videotoolbox/vtishardwaredecodesupported(_:) ,
  https://developer.apple.com/documentation/avfoundation/avplayeritem/appliesperframehdrdisplaymetadata ,
  https://developer.apple.com/documentation/http-live-streaming/hls-authoring-specification-for-apple-devices ,
  https://datatracker.ietf.org/doc/draft-pantos-hls-rfc8216bis/ ,
  https://developer.apple.com/av-foundation/Incorporating-HDR-video-with-Dolby-Vision-into-your-apps.pdf ,
  https://developer.apple.com/documentation/mediaextension

## HDR and Dolby Vision: Android, Media3 (checked 2026-10-19 against the Android reference and the Media3 1.11.1 source)
- **Display:**
  - `Display.getHdrCapabilities().getSupportedHdrTypes()` (API 24) is deprecated in API 34 in favour of `Display.getMode()` and
    `Mode.getSupportedHdrTypes()` (API 34).
  - Types: `HDR_TYPE_DOLBY_VISION` (1), `HDR_TYPE_HDR10` (2), `HDR_TYPE_HLG` (3) since API 24, `HDR_TYPE_HDR10_PLUS` (4) since API 29,
    `HDR_TYPE_HLG_PLUS` (6) since API 37.
  - `Display.isHdr()` (API 26) answers "any HDR type". `getHdrSdrRatio()` (API 34) gives the HDR/SDR luminance ratio when
    `isHdrSdrRatioAvailable()`.
  - `DisplayManager.getHdrConversionMode()` (API 34) returns the user's HDR conversion setting.
- **Decoders:** `MediaCodecInfo.CodecCapabilities.profileLevels` per MIME type. `CodecProfileLevel` constants:
  - Dolby Vision: `DolbyVisionProfileDvheStn` (profile 5), `DvheDtb` (7), `DvheSt` (8, API 27), `DvavSe` (9, API 27), `Dvav110` (10, API 30).
  - HDR: `HEVCProfileMain10HDR10` (API 24), `HEVCProfileMain10HDR10Plus`, `AV1ProfileMain10HDR10`, `AV1ProfileMain10HDR10Plus`,
    `VP9Profile2HDR10Plus` (API 29), `VP9Profile2HDR` (API 24).
  - Dolby Vision decoders are `video/dolby-vision`. Media3 also maps two vendor names: `OMX.MS.HEVCDV.Decoder` as `video/hevcdv`, and
    Realtek's decoders as `video/dv_hevc`.
- **Media3 1.11.1 (2026-09-10):**
  - Extractors:
    - MP4 reads `dvav`, `dva1`, `dvhe`, `dvh1` and `dav1` sample entries and `dvcC`/`dvvC`/`dvwC` boxes.
    - Matroska reads `dvcC`/`dvvC` from `BlockAdditionMapping`. Both give `video/dolby-vision` with codecs `dvhe.PP.LL`, `dvav...` or
      `dav1...`, so Dolby Vision in MKV is recognised.
    - `DolbyVisionConfig` keeps profile and level only, not the base-layer compatibility id.
    - HLS `SUPPLEMENTAL-CODECS` and DASH `scte214:supplementalCodecs` are parsed to detect Dolby Vision.
  - Fallback without Dolby Vision (API 26+): `MediaCodecVideoRenderer` checks the *default* display (`isHdr()` and
    `HDR_TYPE_DOLBY_VISION` through the deprecated `getSupportedHdrTypes()`). If it lacks Dolby Vision, the renderer first tries
    `MediaCodecUtil.getAlternativeCodecMimeType`:
    - profiles 4 and 8 → HEVC decoder;
    - profile 9 → H.264;
    - profile 10 → AV1, unless the track's colour info is PQ with full range (taken as 10.0, no fallback);
    - profiles 5 and 7 → no fallback ("not backward compatible" and "deprecated and is not always backward compatible").

    A fallback reports `DECODER_SUPPORT_FALLBACK_MIMETYPE`. The 8.x sub-profiles (8.1, 8.2, 8.4) are not told apart, so the HEVC decoder
    gets the HDR10, SDR or HLG base layer as is.
  - Track selection prefers other tracks to Dolby Vision when the display does not support it (release notes, ExoPlayer #8944).
  - Without a fallback, the renderer uses whatever `video/dolby-vision` decoder the device has. With none, profiles 5, 7 and 10.0 need the
    server. What a Dolby Vision decoder outputs to a display without Dolby Vision is not documented.
  - HDR10+:
    - In-band SEI stays in the HEVC bitstream for the decoder.
    - Out-of-band metadata (ITU-T T.35 in supplemental data, e.g. WebM) goes to `MediaCodec` as `KEY_HDR10_PLUS_INFO` when the decoder
      reports out-of-band HDR10+ support (API 29+).
    - What the screen does with it is up to the device.
- Sources: https://developer.android.com/reference/android/view/Display.HdrCapabilities ,
  https://developer.android.com/reference/android/view/Display.Mode , https://developer.android.com/reference/android/view/Display ,
  https://developer.android.com/reference/android/media/MediaCodecInfo.CodecProfileLevel ,
  https://github.com/androidx/media/blob/1.11.1/libraries/exoplayer/src/main/java/androidx/media3/exoplayer/mediacodec/MediaCodecUtil.java ,
  https://github.com/androidx/media/blob/1.11.1/libraries/exoplayer/src/main/java/androidx/media3/exoplayer/video/MediaCodecVideoRenderer.java ,
  https://github.com/androidx/media/blob/1.11.1/libraries/container/src/main/java/androidx/media3/container/DolbyVisionConfig.java ,
  https://github.com/androidx/media/blob/1.11.1/RELEASENOTES.md

## HDR and Dolby Vision: desktop, libmpv with `vo=gpu-next` (checked 2026-10-19 in the source; nothing played)
Releases at the time: mpv 0.41.0 (2025-12-21), libplacebo v7.360.1 (2026-03-13), FFmpeg 8.1.3 (2026-09-21). Master of mpv and libplacebo
as of 2026-10-18.
- **Decoding:** FFmpeg's HEVC decoder parses the RPU (NAL type 62) and attaches `AV_FRAME_DATA_DOVI_METADATA`. It does so in
  `hevc_frame_start` whether or not a hwaccel such as `d3d11va` decodes the slices. The native AV1 decoder and libdav1d do the same for
  profile 10. Jellyfin's `EncodingHelper` relies on the same split: "Only native SW decoder, HW accelerator and hevc_rkmpp decoder can
  parse dovi rpu".
- **Rendering:** mpv hands the metadata to libplacebo (`pl_map_avdovi_metadata`), which reshapes Dolby Vision in its own shaders
  (`PL_COLOR_SYSTEM_DOLBYVISION`).
  - Profile 5 comes out as BT.2020 PQ, or tone-mapped SDR.
  - For 8.1, 8.4 and 10, the RPU reshaping is applied over the base layer.
  - The display never receives Dolby Vision: output is HDR10 (`target-colorspace-hint`) or SDR.
  - `--target-colorspace-hint-mode=source-dynamic` (experimental) sends per-scene luminance as HDR10 metadata and "does not send full
    HDR10+ or Dolby Vision metadata".
- **Profile 7 in the releases:**
  - mpv 0.41.0 maps Dolby Vision metadata only when the RPU's `disable_residual_flag` is set. libplacebo 7.360.1's `pl_map_avframe` has
    the same rule ("Only automatically map DoVi RPUs that don't require an EL").
  - Profile 7 therefore plays as its HDR10 base layer: EL and RPU are ignored.
- **Profile 7 in master (unreleased):**
  - libplacebo API 364 maps MEL-only RPUs. APIs 366-370 compose FEL through `pl_frame.enhancement_layer` (commit 801b681, "renderer: add
    support for Dolby Vision FEL merging", authored 2026-05-16, merged 2026-07-01).
  - mpv master (2026-07-01) splits an EL interleaved as NAL type 63 into a virtual track with FFmpeg's new `dovi_split` bitstream filter.
    That filter is in FFmpeg master since 2026-05-31; it is not in 8.1.3, nor in jellyfin-ffmpeg 8.1.3-1.
  - mpv decodes that track with a second decoder and passes the frames as the enhancement layer. It also pairs separate BL and EL tracks
    (libavformat `AV_STREAM_GROUP_PARAMS_DOLBY_VISION`, 62.19.100+, and `demux_mkv`).
  - `vf=format:enhancement-layer=no` turns FEL off. `vf=format:dolbyvision=no` strips Dolby Vision metadata, leaving the base layer, and
    is in 0.41.0. `vf=format:hdr10plus=no` strips HDR10+.
- **HDR10+:** mpv reads `AV_FRAME_DATA_DYNAMIC_HDR_PLUS` into libplacebo's per-scene HDR metadata. It drives tone mapping (e.g.
  `tone-mapping=st2094-40`) and, with `source-dynamic`, per-scene HDR10 values. The HDR10+ metadata itself never reaches the display.
- **Build and licence:**
  - libplacebo's `dovi` meson feature (default `auto`) enables reshaping and needs FFmpeg's `libavutil/dovi_meta.h` (libavutil
    57.16.100+).
  - The optional `libdovi` dependency (1.6.7+; the library of dovi_tool, MIT) adds `pl_hdr_metadata_from_dovi_rpu`: RPU brightness
    metadata for tone mapping.
  - Neither needs a Dolby SDK, key or licence file. libplacebo is LGPL-2.1+.
  - Whether a distributed build that applies Dolby Vision reshaping needs a Dolby licence is a legal question these sources do not answer.
    ADR 0014 keeps full processing to the Private edition; the Store edition plays the base layer (`dolbyvision=no`).
  - Profiles 5 and 10.0 have no base layer to fall back on (Jellyfin's `DOVI` range), so without reshaping they need the server.
- Sources: https://github.com/mpv-player/mpv/blob/v0.41.0/video/mp_image.c , https://github.com/mpv-player/mpv/blob/master/demux/dovi_split.h ,
  https://github.com/mpv-player/mpv/blob/master/DOCS/man/vf.rst , https://github.com/mpv-player/mpv/blob/master/DOCS/man/options.rst ,
  https://github.com/haasn/libplacebo/blob/v7.360.1/src/include/libplacebo/utils/libav_internal.h ,
  https://github.com/haasn/libplacebo/commit/801b6812fb3fcadc7a44ab5f8585d3a1b5302a5f , https://github.com/haasn/libplacebo/blob/master/meson.build ,
  https://github.com/FFmpeg/FFmpeg/blob/master/doc/bitstream_filters.texi , https://github.com/FFmpeg/FFmpeg/blob/n8.1.3/libavcodec/hevc/hevcdec.c ,
  https://github.com/quietvoid/dovi_tool

## HDR and Dolby Vision: Jellyfin 12.2 HLS remux (checked 2026-10-19 in the v12.2 source and jellyfin-ffmpeg 8.1.3-1 patches)
- **Classification** (`MediaStream.cs`, from the stream's Dolby Vision configuration):
  - profile 5 → `DOVI`;
  - 8 with compatibility id 1, 4 or 2 → `DOVIWithHDR10`, `DOVIWithHLG` or `DOVIWithSDR` (any other id → `DOVIInvalid`);
  - 7 → `DOVIWithEL`;
  - 10 with id 0, 1, 2 or 4 → `DOVI`, `DOVIWithHDR10`, `DOVIWithSDR` or `DOVIWithHLG`.
  - HDR10+ metadata turns them into `DOVIWithHDR10Plus`, `DOVIWithELHDR10Plus` or `HDR10Plus`.
  - A base layer whose colour tags do not match its id → `DOVIInvalid`.
- **Copy or transcode** (`EncodingHelper`, the range types the profile lists for the source codec):
  - `DOVI` sources are copied only if `DOVI` is listed.
  - `DOVIWith{HDR10,HLG,SDR}` are copied when the base layer's range is listed, and `HDR10Plus` when `HDR10` is.
  - Anything else is copied only when the metadata removal it needs is possible.
- **The Dolby Vision sample entry and configuration box need `DOVI` in the client's list for the output codec** (`DynamicHlsController`,
  "Only enable Dolby Vision remuxing if the client explicitly declares support for profiles without fallbacks"):
  - Sample entry: HEVC is copied with `-tag:v dvh1 -strict -2`, except profile 8, which gets `hvc1`; AV1 gets `dav1`. Without `DOVI`, HEVC
    is tagged `hvc1` and no `-strict` is added for the video.
  - Configuration box: FFmpeg's mov muxer writes `dvcC`/`dvvC`/`dvwC` only at `-strict unofficial` or lower ("Requires -strict
    unofficial"). jellyfin-ffmpeg patch 0027 copies the configuration into the HLS segment muxers and validates it (profile against codec,
    compatibility id against colour tags) before writing the box.
- **Master playlist** (`DynamicHlsHelper`, video copied):
  - With `DOVI` listed, profile 5 and 10.0 (`DOVI`) get an extra first variant `CODECS="dvh1.PP.LL"` (or `dav1.PP.LL`), `VIDEO-RANGE=PQ`,
    before the `hvc1` variant.
  - Whenever Dolby Vision is copied and not removed, 8.1, 8.4 and 10.x with a base layer get `SUPPLEMENTAL-CODECS="dvh1.PP.LL/db1p"`
    (HDR10) or `/db4h` (HLG) next to the base-layer `CODECS`. This does not depend on `DOVI` being listed.
  - SDR-based, profile 5 and profile 7 sources get no supplemental codec.
  - HDR10+ kept in a copy, without Dolby Vision, gets `SUPPLEMENTAL-CODECS="<codec>/cdm4"`.
  - Transcodes are always `VIDEO-RANGE=SDR`.
- **Profile 7: stripped in a remux, not converted.**
  - For a client that lists `HDR10` but not `DOVIWithEL`, `ShouldRemoveDynamicHdrMetadata` chooses `RemoveDovi`, and the video is still
    copied. The filter is `hevc_metadata=remove_dovi=1`, or FFmpeg's `dovi_rpu=strip=1` when the patched one is missing.
  - jellyfin-ffmpeg patch 0061 removes the access unit's last NAL if it is an RPU (62), then the new last NAL if it is EL (63), and
    drops the configuration record. Upstream `dovi_rpu=strip` deletes the last unit if it is 62 or 63, plus the record.
  - The result is an HDR10 stream. There is no profile 7 → 8.1 conversion (keep the RPU, drop the EL) in 12.2.
  - Other removals:
    - `DOVIInvalid` to a `DOVI` client: Dolby Vision is removed.
    - `DOVIWithHDR10Plus` to a `DOVI` client: HDR10+ is removed (`hevc_metadata=remove_hdr10plus=1`).
    - AV1 has matching `av1_metadata` options.
- **Tone mapping in transcodes:** `DOVI` sources (profile 5) are reshaped only by the software decoder or by NVDEC, VAAPI, D3D11VA,
  VideoToolbox and RKMPP (FFmpeg 7.1.1+) decoders. VideoToolbox tone mapping is not used for profile 5 ("does not produce correct mapping
  results").
- Sources: https://github.com/jellyfin/jellyfin/blob/v12.2/MediaBrowser.Model/Entities/MediaStream.cs ,
  https://github.com/jellyfin/jellyfin/blob/v12.2/MediaBrowser.Controller/MediaEncoding/EncodingHelper.cs ,
  https://github.com/jellyfin/jellyfin/blob/v12.2/Jellyfin.Api/Controllers/DynamicHlsController.cs ,
  https://github.com/jellyfin/jellyfin/blob/v12.2/Jellyfin.Api/Helpers/DynamicHlsHelper.cs ,
  https://github.com/jellyfin/jellyfin-ffmpeg/blob/jellyfin/debian/patches/0061-add-remove-dovi-hdr10plus-bsf.patch ,
  https://github.com/jellyfin/jellyfin-ffmpeg/blob/jellyfin/debian/patches/0027-pass-dovi-sidedata-to-hlsenc-and-mpegtsenc.patch ,
  https://github.com/FFmpeg/FFmpeg/blob/master/libavformat/movenc.c

