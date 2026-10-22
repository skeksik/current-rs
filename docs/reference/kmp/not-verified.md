# Not verified (from platform notes)

> Reference only, copied unchanged from the earlier Kotlin Multiplatform project (`skeksik/Current` at `66452b2`, 2026-10-22).
> Facts carry the dates they were checked on. ADR numbers, module names and "the app" refer to that project. Nothing here is a
> decision for this repo; see `README.md` in this folder.

## Not verified (research before depending on it)
- Single instance on a Linux desktop (section "Single instance on desktop"); Windows: section "Single instance: Windows run".
- Desktop installers on a real machine (see "Desktop installer: Windows run" and "Desktop packaging"): NVDA in the packaged app (the
  checklist in "Windows screen readers and the desktop app"), the
  window icon and the new install folder (`Current App`) by hand, the Installed-apps icon in Settings, the DEB.
- That the internet probe logged as `no response` at home was cancelled by its check, not failed (section "The internet probe that
  "fails" at home"): read in the code, not yet seen as `cancelled` on a device.
- Which exceptions the Android, Darwin and JS Ktor engines throw for a refused connection, an unknown host or a TLS failure. ADR 0015
  maps only `IOException`s to `Unreachable`; anything else passes through unmapped, and its message may contain the request URL.
  Seen on Android 17: a missing `INTERNET` permission throws `SecurityException` (section "Roaming on Android"); PR 23 maps that one
  to `Unreachable`.
- Whether `HttpTimeout`'s request timeout works the same on the Darwin and JS engines; on a blocking engine it does not end the call
  (section "Request timeouts on blocking engines"; `RequestDeadline` returned on time on an Android 17 phone there).
- Local server discovery after the 2026-10-20 runs (see "Local server discovery, run on devices"): Android 17 in the background, in a
  WebView and on Android TV, and which address ranges count as local; macOS, tvOS and iPadOS; a distribution-signed
  iOS build and the directed-broadcast gap seen there; Linux on hardware; Safari; LAN host names instead of IP literals in browsers; a
  server on host networking (its `Address` field).
- For the video spikes: desktop playback on hybrid-GPU laptops and lower-end GPUs (ADR 0010 measured one desktop with an RTX 5090), tone-mapped output
  with Windows HDR off, libmpv's `win32-smtc` from an unpackaged JVM app (needs a build with it), the `wid` route on macOS and Linux desktops,
  interop blending over an OpenGL-rendered video, Compose popups as separate windows (`compose.layers.type=WINDOW`) above a native video window,
  HDR on web as seen on screen (the HDR10 stream reaches the decoder in Chrome and Edge, ADR 0011), patent licensing for HEVC/AAC/AC-3 decoders.
- Whether a browser app on another origin can sign in to Seerr through a reverse proxy without Seerr changes (cookie scope, CSRF). Mixed content
  is covered under "Local network and mixed content: browsers"; it was read, not tried in a browser.
- Playback reporting was never sent to a live server. Password and Quick Connect sign-in, the restored-session check and sign-out were
  (2026-10-20, see "Jellyfin 12.2: sign-in shapes"); `PlaybackInfo` was sent live during the web spike (see "Jellyfin 12.x").
- Seerr sign-in through the app's Jellyfin session (2026-10-20; source and specs only):
  - The whole flow against live Seerr 3.5.0 and Jellyfin 12.2, including that approving the code ourselves lets `authenticate` succeed
    without polling `check`, and the same-server check with `GET /QuickConnect/Connect?secret=`.
  - That the server-side session expiry slides while the cookie's `Expires` does not (express-session 1.19.0 and connect-typeorm 2.0.0,
    the versions Seerr pins, were not read); the csurf error status; whether `express-rate-limit` guards any route.
  - That Seerr's sign-out leaves the Quick Connect "Seerr" device on Jellyfin; the exact 401 body of Jellyfin's `Unauthorized("...")`;
    what status a disabled caller gets from `Authorize`.
  - Whether Ktor's cookie storage keeps a Secure cookie for an `http` URL.
- Dolby Vision's base layer in browsers without Dolby Vision. The web profile accepts `DOVIWithHDR10`, `DOVIWithHDR10Plus`,
  `DOVIWithSDR` and (with HLG) `DOVIWithHLG` everywhere, as ADR 0011 assumes, but no Dolby Vision file was ever played in a browser.
  Check profile 8.1 in Chrome as MKV and as MP4, the latter with a `dvh1`/`dvhe` sample entry as well as `hvc1`. If one fails, turn
  `baseLayerFallback` off for the web or add a `VideoCodecTag` condition.
- Firefox playback was checked only by hand (Playwright cannot drive a stock Firefox); the ADR 0011 hand checks are recorded there.
- Which DeviceProfile each engine needs (Media3, AVPlayer, desktop engine; HTML video: ADR 0011), and how they cope with Stash's 302-to-login and mislabelled mime types.
- Compose Multiplatform web accessibility with a real screen reader. The accessibility docs page does not cover web. The web spike saw
  the accessibility tree only through Playwright: buttons and headings present, a closed menu's items left behind (ADR 0011).
- Media3 capabilities for the Android fallback (e.g. FFmpeg extension) and licences of any custom Apple fallback engine library.
- HDR and Dolby Vision (the 2026-10-19 sections were read from documentation and source; nothing was played):
  - Apple:
    - Whether AVPlayer plays profile 8.1 (only 8.4 files and profiles 5 and 10 in HLS are documented), and how it treats 7 (presumably
      unsupported).
    - Whether HDR10+ dynamic metadata is used or dropped.
    - Whether `hev1`/`dvhe` play at all, or only `hvc1`/`dvh1`.
    - How to tell Dolby Vision from HDR10 display support once `availableHDRModes` is gone.
    - Which types `audiovisualContentTypes` lists per OS.
    - AVPlayer's variant choice with Jellyfin's master playlist (the `dvh1` variant first, then `hvc1`).
  - Android:
    - What a device's Dolby Vision decoder outputs on a display without Dolby Vision.
    - Whether every HEVC decoder ignores RPU NALs (type 62) in Media3's fallback. The release notes list a fixed failure of "some
      playbacks of Dolby Vision files ... when attempting to use a fallback AVC or HEVC codec".
    - Behaviour on external displays (Media3 checks only the default display).
    - What Android TV's HDR conversion setting does to the reported types.
  - Desktop:
    - Dolby Vision colours from mpv/libplacebo against a reference (profile 5, 8.1, 8.4, 10).
    - The cost of the second EL decoder in master, and when mpv, libplacebo and FFmpeg release the profile 7 FEL path.
    - Whether a distributed build that applies Dolby Vision reshaping needs a Dolby licence (legal question, ADR 0009).
  - Jellyfin:
    - Whether a stripped profile 7 stream plays cleanly everywhere. `hevc_metadata=remove_dovi` removes at most one trailing EL NAL per
      access unit after the RPU, so EL NALs (type 63) earlier in the unit may remain.
    - The live master playlist and init segment for each profile with and without `DOVI` in the device profile (only the code was read).
    - Whether `SUPPLEMENTAL-CODECS` without a `dvcC` box (profile 8, `DOVI` not listed) confuses any client.
- Server addresses, discovery and roaming (2026-10-19; documentation and source only, no device, no LAN, no live server):
  - Jellyfin:
    - The discovery handshake was sent to a live 12.2 server on 2026-10-20 (see "Local server discovery, run on devices"); the
      jellyfin-web `AutoDiscovery` checkbox exists at the 12.2 tag, and no mDNS package was found (still not a repo-wide search).
    - The `UserDto` shape of `/Users/Public`; the HTTPS redirect status code; whether `GetBindAddress` returns the interface address when
      `LocalNetworkAddresses` is set; how Docker delivered subnet broadcasts to a bridge-networked container (they arrived, untried why).
  - Seerr and Stash:
    - Whether an admin can change Seerr's `clientId`; what `commitTag` holds on a release image; the default `applicationTitle`; the
      validator's 401 body.
    - Stash: the `systemStatus` resolver, the open `/graphql` response, the `/css` and `/assets` bodies, the chi `/healthz` body (taken
      from chi's source through a summarizer), the "local clients only" rule when no credentials are set.
  - Apple:
    - What tvOS does with local traffic and UDP broadcast (TN3179 lists no local network privacy for tvOS; the entitlement page does not
      mention tvOS; the ATS key pages do not list tvOS).
    - Whether Apple grants the multicast entitlement for a media client, and how fast.
    - Whether `NSAllowsLocalNetworking` admits public IP addresses, and whether AVPlayer follows it.
    - That a dotted non-`.local` name such as `media.lan` is blocked by ATS under `NSAllowsLocalNetworking` (read from the key's text, not tried).
    - Whether `NWPathMonitor` reports the current path at start and fires for a Wi-Fi to Wi-Fi switch on iOS and tvOS (it does on
      macOS 26.6.2), and for a move between access points of one network.
    - Whether `UIApplicationWillEnterForegroundNotification`, which the Apple `NetworkChanges` observes by that string name, is posted
      under the SwiftUI scene lifecycle on tvOS. On iOS 27.0.1 it is, at every return and every cold launch (section "Roaming on iOS,
      final"); not checked in Apple's documentation.
    - Whether `UIApplicationDidEnterBackgroundNotification`, observed by that string name, is posted under the SwiftUI scene lifecycle on
      tvOS, and early enough for the loop to handle it before the app is suspended. On iOS 27.0.1 it is, at every trip to the
      background, 0.01-1.3 s before suspension (section "Roaming on iOS, suspension fix"); not checked in Apple's documentation.
  - Android:
    - Which addresses count as "local network" for `ACCESS_LOCAL_NETWORK`; whether a `domain-config` can name a LAN IP literal.
    - Media3's and Coil's handling of the cleartext policy. (A broadcast discovery needed no `MulticastLock` on Android 16 or 17, see
      "Local server discovery, run on devices".)
    - Roaming between Wi-Fi and mobile data, and between access points of one network; why a probe of the internet address on home
      Wi-Fi sometimes failed fast right after joining (section "Roaming on Android").
  - Desktop: OS-level network change notifications (Windows, macOS, Linux). (`255.255.255.255` leaves one interface only on a
    multi-homed Windows PC, and a sender that reads unicast replies got no firewall prompt: see "Local server discovery, run on devices".)
  - Web:
    - Safari's mixed-content exemption for LAN addresses (WebKit's Local Network Access work was not opened); whether media elements
      can reach a `.lan` host from an HTTPS page in Chrome. (Firefox 158 has none for an IP literal, and a denial in Chrome and Edge 155 is
      a `TypeError` at once: see "Local server discovery, run on devices".)
    - Chrome 145 and 156 changes (third-party reports only).
  - ktor-network: the wasm-js source jar was not opened; nothing was run on any target.
- Apple system integration with a custom player (section of that name; documentation only):
  - That the fallback engine can feed an `AVSampleBufferDisplayLayer` for PiP (and keep HDR there), and that `MPNowPlayingSession`
    works on tvOS without an `AVPlayer`.
  - That media keys and the macOS menu bar's Now Playing reach `MPRemoteCommandCenter` and `MPNowPlayingInfoCenter` (the pages say
    "system controls", Lock Screen and Control Center, not media keys or the menu bar by name).
  - AirPlay video from the fallback engine: `AVRoutePickerView` picks a route, but what plays there without `AVPlayer` is not stated.
  - Matching frame rate and dynamic range on iOS and macOS (no display-criteria API there).
  - The remux route (ADR 0007): whether AVPlayer plays a local HLS/fMP4 repackaged on the device, and how it is served to it.
- Exact CMP/Kotlin/AGP/Gradle compatibility of our pinned set (JVM tests and desktop compile pass on Linux; CI also builds the web, Android and Apple targets since 2026-10-18, roadmap M0).
