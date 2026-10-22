# Roadmap

[x] done, [ ] not started. Each milestone ends with something usable; "Done when" is the exit check. Agreed on 2026-10-22: desktop
first (Windows, then macOS, then Linux), then the other platforms (ADR 0001). Reorder later milestones as priorities change.

## Principles
- Anyone may use Current against their own servers: every service (Jellyfin, Seerr, Stash) is optional at runtime.
- All logic is in the Rust core; the native UIs render its state (ADR 0001, ADR 0002).
- Native behaviour by default; departures are discussed with the owner first (ADR 0001).
- Check API and platform facts against primary sources before relying on them. The repo is public: nothing private in it.

## D: Desktop foundations
Spikes first: they decide whether the stack works at all. Each spike ends in a decision record or notes; its prototype is removed
before merging, and its PR carries only the record.
- [x] D0 (2026-10-22, PR 1): docs foundation: CLAUDE.md, architecture, ADRs 0001-0003, `/manage`, PR template,
      CI (Rust on Linux, Windows and macOS; licences with cargo-deny)
- [x] D1 spike (2026-10-22, PR 3, ADR 0004): the UniFFI bridge. A small API (an observable value, a list with diffs, an async call with cancellation, an error, a
      listener implemented by the UI) shown in a minimal WinUI 3 app and a minimal SwiftUI app, both built in CI. Decides the exact
      observable pattern and how each UI wraps it (ADR 0002). Built in CI; running both apps by hand is left to D5's checks
- [x] D2 spike (2026-10-22, PR 4, ADR 0007): Windows video. libmpv into a WinUI 3 `SwapChainPanel` (mpv 0.41's D3D11 composition mode, `display-swapchain`) with
      XAML controls over the video; real HDR10 output on an HDR display; resizing, full screen, several monitors; media keys and the
      Windows media flyout (`SystemMediaTransportControls` for a window). Fallbacks if it fails: mpv's D3D11 render API (unreleased,
      mpv 0.42) into our own swap chain; libmpv in a child window (`wid`) with our controls in an owned
      overlay window
- [x] D3 spike (2026-10-22, PR 6, ADR 0008): macOS video. AVPlayer under our own SwiftUI controls: HDR/EDR, PiP, Now Playing and remote commands, AirPlay; which of
      our formats need a server remux or transcode, so the size of the gap a fallback engine must fill is known
- [x] D4 (2026-10-22, PR 2, ADRs 0005-0006): `current-model` and `current-net`: identity and open values, one client per server, error model, redacted request log
- [ ] D5a: `current-app` and `current-ffi` (ADR 0004): the composition root, app info, the capabilities state and diagnostics over
      the bridge; C# and Swift bindings with their adapters, built in CI
- [ ] D5b: Windows app shell and packaging: the WinUI app over `current-ffi`, navigation, settings window, unpackaged self-contained build,
      per-user install in `%LOCALAPPDATA%` (program folder never the data folder), an AppUserModelID so the media flyout names the app,
      installer tested in CI; the bridge checks D1 left unrun
- [ ] D6: our LGPL-2.1 FFmpeg and libmpv builds for Windows in CI (ADR 0003, ADR 0007), with the codec list the player needs; no
      LAME; uchardet allowed (owner, 2026-10-22)
- [ ] D7 spike: macOS subtitle renderer over AVPlayer (ADR 0008): styled ASS through libass and PGS/VobSub bitmaps, fed by subtitle
      streams the server extracts. A full fallback engine waits for local and offline playback
- [ ] D8 spike: Linux. A GTK4 + libadwaita window on the core; libmpv in a `GtkGLArea` (SDR) and the state of HDR on Linux

## M1: Jellyfin sign-in and server setup (Windows)
Each item starts with our own research (the server's API and source, the platform's documentation, how existing clients behave),
recorded with sources and dates, and a decision record where it decides something.
- [ ] Server setup: add a Jellyfin server by URL, check it is the same server on every address, warn about plain HTTP except locally
- [ ] Several addresses per server with roaming (local and internet, the user's order)
- [ ] Secrets in the OS keyring and a device identity per install
- [ ] Sign-in with a password or Quick Connect; sessions restored at start and dropped on a 401; sign out
- [ ] Local server discovery

Done when: sign in and out of Jellyfin on Windows, and the app restarts signed in.

## M2: First usable version: Jellyfin browse and play (Windows)
- [ ] `current-jellyfin`: libraries, items (paging, sort, filter), latest, resume, next up, series/seasons/episodes, details, similar,
      search, images, played state, favorites
- [ ] Home, library grids, detail pages, search
- [ ] `current-playback`: device profile from the engine's capabilities, route planner (direct play, direct stream, remux, transcode),
      playback controller (resume, progress reporting, tracks, quality, next episode, intro skip, fallback on engine errors)
- [ ] The libmpv engine and the Windows player UI, media keys and the media flyout
- [ ] Playback information (route, codecs, reasons for the choice) and remembered track choices

Done when: a Jellyfin library is browsable and playable on Windows every day, with resume and progress synced to the server.

## M3: macOS
- [ ] The M1-M2 screens in SwiftUI on the shared core; AVPlayer plus the fallback engine from D7; Keychain; Now Playing, PiP, AirPlay

Done when: everything from M1-M2 works on a Mac.

## M4: Seerr: discover and request (Windows, macOS)
- [ ] Seerr sign-in through the app's Jellyfin session; discover rows, browse, search merged with library results; title pages joined
      with Jellyfin; requests, my requests, approvals for managers; watchlist, issues

Done when: a title can be found, requested and followed to "available", then played, without leaving the app.

## M5: Linux
- [ ] The M1-M4 screens in GTK4 + libadwaita; the libmpv engine from M2; Secret Service; MPRIS media controls

## M6: Adult content
- [ ] Hidden unless a server offers it and the user may see it; access rules enforced in the core. Details are kept privately while the
      repo is public.

## Later
- [ ] Android (Compose, Media3 plus a fallback engine), iOS and iPadOS (SwiftUI from macOS), tvOS, the web (the core on WebAssembly)
- [ ] Local files, downloads and offline playback; several servers and user switching; casting and watch-together
- Not planned yet (ask to add): music, live TV, books, photos, other media servers.
