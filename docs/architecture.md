# Architecture

> Status (2026-10-22): the workspace holds `current-model` (ADR 0005), `current-net` (ADR 0006), `current-store` (ADR 0013),
> `current-jellyfin` (server setup, ADR 0012; sign-in, ADR 0015), `current-app` and `current-ffi` (ADR 0004), with the C# and Swift bindings in `bindings/`
> and the Windows app shell in `apps/windows` (ADR 0009). Everything else here is the target (ADR 0001, ADR 0002); details are settled
> by the desktop spikes (docs/roadmap.md, phase D).

## Overview
```
 Windows: WinUI 3 (C#)     macOS: SwiftUI (+AppKit)     Linux: GTK4 + libadwaita (Rust)     later: Android, iOS, tvOS, web
          |                         |                               |
          +------- current-ffi (UniFFI: C#, Swift, Kotlin) ------+  |  (Linux uses the core directly)
                                    |                               |
                              current-app (screen state, capabilities, composition root)
                                    |
      current-jellyfin  current-seerr  current-stash    current-playback    current-mpv (libmpv engine)
                                    |
                    current-net (clients, roaming, errors, request log)    current-store (settings, secrets, cache)
                                    |
                              current-model (domain types)
```
Dependencies point down only. The UIs render state from `current-app` and send user actions back; they hold no logic of their own
beyond platform integration (windows, video surfaces, system media controls, accessibility).

## Services
| Service | What Current uses it for | Signed in with |
|---|---|---|
| Jellyfin | Libraries, details, playback, watch state | Access token for a Jellyfin user |
| Seerr | Discover, the wider catalogue, requests, watchlist, issues | Session of a Seerr user |
| Stash | Scenes and their playback; watch state stays in Stash | One API key per server |

Any non-empty subset can be configured: every service is optional at runtime. Features read one capabilities state (what each
configured server offers, whether it is signed in and reachable) and hide what is unavailable. An unreachable server keeps its features,
shown with an offline state.

## State across the bridge (ADR 0002, ADR 0004)
Each screen or feature has a state object in `current-app`. A UI subscribes with a listener and gets a task handle it cancels when the
screen goes away; lists arrive as diffs. Actions are methods on the state object; long work is async.

- **`current-app`** is the composition root. `App` is created once per process and owns the shared request log, the capabilities
  state (a `watch` value, one status per saved server), the saved servers and one signed-in `JellyfinClient` per live session.
  `App::open` takes the data folder the UI passes in, a `Secrets` implementation and the device name Jellyfin shows (ADR 0015), and
  loads the settings, the device id and the saved servers (ADR 0013). Its async methods run settings and keyring calls on tokio's
  blocking pool. `ObservableList` is the core-side list: changes and subscriptions under one
  lock, `Reset` first and after lag. It has no runtime of its own.
- **`current-store`** keeps what lasts between runs (ADR 0013): `settings.json` (versioned, saved atomically, migrated step by step;
  a corrupt file is kept aside, a newer one is read-only), the `Secrets` trait over the OS keyring (Windows Credential Manager, macOS
  login keychain; elsewhere `Unavailable`, never a file fallback) and the random per-install `DeviceId`. Its API is sync and blocking;
  async callers run it on a blocking thread.
- **Server setup** (ADR 0012): `current-jellyfin` turns typed text into candidate base URLs, probes `/System/Info/Public` on each
  and identifies the server by its id; `current-net::transport` says whether plain HTTP to an address needs the user's confirmation
  (silent on loopback and the local network). `current-app`'s `AddServer` is the flow's state (`Idle`, `Probing`, `Failed`,
  `ConfirmPlainHttp`, `Ready`).
- **Sign-in** (ADR 0015): `current-jellyfin` sends `Authorization: MediaBrowser Client, Device, DeviceId, Version[, Token]` with
  percent-encoded values on every request; `JellyfinClient` signs in by password or Quick Connect, reads the current user and signs
  out. `current-app`'s `SignIn` flow (`Ready`, `SigningIn`, `QuickConnectWaiting`, `Failed`, `SignedIn`) starts from `AddServer`'s
  `Ready` or a saved server; success saves the server (`SavedServer` in `settings.json`: key, server id, addresses, users, active
  user) and the token in the keyring only. `App::restore_sessions` checks each saved session; a 401 to any
  request with the token signs the server out and deletes the token; `App::sign_out` deletes it whatever the server answers. Each
  saved server's session and sign-ins go through its one `RoamingAddresses` over all its addresses (ADR 0014). It serves an address
  only after the identity check passed there, so no token goes anywhere else, and the capabilities state follows its status.
- **`current-ffi`** is the UniFFI surface. It owns the one tokio runtime (`core-worker` threads, I/O and time enabled) and exports
  `Core`, `TaskHandle` (cancel and abort on drop), `CancelToken` and one error enum, `CoreError`. Values are pumped from `watch`
  channels to listeners, lists as diff batches (`list_diff!` declares the per-item diff enum). `Core::live_listeners` counts the
  listeners the core holds, for leak checks.
  - **Listener delivery.** Each subscription is one task on the runtime, but the task never calls the listener itself: each
    callback runs on the runtime's blocking pool (`spawn_blocking`), and the task awaits it before taking the next value or batch.
    So a subscription's callbacks are ordered and never overlap, while callbacks of different subscriptions can run at the same
    time on different threads (up to tokio's blocking-pool limit, 512 by default), not only on the two workers. A slow, blocked or
    throwing listener holds one pool thread and delays only its own subscription, never the workers every core task shares: UniFFI
    turns a foreign exception into a Rust panic (`uniffi_core` 0.31.0, `LiftReturn::handle_callback_unexpected_error`), and the
    panic hook, however costly, runs on that pool thread. The core installs no panic hook. A failing callback costs that delivery
    only; a list's next batch is then a Reset. A value listener still busy skips intermediate values. Cancelling lets a callback
    already running finish and sends nothing more. The runtime is a process-wide static that is never dropped, so a listener that
    never returns does not hold up exit (`a_stuck_listener_does_not_hold_up_exit`). Every runtime thread, pool included, is named
    `core-worker`. The C# and Swift adapters hop to the UI thread (`IUiDispatcher`, the main actor) before touching UI state.
  - `open_core(data_dir, device_name, secrets)` is async: the core opens the settings and the keyring on its own blocking pool, so a
    UI awaits it from its UI thread. `SecretStore::Keyring` in the apps, `Memory` for tests only. `Core::load_report` says what the
    store found. `Core::new` is the core that stores nothing (tests, the smoke run).
  - The M1 flows are objects: `AddServerFlow`, `DiscoveryFlow` and `SignInFlow`, each with `subscribe_state` (a value) and actions;
    long actions are async with a `CancelToken`, and a cancelled one puts its flow back unless a newer action started. A sign-in is
    cancelled through the app flow's own `cancel`, never dropped halfway; one the server already accepted is saved and wins.
    Sessions are on `Core`: `sign_in_again`, `restore_sessions` (cancelling stops the waiting; each server's check runs to its
    end), `sign_out` (not cancellable; reports whether the server heard), `network_changed`, `returned_to_foreground`. The saved servers are a diff list (`subscribe_servers`), kept in step with `App::saved_servers` by
    server key.
  - Nothing secret crosses in a state: no address, token, password, Quick Connect secret or network key. Records carry ids, names,
    versions and counts; failures are flat enums (`ProbeProblem`, `SignInProblem`, `NetProblem`, `StoreProblem`, ...) without
    server messages or redirect targets. The password goes in only as an argument, the network key only inside `NetworkFacts`,
    whose Debug, C# `ToString()` and Swift description redact it.
  - `export_diagnostics` is what "copy diagnostics" copies: the version, platform, architecture and the settings' load report, then
    the redacted request log.
- **`bindings/csharp`** (`Current.Bindings`, net10.0-windows): the generated C# (internal) plus public adapters: `CoreHost`
  (`Create`, `OpenAsync`), `AddServerFlow`, `DiscoveryFlow`, `SignInFlow`, public records for every state and problem,
  `ValueSubscription<T>` (weak listener, catch-all, hop through `IUiDispatcher`), `ListSubscription<T>` over
  `DiffObservableCollection<T>` (it re-subscribes after a lost or bad batch), cancellable calls that take a `CancellationToken`, and one public exception per `CoreError`
  variant. The WinUI app implements `IUiDispatcher` over `DispatcherQueue`. Tests run against the Jellyfin stand-in process.
- **`bindings/swift`** (SwiftPM, Swift 6): the XCFramework and generated Swift plus adapters: `@MainActor @Observable`
  `ObservedValue` and `ObservedList` pumped by one `AsyncStream` each, `observeState()`/`observeServers()` helpers, and async
  wrappers through `withCoreCancellation`. The generated records are used directly. Tests run against the stand-in process.
- **`tools/jellyfin-stand-in`**: a hand-written Jellyfin on 127.0.0.1 for the bridge tests (a library for Rust, a process for C# and
  Swift). Test code only, never shipped.
- **`apps/windows`** (`Current.Shell`, WinUI 3, unpackaged, self-contained x64): a Mica window with a TitleBar and a NavigationView
  (Home, Settings), one `CoreHost` per process opened on the data folder at start (then the network observer and session restore),
  `DispatcherQueueUi` as the `IUiDispatcher`, page-scoped subscriptions and flows (add a server with local discovery, sign in with a
  password or Quick Connect, saved servers with sign out in Settings, M1), `NetworkObserver` feeding `NetworkFacts`, an explicit
  AppUserModelID when unpackaged and a single instance through `AppInstance`. Installed per user by Inno Setup into
  `%LOCALAPPDATA%\Programs\Current`; data in `%LOCALAPPDATA%\Current` (ADR 0009, `docs/notes/windows-shell.md`).
- Generated code is never checked in: `bindings/csharp/build.ps1` and `bindings/swift/build.sh` regenerate it from the built library.

## Playback
```
Player UI (WinUI | SwiftUI | GTK)  ->  playback controller (current-playback)  ->  route planner (current-playback)
                                              |                                     |
                              engine (headless, per platform) <-- plan: direct play | direct stream | remux | transcode
```
- **Engines are headless:** they play and render to a platform surface; the controls are always our own (ADR 0001).
- **The controller** owns the plan, fallbacks, progress reporting and resume, track selection, intro skip and next episode.
- **The planner** chooses the route from the media, the engine's capabilities, the quality limit and the user's permissions; Jellyfin
  negotiates part of it from a device profile.
- **Engines per platform** (candidates until the spikes decide):

| Platform | Engine | Surface |
|---|---|---|
| Windows | libmpv (our LGPL-2.1 build), `vo=gpu-next`, D3D11 | `SwapChainPanel` (D2) |
| macOS | AVPlayer (ADR 0008), with our own subtitle layer: libass, PGS/VobSub bitmaps (ADR 0010); a fallback engine only for local playback | `AVPlayerLayer` plus a subtitle `CALayer` |
| Linux | libmpv (system library in development; our LGPL-2.1 build when distributed), OpenGL render API (`vo=libmpv`), SDR (ADR 0011) | `GtkGLArea` under GTK controls in a `GtkOverlay` |
