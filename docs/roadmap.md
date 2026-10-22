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
- [x] D5a (2026-10-22, PR 8): `current-app` and `current-ffi` (ADR 0004): the composition root, app info, the capabilities state and diagnostics over
      the bridge; C# and Swift bindings with their adapters, built in CI
- [x] D5b (2026-10-22, PR 12, ADR 0009): Windows app shell and packaging: the WinUI app over `current-ffi`, navigation, settings window, unpackaged self-contained build,
      per-user install in `%LOCALAPPDATA%` (program folder never the data folder), an AppUserModelID so the media flyout names the app,
      installer tested in CI; the bridge checks D1 left unrun
- [x] D6 (2026-10-22, PR 9): our LGPL-2.1 FFmpeg and libmpv builds for Windows in CI (ADR 0003, ADR 0007), with the codec list the player needs; no
      LAME; uchardet allowed (owner, 2026-10-22)
- [x] D7 spike (2026-10-22, PR 14, ADR 0010): macOS subtitle renderer over AVPlayer (ADR 0008): styled ASS through libass and PGS/VobSub bitmaps, fed by subtitle
      streams the server extracts. A full fallback engine waits for local and offline playback
- [x] D8 spike (2026-10-22, PR 15, ADR 0011): Linux. A GTK4 + libadwaita window on the core; libmpv in a `GtkGLArea` (SDR) and the
      state of HDR on Linux. Run headless on software GL in a container; hands-on checks in WSL2 (WSLg) are listed in
      docs/notes/linux-gtk.md. HDR, hardware decoding, MPRIS, the keyring and Orca need native Linux later

## Follow-ups from phase D
- [ ] "Copy diagnostics" copies an empty text until a server exists: include the app version (the core's diagnostics export) in M1
- [ ] Shipping libmpv in the app (`current-mpv`) brings the Visual C++ runtime (vcruntime140, msvcp140) under Microsoft's
      redistributable terms: record them in ADR 0003 with the owner first
- [ ] The libmpv build installs unpinned MSYS2 make/diffutils at build time (build-only; docs/notes/windows-mpv-build.md)
- [ ] A `cargo test --workspace` run in a cloud container once had 3 failures in one test binary (unidentified; 28 later runs passed).
      If it recurs, capture the test names and fix the flake
- [ ] Linux: choose how to ship (Flatpak, per-distro packages, AppImage or tarball; ADR 0011) before M5's release work
- [ ] Linux HDR: revisit when GTK enables Wayland colour management by default or mpv's render API gains HDR output (ADR 0011)

## M1: Jellyfin sign-in and server setup (Windows)
Each item starts with our own research (the server's API and source, the platform's documentation, how existing clients behave),
recorded with sources and dates, and a decision record where it decides something.
- [ ] Server setup: add a Jellyfin server by URL, check it is the same server on every address, warn about plain HTTP except locally
      (core done 2026-10-22, ADR 0012: address parsing, probe, identity, plain-HTTP rule, the in-memory flow; the Windows screen and
      storing the server are still to do)
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
- [ ] The M1-M2 screens in SwiftUI on the shared core; AVPlayer with our own subtitle layer (ADR 0010: libass, PGS/VobSub decoder in
      the core, External delivery from Jellyfin); Keychain; Now Playing, PiP, AirPlay

Done when: everything from M1-M2 works on a Mac.

## M4: Seerr: discover and request (Windows, macOS)
- [ ] Seerr sign-in through the app's Jellyfin session; discover rows, browse, search merged with library results; title pages joined
      with Jellyfin; requests, my requests, approvals for managers; watchlist, issues

Done when: a title can be found, requested and followed to "available", then played, without leaving the app.

## M5: Linux
- [ ] The M1-M4 screens in GTK4 + libadwaita; the libmpv engine from M2 through the OpenGL render API (SDR); Secret Service; MPRIS
      media controls (ADR 0011)
- [ ] Our LGPL-2.1 libmpv and FFmpeg for Linux (the distributions' builds are GPL; ADR 0003, ADR 0011), and the Linux CI job

## M6: Adult content
- [ ] Hidden unless a server offers it and the user may see it; access rules enforced in the core. Details are kept privately while the
      repo is public.

## Before the first distribution (to anyone, testers included)
Nothing leaves the owner's machines until these are done (ADR 0003, ADR 0009 when merged):
- [ ] End-user terms (EULA) that protect Microsoft at least as much as the Windows App SDK licence does (its section 3(b)(ii)); the
      indemnity in 3(b)(iii) accepted by the owner; the installer shows the terms
- [ ] What the Windows App SDK sends to Microsoft (its section 2(a)) checked, and turned off where possible
- [ ] Code signing for Windows (SmartScreen; the Microsoft Store's installer route requires it) and Apple (Developer ID)
- [ ] Inno Setup commercial licence: requested, not required, from commercial users above USD 5,000 a year in revenue
- [ ] Each LGPL library's source and build scripts published with the release; licence texts and notices shipped (ADR 0003)
- [ ] Store policies checked before any store submission (adult content, M6)

- [ ] Android (Compose, Media3 plus a fallback engine), iOS and iPadOS (SwiftUI from macOS), tvOS, the web (the core on WebAssembly)
- [ ] Local files, downloads and offline playback; several servers and user switching; casting and watch-together
- Not planned yet (ask to add): music, live TV, books, photos, other media servers.
