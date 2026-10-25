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
- [x] "Copy diagnostics" copies an empty text until a server exists: include the app version (the core's diagnostics export) in M1.
      The core side is done (`export_diagnostics`: version, platform, settings state, then the log; PR 25); the Windows app's button
      uses it since the M1 screens (feat/windows-sign-in)
- [x] Shipping libmpv in the app (`current-mpv`) brings the Visual C++ runtime (vcruntime140, msvcp140) under Microsoft's
      redistributable terms: record them in ADR 0003 with the owner first. Decided 2026-10-25 (owner): Microsoft's installer
      (`vc_redist.x64.exe`), recorded in ADR 0003
- [ ] Installer: run `vc_redist.x64.exe` when `HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64` shows the runtime
      missing or older than the build's toolset (ADR 0003). Check whether Inno Setup can keep the per-user install
      (`PrivilegesRequired=lowest`) and elevate only the `vc_redist` run, so the UAC prompt appears only when the runtime is missing
      (Inno's `ShellExec` with the `runas` verb; not verified); that would amend ADR 0009. The CI install test cannot see a missing
      runtime (the runner has Visual Studio), so test on a clean Windows install
- [ ] The libmpv build installs unpinned MSYS2 make/diffutils at build time (build-only; docs/notes/windows-mpv-build.md)
- [x] A `cargo test --workspace` run in a cloud container once had 3 failures in one test binary (unidentified; 28 later runs passed).
      If it recurs, capture the test names and fix the flake. Very likely the refused-port race in `current-net`'s tests (seen again
      in CI on PR 29): a port one test freed to be refused was handed to another test's stand-in. Stand-ins now bind under a lock
      that a test holds while it relies on a freed port (`crates/net/tests/ports`; test/roaming-refused-port-race)
- [x] `current-ffi`'s tests: three list-subscription tests in `crates/ffi/src/tests.rs`
      (`a_list_subscription_starts_with_a_reset_and_mirrors_the_core`, `a_failing_listener_keeps_its_subscription`,
      `a_failing_list_listener_gets_a_reset_next`) once failed together in a cloud container, each on `recv_timeout(WAIT)` (5 s)
      with no value, the binary taking 6 s instead of about 2.4 s (2026-10-23, PR 29's worker; never seen in CI). Cause: the two
      failing-listener tests simulated a foreign exception with `panic!`, so the panic hook ran on the core's two worker threads,
      both at once (the tests sort first and run together), stalling every task on the runtime; the cloud container sets
      `RUST_BACKTRACE=1`, so the hook captures and prints a backtrace. Proven with a hook forced to take 6 s: the same three tests
      fail on the same lines, 10 of 10 runs, the binary taking 6.5 s. That the real hook took over 5 s is not measured, and why
      (first symbolization in a large debug test binary on a loaded host) is not verified: 70 unforced runs passed. The tests now
      unwind with `resume_unwind`, which skips the hook (test/ffi-list-subscription-timeouts)
- [x] A foreign exception from a C# or Swift listener becomes a panic in UniFFI's generated code (`uniffi_core` 0.31.0,
      `ffi_converter_traits.rs`), so the panic hook runs on one of the core's two worker threads. With `RUST_BACKTRACE` set, or a
      costly custom hook added later, that could stall the core for every subscription (not verified in production). Fixed by
      delivering every listener callback on the runtime's blocking pool, one at a time per subscription, so a slow, blocked or
      panicking listener (and its hook) holds a pool thread, never a worker; the core installs no hook (PR #50, 2026-10-25;
      docs/architecture.md, "Listener delivery")
- [x] `current-ffi`'s tests after PR #50: `a_failing_listener_keeps_its_subscription` and `a_failing_list_listener_gets_a_reset_next`
      once failed together in a cloud container under full parallel load, then passed 3 of 3 alone (2026-10-25; failure output not
      kept; never seen in CI). Not reproduced unforced: 0 failures in 164 runs at e1a8dc5 and 104 at 852a55d, with
      `RUST_BACKTRACE` 0 and 1, up to 4 copies of the binary at once beside 12 CPU-burning processes on 4 CPUs. The panic hook is
      not the cause: the default hook with a backtrace took 160-300 ms on the pool thread (0.03-0.2 ms without), and the test hook
      only slows its own marker payload. Cause found for the list test (and `a_panicking_list_listener_with_a_slow_hook_gets_a_reset_next`):
      an ordering race. After the lost batch the subscription resyncs with the list as it is then; if the failed delivery ends before
      the test appends "next", the Reset holds only "lost" and "next" follows as a diff, but the test expected one Reset of both.
      Proven by forcing a 500 ms sleep between the appends: fails 1 of 1, `RUST_BACKTRACE` 0 and 1. The tests now check that a Reset
      comes first and that the mirror reaches the whole list; with a forced 2.5 s sleep they pass. The capabilities test has no race
      found: its one failure is explained only by a stall of the whole process over 5 s (`WAIT`), not proven. The same load found
      `flow_tests::removing_a_server_signs_out_and_forgets_it` failing once (`list.len()` 0): the core's saved-server list is filled on
      the runtime after sign-in, so the first batch can be `[Reset []]`, which the test took for the list. Proven with a forced 500 ms
      delay in `saved::pump` (fails 1 of 1, passes after the fix); the test now applies batches until the list holds the server, and
      until it is empty after the removal. After the fixes, 200 runs under the same load (100 at each `RUST_BACKTRACE`): the fixed
      tests passed every time; `a_blocking_listener_does_not_delay_other_subscriptions_or_tasks` failed once (`RUST_BACKTRACE=1`,
      its 1 s `PROMPT` bound on the first value), not investigated here (PR #59, 2026-10-25)
- [ ] Windows, not measured in the owner's M1 runs: the caption buttons' pressed colour against Windows' own apps (high contrast
      and Narrator passed on PR 39's run, 2026-10-24; docs/notes/windows-shell.md)
- [ ] Linux: choose how to ship (Flatpak, per-distro packages, AppImage or tarball; ADR 0011) before M5's release work
- [ ] Windows: move `CommunityToolkit.WinUI.Controls.SettingsControls` from 8.3.260402-preview2 to 8.3 stable when it ships with the
      same component-package dependency (docs/notes/windows-shell.md, "Windows Community Toolkit")
- [x] Windows: pin `Microsoft.WindowsAppSDK.WinUI` and `.InteractiveExperiences` exactly (`[x]`) like the toolkit packages (2026-10-25,
      PR 49): WinUI, InteractiveExperiences, Foundation and Base pinned at the 2.5.1 set, no version change; NU1603 no longer exempt
      from warnings-as-errors (docs/notes/windows-shell.md, Layout)
- [ ] Linux HDR: revisit when GTK enables Wayland colour management by default or mpv's render API gains HDR output (ADR 0011)

## M1: Jellyfin sign-in and server setup (Windows) (done 2026-10-24)
Each item starts with our own research (the server's API and source, the platform's documentation, how existing clients behave),
recorded with sources and dates, and a decision record where it decides something.
- [x] Server setup: add a Jellyfin server by URL, check it is the same server on every address, warn about plain HTTP except locally
      (core 2026-10-22, ADR 0012; the Windows screen, PR 27; a web app on a typed port without a scheme is reported as not
      Jellyfin, PR 28; the owner's runs on a real server, 2026-10-23)
- [x] Several addresses per server with roaming (local and internet, the user's order)
  - [x] Spike: research and design (2026-10-22, PR 17, ADR 0014, docs/notes/roaming.md; OS trust only for
        TLS in M1)
  - [x] Core: `RoamingAddresses` in `current-net`, Jellyfin's identity check, `NetworkFacts`, `App::network_changed` and the
        request log's address position (2026-10-22, ADR 0014)
  - [x] Wire the providers into signed-in servers: one provider per saved Jellyfin server for its session, restore and
        sign-in again; the capabilities state follows its status (2026-10-22, ADR 0015)
  - [x] `NetworkFacts` from the Windows UI (2026-10-23, PR 27: `NetworkObserver`, docs/notes/windows-shell.md; the owner's
        network-change case passed 2026-10-23)
- [x] Secrets in the OS keyring and a device identity per install (ADR 0013; Credential Manager checked in the owner's runs)
- [x] Sign-in with a password or Quick Connect; sessions restored at start and dropped on a 401; sign out
      (core 2026-10-22, ADR 0015; the bridge, PR 25; the Windows screens, PR 27; Narrator reads the Quick Connect code digit by
      digit, the sign-in is announced and the code can be copied, PR 32; the owner's runs 2026-10-23 and 2026-10-24)
- [x] Remove a saved server (owner, 2026-10-23): signs out every user there, deletes their tokens, forgets the server; the
      server is told in the background (PR 31, ADR 0015 amended; the owner's run 2026-10-23)
- [x] Local server discovery (core 2026-10-22, ADR 0016; the Windows screen, PR 27)
  - [x] Search from each up interface (PR 29, ADR 0016 amended; the owner's multi-adapter run passed 2026-10-23)
- [x] Windows window and page layout, matched to Windows Settings and Task Manager (owner, 2026-10-23 and 2026-10-24; done
      on the owner's run of PR 38, 2026-10-24)
  - [x] Transparent disabled back button; page columns left-aligned under the page title (PR 33; the columns are centred now)
  - [x] The title bar, the pane and the page backgrounds (PR 35): a 48 px title bar (tall caption buttons since PR 38); a 500 × 500
        minimum; the navigation view's three-stage pane with an always-shown "Open/Close Navigation" button, the pane opening
        over the page on narrower windows in the title bar's colour; an open overlay pane is a back step that Back only closes;
        the content layer on content pages and Mica behind Settings; "All rights reserved." in About
  - The owner's review of main cfa83b9 (2026-10-24), replacing the four items from the re-check of PR 35; fixed on
    fix/windows-shell-defaults (PR 38) and accepted on the owner's run (docs/notes/windows-shell.md, "Windows shell defaults:
    owner's check"). Rules: the controls' defaults first, sizes from Microsoft's guidelines, else from Microsoft's apps.
    - [x] 1. The title bar's back and pane buttons inset from the window's left edge, as the guideline and Windows Settings
    - [x] 2. The pane never hidden: the full pane or the icon rail at every width, with the pane button always shown
    - [x] 3. A narrower full pane: 300 px, Windows Settings' as the owner measured it
    - [x] 4. One content background on every page, Settings included: the control's own layer, which matches Photos
    - [x] 5. The overrides that are not needed removed (layer switch, pane colour and border, header margins, `PageLayout.cs`)
    - [x] 6. Every size from Microsoft's guidelines or apps, with its source
    - [x] 7. The pane opened over the page translucent acrylic, as in Photos; the owner keeps its 1 px border
    - [x] 8. Page content centred with a maximum width, the page title over the column
    - [x] 9. Caption buttons as tall as the title bar (48 × 48, as Photos'); back and pane buttons 40 × 36, as Windows Settings'
    - [x] 10. Title bar buttons that work while the pane is open over the page: the navigation view under the title bar
    - [x] 11. Caption button hover in the app's theme, also after a switch while the app runs
- [x] App theme in Settings, as in Photos: Personalization → "App theme", Light / Dark / Windows default (owner, 2026-10-24); saved
      in the core (ADR 0013), applied to the whole window and its caption buttons without a restart, and before the window shows
      (PR 39; docs/notes/windows-shell.md, "App theme"; accepted on the owner's run, 2026-10-24)

Done when: sign in and out of Jellyfin on Windows, and the app restarts signed in; and the owner's open layout items above are
fixed. Done: the owner accepted M1 on 2026-10-24 (PR 39's run).

## M2: First usable version: Jellyfin browse and play (Windows)
- [x] `current-jellyfin`: libraries, items (paging, sort, filter), latest, resume, next up, series/seasons/episodes, details, similar,
      search, images, played state, favorites (ADR 0018 tasks 1 and 2, PRs 53 and 54)
- [x] Home, library grids, detail pages, search (ADR 0018 tasks 3 to 5, PRs 57, 60 and 63; checked live on the owner's server,
      docs/notes/windows-shell.md "Browse: Windows run"). A grid of thousands was not checked live (the owner's largest library has
      about 90 items; the smoke run covers the stand-in's 450)
- [x] `current-playback`: device profile from the engine's capabilities, route planner (direct play, direct stream, remux, transcode),
      playback controller (resume, progress reporting, tracks, quality, next episode, intro skip, fallback on engine errors)
  - [x] `current-playback` (ADR 0019 task 2, feat/playback-core): the engine and server traits, planner and controller, tested with
        a fake engine, server and clock; wired to Jellyfin and libmpv in task 4 (PR 62)
  - [x] A stalled stream fails with a network problem after 60 s (PR 67)
- [x] The libmpv engine and the Windows player UI, media keys and the media flyout (ADR 0019 task 5, PR 66; checked live,
      docs/notes/windows-shell.md "Player: Windows run")
  - [x] `current-mpv` (ADR 0019 task 3, feat/mpv-engine): the engine over hand-written client API declarations, linked against the
        D6 build in CI's "mpv (Windows)" job, with redirect, TLS and log-redaction tests (docs/notes/mpv-engine.md)
  - [ ] Device checks not done yet (no files or hardware on the owner's run, 2026-10-25): Dolby Vision profiles 5, 7 and 8, HDR10+,
        120 Hz, remux, playback through cookie and Basic gateways, intro and credits skip (needs a segment plugin), a gateway session
        expiring mid-play, the 60 s stall timeout on a real stall, and quality remembered for internet addresses
- [x] Playback information (route, codecs, reasons for the choice) and remembered track choices (PR 66)
- [x] Servers behind an authentication gateway (after M1; ADR 0017): a password prompt for Basic gateways, a sign-in
      window that hands the gateway's cookies to the core, advanced extra headers, per address; the 401 rule amended so that a
      gateway's 401 keeps the Jellyfin session. Choices decided by the owner (2026-10-22); the player and image parts follow the
      libmpv engine and the image path
  - [x] Spike: research and design (2026-10-22, PR 24, ADR 0017, docs/notes/gateway-auth.md)
  - [x] Core (2026-10-24): gateway credentials per address and answer classification in `current-net` (PR 42), per-address
        gateway secrets in the keyring (PR 41), Jellyfin probe classification, header placement and the 400 diagnosis (PR 43),
        add-a-server gateway states, saved credentials and cookie rotation, the 401 rule (PR 44), the ffi surface and C# adapters
        (PR 45); ADRs 0006, 0012-0015 amended as ADR 0017 proposed
  - [x] Windows UI (2026-10-24, PR 47): password dialog, WebView2 sign-in window with the origin bar on a throw-away profile, extra
        headers editor, "Sign in to the gateway again"; checked live on the owner's Nginx and Cloudflare test setups (all six cases
        passed, docs/notes/windows-shell.md). A typed bare host keeps today's probe order, which may pick a direct plain-http port
        over an https gateway; typing `https://` reaches the gateway (owner, 2026-10-24)
  - [ ] Windows installer: make sure the WebView2 Runtime is present (Windows 10); today the app links to Microsoft's download
        (owner: not now, 2026-10-24)
  - [x] Images and the player carry gateway credentials (PRs 54, 62; images checked live behind each gateway in PR 63, the player
        through a header gateway in PR 66)
  - [x] Before the player bridge ships: the core checks every HLS playlist it hands to the player with gateway credentials (master
        and each variant): every URI relative or on the stream's origin, else a playback problem; `current-mpv` refuses an unchecked
        playlist with credentials (ADR 0019, "Playlists and headers", owner 2026-10-25; PR 61, cookies included since PR 62)

Done when: a Jellyfin library is browsable and playable on Windows every day, with resume and progress synced to the server.

## M3: macOS
- [ ] The M1-M2 screens in SwiftUI on the shared core; AVPlayer with our own subtitle layer (ADR 0010: libass, PGS/VobSub decoder in
      the core, External delivery from Jellyfin); Keychain; Now Playing, PiP, AirPlay
- [ ] Local discovery from each up interface: list the IPv4 interfaces with `getifaddrs` and pass them to each search (ADR 0016)
- [ ] A loopback relay in the core for streams with gateway credentials: the player talks to 127.0.0.1 without credentials and the
      relay adds them only for same-origin requests. It closes AVPlayer's header gap (ADR 0017) and enforces the same-origin rule for
      libmpv's HLS sub-requests on Windows too, replacing the playlist check (ADR 0019, "Playlists and headers", owner 2026-10-25)

Done when: everything from M1-M2 works on a Mac.

## M4: Seerr: discover and request (Windows, macOS)
- [ ] Seerr sign-in through the app's Jellyfin session; discover rows, browse, search merged with library results; title pages joined
      with Jellyfin; requests, my requests, approvals for managers; watchlist, issues

Done when: a title can be found, requested and followed to "available", then played, without leaving the app.

## M5: Linux
- [ ] The M1-M4 screens in GTK4 + libadwaita; the libmpv engine from M2 through the OpenGL render API (SDR); Secret Service; MPRIS
      media controls (ADR 0011)
- [ ] Our LGPL-2.1 libmpv and FFmpeg for Linux (the distributions' builds are GPL; ADR 0003, ADR 0011), and the Linux CI job
- [ ] Several player windows at once: libmpv engines share FFmpeg's log through the first mpv instance only (ADR 0019, 2026-10-25), so
      `current-mpv` runs one engine per process. For several players: one redactor for every engine's secrets, and a closed engine
      kept alive (stopped) while another runs, so FFmpeg's log never falls back to stderr
- [ ] The engine's redirect refusal needs FFmpeg 9.0 or later (`max_redirects` is not in FFmpeg 8.0's `http.c`); a system libmpv on an
      older FFmpeg follows redirects with the gateway headers (seen with Ubuntu's FFmpeg 6.1, docs/notes/mpv-engine.md). Use our build or
      check the FFmpeg version before shipping on Linux

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

## Later
- [ ] Generated subtitles for videos without any (owner, 2026-10-25): speech to text on the device, with translation into English
      (for example Japanese audio to English subtitles). The first candidate is Whisper through whisper.cpp: MIT code and weights;
      Whisper's `translate` task targets English only, and its turbo model does not translate; models are 1.5 GiB (medium) to 2.9 GiB
      (large); GPU backends include Vulkan and CUDA on Windows and Metal on macOS (github.com/openai/whisper and
      github.com/ggml-org/whisper.cpp, read 2026-10-25). The core extracts the audio, generates ahead of the playhead, caches the
      result and loads it as an added subtitle; for Jellyfin it may upload the result as an external subtitle so every client gets it
      (the SDKs list an `uploadSubtitle` operation; its route is not verified). Other target languages need a second translation
      model. Before generating, offer the server's own subtitles: Jellyfin's remote subtitle search, and Stash's captions (external
      SRT/VTT files beside the scene; GraphQL `Scene.captions` and `paths.caption`, served as WebVTT; docs.stashapp.cc "Captions" and
      stashapp/stash `internal/api/routes_scene.go`, read 2026-10-25). Start with a spike: speed and quality of medium and large on
      the owner's GPU with one Japanese file, ending in an ADR; the dependency and the model download need the owner's approval.
      Models are not shipped in the app: downloaded on first use after the user picks a size, checked against a pinned checksum,
      kept in the data folder and deleted from Settings → Storage. whisper.cpp's files: medium 1.53 GB (539 MB as q5_0), large-v3
      3.1 GB (1.08 GB as q5_0); the quality cost of q5_0 on Japanese is for the spike (huggingface.co/ggerganov/whisper.cpp, read
      2026-10-25). On Apple platforms, Apple-hosted asset packs (200 GB per app record, App Store Connect Help, read 2026-10-25) are
      an alternative; whether the App Store guidelines treat model weights as allowed downloaded data is not verified
- [ ] Android (Compose, Media3 plus a fallback engine), iOS and iPadOS (SwiftUI from macOS), tvOS, the web (the core on WebAssembly)
- [ ] Local files, downloads and offline playback; several servers and user switching; casting and watch-together
- Not planned yet (ask to add): music, live TV, books, photos, other media servers.
