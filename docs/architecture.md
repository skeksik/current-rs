# Architecture

> Status (2026-10-22): the workspace holds `current-model` (ADR 0005), `current-net` (ADR 0006), `current-app` and `current-ffi`
> (ADR 0004), with the C# and Swift bindings in `bindings/`. Everything else here is the target (ADR 0001, ADR 0002); details are settled
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

- **`current-app`** is the composition root. `App` is created once per process and owns the shared request log and the capabilities
  state (a `watch` value, empty until server setup exists). `ObservableList` is the core-side list: changes and subscriptions under one
  lock, `Reset` first and after lag. It has no runtime of its own.
- **`current-ffi`** is the UniFFI surface. It owns the one tokio runtime (`core-worker` threads) and exports `Core`, `TaskHandle`
  (cancel and abort on drop), `CancelToken` and one error enum, `CoreError`. Values are pumped from `watch` channels to listeners, lists
  as diff batches (`list_diff!` declares the per-item diff enum). `Core::live_listeners` counts the listeners the core holds, for leak
  checks.
- **`bindings/csharp`** (`Current.Bindings`, net10.0-windows): the generated C# (internal) plus public adapters: `CoreHost`,
  `ValueSubscription<T>` (weak listener, catch-all, hop through `IUiDispatcher`), `DiffObservableCollection<T>`, and cancellable calls
  that take a `CancellationToken`. The WinUI app implements `IUiDispatcher` over `DispatcherQueue`.
- **`bindings/swift`** (SwiftPM, Swift 6): the XCFramework and generated Swift plus adapters: `@MainActor @Observable`
  `ObservedValue` and `ObservedList` pumped by one `AsyncStream` each, and `withCoreCancellation`.
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
| macOS | AVPlayer first; fallback engine from D7 | `AVPlayerLayer` / sample-buffer layer |
| Linux | libmpv | `GtkGLArea` (D8) |
