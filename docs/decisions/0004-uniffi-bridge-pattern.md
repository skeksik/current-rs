# 0004: How state and actions cross the UniFFI bridge

Status: accepted (2026-10-22). Decided by the D1 spike. Its details, build steps and evidence are in `docs/notes/uniffi-bridge.md`.

## Context
ADR 0002 picks UniFFI with proc-macros, C# bindings from uniffi-bindgen-cs, and matrix-rust-sdk's listener + task handle pattern
for observable state. It left the exact shape, threading, lifetimes and cancellation to this spike. The spike built a throwaway
crate plus a WinUI 3 app and a SwiftUI app over it, in CI, and tested the Rust side.

## Decision

### Versions
- `uniffi =0.31.0`, the version uniffi-bindgen-cs `v0.11.1+v0.31.0` resolves in its own `Cargo.lock`. UniFFI is upgraded only
  when a uniffi-bindgen-cs release tracks the newer version, and both move in the same PR.
- Swift bindings come from a `uniffi-bindgen` binary built inside our workspace (`uniffi` with the `cli` feature), so the
  generator is always the library's exact version. uniffi-bindgen-cs is installed with
  `cargo install --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.1+v0.31.0 uniffi-bindgen-cs`.

### The core owns one runtime
- One global multi-threaded tokio runtime, created on first use by `current-ffi` and never exposed. UniFFI async calls are polled
  by the foreign side; the Rust future they poll only awaits work spawned on our runtime, so no `async_runtime` attribute and no
  tokio context are needed on the foreign thread.

### Observable values and lists
- `subscribe_x(listener: Box<dyn XListener>) -> Arc<TaskHandle>`. The listener is a `callback_interface` trait (`Send + Sync`). The
  core calls it with the current value first, then on every change, from a core worker thread.
- `TaskHandle` aborts its task on `cancel()` and on drop. Aborting drops the listener, which frees the foreign object.
- Values use `tokio::sync::watch`: a slow UI skips intermediate values and never queues them.
- Lists are sent as batches of diffs `Append | Insert | Set | Remove | Reset`. Every subscription starts with `Reset(current)`. The
  snapshot and the subscription are taken under the same lock as every change. A subscriber that falls behind the broadcast
  backlog gets a fresh `Reset`, not an error.
- Callback methods return nothing and must never throw: UniFFI turns a foreign exception in such a callback into a Rust panic,
  which ends the subscription task. Each UI's listener catches everything.

### Async calls and cancellation
- Long or cancellable calls are `async fn` that take an explicit `Arc<CancelToken>`. On cancel they end with the typed `Cancelled`
  error.
- UniFFI does not cancel a Rust future when the foreign task is cancelled. Swift and C# keep polling it until it completes. So the
  rule is: **every cancellable call goes through a UI wrapper that cancels the token when its own task is cancelled**.
  - In Swift, `withTaskCancellationHandler { … } onCancel: { token.cancel() }`, mapping `Cancelled` to `CancellationError`.
  - In C#, `ct.Register(token.Cancel)`, mapping `Cancelled` to `OperationCanceledException`.
- If the future is dropped (a Rust caller, or Kotlin later), the spawned work is aborted too (abort on drop).

### Errors
- One `uniffi::Error` enum per surface with typed fields. It becomes a C# exception class with one nested subclass per variant,
  and a Swift `Error` enum.

### UI side
- **Threading.** Callbacks never touch UI objects directly: they hop to the UI thread first.
  - In C#, `DispatcherQueue.TryEnqueue` (in order).
  - In Swift, `AsyncStream` drained by one `@MainActor` task (in order; separate `Task { @MainActor }` per callback is not
    guaranteed to keep order).
- **Wrapping.**
  - In C#, a view model (`INotifyPropertyChanged`) or an `ObservableCollection` that applies diffs one by one.
  - In Swift, an `@MainActor @Observable` model.
- **Lifetimes.**
  - The view model owns the `TaskHandle`. The listener holds only a weak reference to the view model (C#) or only a stream
    continuation (Swift), and never captures the core object.
  - Views cancel their handles when they go away. Window close, page unload or `onDisappear` is the trigger, never the GC
    or `deinit` alone.

### Lints
- `unsafe_code = "forbid"` holds for the bridge crate: UniFFI 0.31's proc-macro output compiles under it. `current-ffi` therefore
  needs no exception (this narrows ADR 0002, which allowed one).

## Consequences
- Every observable in `current-app` is exposed in one of two shapes (value or diff list), each with a listener trait and a handle.
- Each UI keeps a small adapter layer per shape (forwarder plus model), written once per platform.
- Cancellation is an explicit API concept, tested in Rust; nothing relies on foreign task cancellation.
- Both UI toolchains build in CI from generated code that is not checked in.

## Sources (read 2026-10-22)
- uniffi-bindgen-cs `v0.11.1+v0.31.0`, `Cargo.toml` and `Cargo.lock` (uniffi 0.31.0): https://github.com/NordSecurity/uniffi-bindgen-cs
- UniFFI 0.31.0 manual, "Cancelling async code": "We don't directly support cancellation in UniFFI even when the underlying
  platforms do." https://github.com/mozilla/uniffi-rs/blob/v0.31.0/docs/manual/src/futures.md
- UniFFI 0.31.0 generated code: Swift `Async.swift` (`uniffiRustCallAsync` frees the future only after it is ready), uniffi-bindgen-cs
  `_UniFFIAsync.UniffiRustCallAsync` (same), `uniffi_core` `ffi_converter_traits.rs` (`panic!("Callback interface failure: …")`)
- matrix-rust-sdk `bindings/matrix-sdk-ffi/src/task_handle.rs` (abort on cancel and drop) and `timeline/mod.rs` (`add_listener`
  sends `Reset` first, then diffs): https://github.com/matrix-org/matrix-rust-sdk
