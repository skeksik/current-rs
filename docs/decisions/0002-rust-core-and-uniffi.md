# 0002: Rust core, UniFFI bridge and core libraries

Status: accepted (2026-10-22). The crate list is the target layout; each crate is created by the task that first needs it. Library
versions are checked when each is added.

## Context
ADR 0001 puts all logic in a shared Rust core under native UIs in C#, Swift, Rust (GTK) and later Kotlin. The core talks to Jellyfin,
Seerr and Stash itself, roams between server addresses, stores settings and secrets, plans playback and holds each screen's state.
Options for the bridge were researched on 2026-10-22 (sources below).

## Decision

### The core does its own work
The core runs its own async runtime (tokio) and does networking, storage and timers itself. The UIs render state and send user actions;
they do not run the core's side effects. This rules out designs where the shell executes every effect (Crux).

### Bridge: UniFFI
- **UniFFI** (mozilla/uniffi-rs) with proc-macros (no UDL), one bridge crate (`crates/ffi`) that exposes the core to Swift, Kotlin and C#.
  Swift and Kotlin bindings are UniFFI's own. C# bindings come from NordSecurity's `uniffi-bindgen-cs`, which tracks UniFFI one minor
  version behind (v0.11.1 tracks UniFFI 0.31.0, 2026-10-09). **UniFFI stays on the version uniffi-bindgen-cs supports.**
- Accepted with UniFFI's MPL-2.0 licence (ADR 0003).
- **Observable state** follows matrix-rust-sdk's pattern (`bindings/matrix-sdk-ffi`, used by Element X): a method takes a listener (a
  foreign trait implemented in Swift, C# or Kotlin) and returns a task handle the UI keeps and cancels; list changes travel as diffs
  (append, insert, set, remove, reset) rather than whole lists. Each UI wraps these into its own observable type (`@Observable` in
  Swift, `INotifyPropertyChanged`/`ObservableCollection` in C#, `StateFlow` in Kotlin). The exact shape is decided by the bridge spike
  (docs/roadmap.md, D1).
- Async Rust functions become Swift `async`, Kotlin `suspend` and C# `Task`. Cancellation is explicit (UniFFI has no built-in
  cancellation of a foreign await).
- **Linux uses the core directly** from Rust (gtk4-rs); it needs no bridge.

### Crate layout (target)
Dependencies point down only. UI-facing crates never import a UI toolkit, except the Linux app crate.

| Crate | Responsibility |
|---|---|
| `current-model` | Domain types: identity (server + id), open values for services, media kinds, permissions; artwork; users; capabilities. Pure, no I/O |
| `current-net` | HTTP client per server that follows its current address, error model, redacted request log, roaming between addresses, server probes |
| `current-store` | Settings, secrets (OS keyring), device identity, cache (SQLite) |
| `current-jellyfin`, `current-seerr`, `current-stash` | One client per service: internal wire types, mapping to `current-model` |
| `current-playback` | Device profile, route planner, the headless engine trait, playback controller |
| `current-mpv` | libmpv engine (Windows and Linux, macOS fallback if chosen) |
| `current-app` | Presentation: one state object per screen or feature, the capabilities state, the app's composition root |
| `current-ffi` | The UniFFI surface over `current-app` and the engines; the only crate built as a library for C#, Swift and Kotlin |
| `apps/linux` | The GTK4 app (Rust) |

`unsafe_code` stays forbidden workspace-wide; only crates that must call C (`current-mpv`, `current-ffi`) may allow it, each with a
comment saying why.

### Core libraries (candidates; each added with the owner's approval when first needed)
| Need | Library | Licence |
|---|---|---|
| Async runtime | tokio | MIT |
| HTTP | reqwest with rustls (crypto provider chosen in the networking task) | MIT/Apache-2.0 |
| JSON | serde, serde_json | MIT/Apache-2.0 |
| Errors | thiserror | MIT/Apache-2.0 |
| Logging | tracing | MIT |
| Cache | rusqlite with bundled SQLite (SQLite is public domain) | MIT |
| Secrets | keyring 4 with its per-platform stores (Windows Credential Manager, Apple Keychain, Secret Service, Android) | MIT/Apache-2.0 |
| Stash GraphQL | graphql_client (cynic is MPL-2.0) | MIT/Apache-2.0 |
| URLs, ids, time | url, uuid, time | MIT/Apache-2.0 |

### Service API types
Wire types are hand-written for the endpoints we use and checked against each server's published spec: Jellyfin's OpenAPI
(`api.jellyfin.org/openapi/`), Seerr's `seerr-api.yml` at its repo root, Stash's `graphql/schema/`. No mature Rust Jellyfin crate exists
for 12.x, and generating all of Jellyfin's 357 schemas costs compile time for little gain. Jellyfin 12.0 (2026-09) broke parts of the API
from 10.11; we support the latest server releases only.

### Portability
The core must stay buildable for Android, iOS, tvOS and later WebAssembly. Platform-specific code sits behind traits with per-platform
implementations. tokio on Wasm only supports a single-threaded subset, so code that would block a thread (rusqlite, `spawn_blocking`)
stays behind those traits too. A web target is not built yet.

## Alternatives considered
- **BoltFFI** (MIT, generates Swift, Kotlin, C#, TypeScript; used by Crux): permissive and first-party C#, but very new, with no
  production track record checked. Revisit if uniffi-bindgen-cs stalls.
- **Crux** (Apache-2.0): its core must stay pure, with HTTP run by the shell; conflicts with a core that does its own networking.
- **Interoptopus** (C#-first, async, callbacks) or a hand-written C ABI with cbindgen: C# only, so Swift and Kotlin would need a second
  bridge.
- **swift-bridge**: Swift only.

## Sources (read 2026-10-22)
- UniFFI 0.32.2 (2026-09-23), MPL-2.0: https://crates.io/crates/uniffi, https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md,
  https://mozilla.github.io/uniffi-rs/latest/futures.html
- uniffi-bindgen-cs v0.11.1+v0.31.0 (2026-10-09): https://github.com/NordSecurity/uniffi-bindgen-cs
- matrix-rust-sdk FFI listeners and task handles: https://github.com/matrix-org/matrix-rust-sdk/tree/main/bindings/matrix-sdk-ffi
- Crux: https://github.com/redbadger/crux; BoltFFI: https://github.com/boltffi/boltffi
- Jellyfin OpenAPI: https://api.jellyfin.org/openapi/; Seerr: https://github.com/seerr-team/seerr/blob/develop/seerr-api.yml; Stash:
  https://github.com/stashapp/stash/tree/develop/graphql/schema
- tokio on Wasm: https://github.com/tokio-rs/tokio/blob/master/tokio/src/lib.rs
