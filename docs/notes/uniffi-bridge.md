# UniFFI bridge spike (D1)

Notes from the D1 spike. The decision is ADR 0004; this file has the evidence, code sketches, build steps and pitfalls the real
`current-ffi` and app shells (D5) need. Dates are when a source was read.

## Status: what was built, tested and run
| What | Where | Result |
|---|---|---|
| Rust bridge crate, 17 unit tests (value, list diffs, lag recovery, cancellation, abandoned and dropped awaits, listener release) | Linux container and CI (Windows, macOS) | Pass |
| `cargo fmt`, `cargo clippy -D warnings` on the spike workspace | Linux container, CI macOS | Pass |
| C# bindings generated with uniffi-bindgen-cs | Linux container (inspected), CI Windows | Generated |
| WinUI 3 app built and published (x64, self-contained, unpackaged) | CI `windows-latest` only | Built (CI run 39724206770, after one fix: see below) |
| Swift bindings generated, XCFramework made, SwiftUI app built in Swift 6 mode | CI `macos-latest` only | Built on the first try (Swift 6.3.3, no warnings, no extra linker flags) |
| Either app **run** | Nobody | Not verified: the owner runs both by hand (below) |

So the threading, ordering and release behaviour on the C# and Swift sides is shown in code and compiled, but nobody has watched it
run yet. The Rust side of the same behaviour is tested.

The prototype lived in `spikes/uniffi-bridge/` and was deleted before merge. The prototype as CI built it is commit
`1df3660f1cb9189fe74c1a82b492664d40de693e` on branch `spike/uniffi-bridge` (CI run 39724206770, all jobs green). It is unchanged
until the commit that deletes it. To get it back, run `git checkout 1df3660 -- spikes .github/workflows/ci.yml`.

## Versions and generators
| Piece | Version | How it is pinned |
|---|---|---|
| `uniffi` crate | `=0.31.0` | uniffi-bindgen-cs `v0.11.1+v0.31.0` declares `uniffi = "0.31.0"` and its `Cargo.lock` resolves 0.31.0 (read 2026-10-22). 0.31.1 and 0.31.2 exist, but we do not use them, so that we match the generator exactly. **Upgrades follow uniffi-bindgen-cs releases:** bump both in one PR. |
| uniffi-bindgen-cs | `v0.11.1+v0.31.0` (commit `7b1aaae1`) | `cargo install --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.1+v0.31.0 uniffi-bindgen-cs`. `--locked` uses its own lockfile. It takes about 3 minutes in CI and is uncached. |
| Swift generator | same as `uniffi` | A 1-line `uniffi-bindgen` binary in our workspace: `uniffi = { features = ["cli", "cargo-metadata"] }`, `fn main() { uniffi::uniffi_bindgen_main() }`. |
| tokio | `=1.54.1` | Features `macros`, `rt-multi-thread`, `sync`, `time` |
| .NET SDK | 10.0 (`global.json` `10.0.100`, `rollForward: latestFeature`) | Preinstalled on `windows-latest` (10.0.401 in image 20261007, runner-images `Windows2025-VS2026-Readme.md`, read 2026-10-22), so there is no setup step. |
| Windows App SDK | `Microsoft.WindowsAppSDK` 2.5.1 | NuGet. It pulls `Microsoft.Windows.SDK.BuildTools` through `Microsoft.WindowsAppSDK.Base` 2.0.4 (nuspec read 2026-10-22), so no explicit reference is needed. |
| Xcode / Swift | Xcode 26.6 default on `macos-latest` (macOS 26 arm64), Swift tools 6.2 | runner-images `macos-26-arm64-Readme.md`, read 2026-10-22 |

Generated code is not checked in: each build regenerates it from the compiled library (`--library` mode reads the metadata UniFFI
embeds). `--no-format` is passed to uniffi-bindgen-cs because it otherwise tries `dotnet csharpier` and prints a backtrace when that
is missing. The Swift generator only warns when `swift-format` is missing.

## The pattern, with code sketches
The prototype's API used neutral names (`Core`, `Snapshot`, `Item`). Only the shapes carry over.

### One runtime, owned by the core
```rust
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn handle() -> Result<tokio::runtime::Handle, BridgeError> {
    if let Some(rt) = RUNTIME.get() { return Ok(rt.handle().clone()); }
    let rt = Builder::new_multi_thread().worker_threads(2).thread_name("core-worker").enable_time().build()
        .map_err(|e| BridgeError::Runtime { message: e.to_string() })?;
    Ok(RUNTIME.get_or_init(|| rt).handle().clone())   // a racing loser's runtime is dropped unused
}
```
- The UI object's constructor (`Core::new() -> Result<Arc<Self>, BridgeError>`) creates the runtime, so a failure is a typed error,
  not a panic.
- No `#[uniffi::export(async_runtime = "tokio")]` is needed. Async exports spawn on our runtime and only await the `JoinHandle`.
  The future UniFFI polls on the foreign thread therefore needs no tokio context. The test `ops::tests::completes` polls it from a
  plain current-thread executor.

### Task handle
```rust
#[derive(uniffi::Object)]
pub struct TaskHandle { task: Mutex<Option<JoinHandle<()>>> }
#[uniffi::export] impl TaskHandle { pub fn cancel(&self) { /* take + abort */ }  pub fn is_finished(&self) -> bool }
impl Drop for TaskHandle { fn drop(&mut self) { self.cancel() } }
```
This is the same contract as matrix-sdk-ffi's `TaskHandle`: abort on cancel and on drop. Aborting drops the task's future, which
drops the `Box<dyn Listener>`, which calls the foreign "free" callback and removes the object from the bindings' handle map.

### Observable value
```rust
#[uniffi::export(callback_interface)]
pub trait ValueListener: Send + Sync { fn on_value(&self, value: Snapshot); }

pub fn subscribe_value(&self, listener: Box<dyn ValueListener>) -> Arc<TaskHandle> {
    let mut rx = self.tx.subscribe();                     // tokio::sync::watch
    Arc::new(TaskHandle::new(self.rt.spawn(async move {
        loop {
            listener.on_value(rx.borrow_and_update().clone());   // current value first
            if rx.changed().await.is_err() { break; }
        }
    })))
}
```
`watch` delivers the latest value only, so a slow UI skips values rather than queueing them. This is right for state; it is wrong
for events, which would need a queue.

### Observable list with diffs
```rust
#[derive(uniffi::Enum)]
pub enum ItemDiff { Append { values: Vec<Item> }, Insert { index: u32, value: Item }, Set { index: u32, value: Item },
                    Remove { index: u32 }, Reset { values: Vec<Item> } }
#[uniffi::export(callback_interface)]
pub trait ItemsListener: Send + Sync { fn on_diffs(&self, diffs: Vec<ItemDiff>); }
```
- **State.** The core keeps `Mutex<Vec<Item>>` plus a `tokio::sync::broadcast` of diffs. Every change is applied and sent while the
  lock is held. A subscription takes its snapshot and its receiver under the same lock, so no diff is lost or doubled.
- **Delivery.** The subscription task first sends `[Reset(snapshot)]`. After that, each wake-up drains everything waiting into one
  batch.
- **Lag.** On `RecvError::Lagged` (the backlog was 256 in the spike), it takes a fresh snapshot and receiver and sends `Reset`.
  The UI never sees an error for falling behind. Tested in `a_listener_that_falls_behind_gets_a_reset`.
- **Order.** Indices refer to the list after the diffs before it in the batch. The UIs apply them in order. A Rust mirror applying
  500 random-ish changes matches the core (`mirror_matches_the_core_after_many_changes`).
- **Kinds.** The spike's five kinds are enough for a list. matrix-sdk's `VectorDiff` also has `PushFront`, `PopBack`, `Truncate`
  and `Clear`. Add those only if a real list needs them.

### Async call with explicit cancellation
```rust
#[derive(uniffi::Object)] pub struct CancelToken { tx: watch::Sender<bool> }   // new(), cancel(), is_cancelled()

pub async fn slow_reverse(&self, text: String, delay_ms: u64, token: Arc<CancelToken>) -> Result<String, BridgeError> {
    let mut cancelled = token.tx.subscribe();
    let mut work = AbortOnDrop(self.rt.spawn(async move {
        tokio::select! {
            () = sleep(Duration::from_millis(delay_ms)) => Ok(text.chars().rev().collect()),
            _ = cancelled.wait_for(|c| *c) => Err(BridgeError::Cancelled),
        }
    }));
    (&mut work.0).await.unwrap_or_else(|e| Err(BridgeError::Runtime { message: e.to_string() }))
}
```
What happens when the foreign side stops waiting:

| Foreign side does | Rust future | Rust work | Evidence |
|---|---|---|---|
| Swift `Task.cancel()` without the token | still polled until ready | runs to the end | Swift `Async.swift`: `uniffiRustCallAsync` loops `pollFunc` until `UNIFFI_RUST_FUTURE_POLL_READY`, frees in `defer` (uniffi_bindgen 0.31.0) |
| C# stops awaiting (`WaitAsync(ct)`, abandoned `Task`) | still polled until ready | runs to the end | uniffi-bindgen-cs `_UniFFIAsync.UniffiRustCallAsync`: `PollFuture` loop, `freeFunc` in `finally`; the generated method has no `CancellationToken` |
| Future dropped (Rust callers; Kotlin's cancel is said to do this, not checked) | dropped | aborted by `AbortOnDrop` | test `dropping_the_future_aborts_the_work` |
| Token cancelled | completes with `Cancelled` | stops at once | tests `cancel_ends_the_call_early`, `abandoned_await_keeps_running_until_its_token_is_cancelled` |

UniFFI says so itself (manual 0.31.0, `futures.md`, read 2026-10-22): "We don't directly support cancellation in UniFFI even when
the underlying platforms do. You should build your cancellation in a separate, library specific channel".

**Rule for the real code:** every cancellable async export takes a `CancelToken`, and every call from a UI goes through a wrapper
that cancels it:
```swift
extension Core {
    func slowReverse(text: String, delayMs: UInt64) async throws -> String {
        let token = CancelToken()
        do {
            return try await withTaskCancellationHandler {
                try await slowReverse(text: text, delayMs: delayMs, token: token)
            } onCancel: { token.cancel() }
        } catch BridgeError.Cancelled where Task.isCancelled { throw CancellationError() }
    }
}
```
```csharp
public static async Task<string> SlowReverseAsync(this Core core, string text, ulong delayMs, CancellationToken ct)
{
    using var token = new CancelToken();
    using var registration = ct.Register(token.Cancel);
    try { return await core.SlowReverse(text, delayMs, token); }
    catch (BridgeException.Cancelled) when (ct.IsCancellationRequested) { throw new OperationCanceledException(ct); }
}
```
Calls that are quick and not cancellable need no token. `current-ffi` should generate or hand-write these wrappers once per
export; D5 decides which.

### Typed errors
```rust
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum BridgeError { Cancelled, InvalidInput { reason: String }, Runtime { message: String } }
```
- **C#.** It becomes `class BridgeException : UniffiException` with nested `BridgeException.Cancelled`,
  `BridgeException.InvalidInput` (public field `reason`) and so on. Catch by variant.
- **Swift.** It becomes `enum BridgeError: Swift.Error, Equatable, Hashable, LocalizedError` with cases `Cancelled` and
  `InvalidInput(reason:)`. Swift's `throws` is untyped, so callers match the cases.
- **Display.** `uniffi::Error` needs `Display` + `Error`. The spike wrote them by hand; the real code uses thiserror (ADR 0002).

## Threading rules
**Rust side**
- Every listener callback runs on a core worker thread (`core-worker`, tested in `sends_current_value_then_changes_on_a_worker_thread`),
  never on the UI thread. A callback blocks that worker while it runs, so it must only enqueue and return.
- A listener's callbacks are ordered (one task per subscription). Different subscriptions are not ordered against each other.
- `TaskHandle::cancel()` stops the task at its next await. A callback already running on another worker may still finish, so the UI
  must tolerate one late delivery (the weak reference below makes that harmless).

**C# (WinUI 3)**
- **Callback thread.** Callbacks arrive on a native thread the CLR has never seen. It shows as a fresh managed thread id with
  `DispatcherQueue.HasThreadAccess == false`, and the app displays both.
- **The hop.** `DispatcherQueue.TryEnqueue(() => vm.Apply(...))` runs the work on the UI thread, which `Apply` checks with
  `Debug.Assert(_ui.HasThreadAccess)`. Enqueued work with the same priority runs in order (not verified against a documented
  guarantee; the app's batch counter and list would show a violation). `TryEnqueue` returns false only when the queue is shutting
  down.
- **Async calls.** `await core.SlowReverse(...)` resumes on the UI thread, because `await` captures the window's
  `SynchronizationContext`. The generated code completes its `TaskCompletionSource` with `RunContinuationsAsynchronously`.
- **Exceptions.** Callbacks catch every exception: see "Pitfalls".

**Swift**
- **Protocols.** The generated listener protocols are `AnyObject, Sendable`. Objects are `open class …: @unchecked Sendable`.
  Records, enums and errors get `extension …: Sendable {}`. The handle maps are `@unchecked Sendable` with a lock.
- **Swift 6 friction.** None in the generated code: it has no global `var`, only `let`. It compiled with no warnings in Swift 6
  language mode (`swift-tools-version: 6.2`, Swift 6.3.3 in CI). The only friction is on our side: listeners must be `Sendable`
  (next point).
- **Listeners.** A listener must be a `final class` whose stored properties are all `let` and `Sendable`. A `weak var` to the model
  is not allowed in a `Sendable` class. Hence the spike's listener holds only an `AsyncStream.Continuation`:
```swift
final class Forwarder<T: Sendable>: Sendable {
    private let continuation: AsyncStream<Delivery<T>>.Continuation
    func forward(_ p: T) { continuation.yield(Delivery(payload: p, thread: currentThreadName(), onMainThread: Thread.isMainThread)) }
    deinit { continuation.finish() }       // core dropped the listener → stream ends → pump task ends
}
@MainActor final class Subscription<T: Sendable> {
    init(subscribe: (Forwarder<T>) -> TaskHandle, apply: @escaping @MainActor @Sendable (Delivery<T>) -> Void) {
        let (stream, continuation) = AsyncStream.makeStream(of: Delivery<T>.self)
        handle = subscribe(Forwarder(continuation))
        pump = Task { @MainActor in for await d in stream { MainActor.assertIsolated(); apply(d) } }
    }
    func cancel() { handle.cancel(); pump.cancel() }
    deinit { handle.cancel(); pump.cancel() }  // both Sendable lets: allowed in a nonisolated deinit
}
```
- **Ordering.** One `AsyncStream` drained by one main-actor task keeps the order. A separate `Task { @MainActor in … }` per callback
  does not promise FIFO order across tasks, which matters for diffs.
- **`Thread.isMainThread`.** It is unavailable in async contexts in Swift 6. The spike reads it in the synchronous callback, and the
  model reads it in its synchronous `apply`.
- **Models.** Models are `@MainActor @Observable final class`. Fields that must not be observed, or that a `deinit` touches, are
  `@ObservationIgnored`.

## Lifetimes and reference cycles
**The cycle to avoid.** UI model → `TaskHandle` (a Rust `Arc`) → core task → `Box<dyn Listener>` → bindings handle map → foreign
listener → UI model. Neither GC nor ARC can see through the Rust part, so a listener that strongly holds its model keeps the model
alive forever. The model's `deinit`/finaliser, which would cancel the handle, then never runs.

**Rules**
1. **The listener never holds its model strongly.** C# uses `WeakReference<TViewModel>`. Swift holds only a stream continuation
   and the pump closure captures `[weak self]`.
2. **The listener never captures the core object.** Calling a `Core` method from a callback would keep `Core` alive from its own
   task. The spike made `current_thread_name()` a free function for that reason.
3. **The view cancels its handles explicitly when it goes away.** WinUI cancels on window `Closed` (D5: on page `Unloaded` or
   navigation away). SwiftUI cancels on the model's owner going away or `onDisappear`. C# also calls `Dispose()` on the handle, so
   the Rust `Arc` is released now and not at the next GC. `deinit` and finalisers remain the backstop: dropping a handle cancels it.
4. **Background jobs the core starts on its own** (the ticker, the feed) belong to the core object and stop in its `Drop`.

**How it was checked**
- **Rust.** Tests use a listener that holds a drop counter. `cancel_releases_the_listener`,
  `dropping_the_future_aborts_the_work`, `dropping_the_handle_releases_the_listener` and
  `feed_drives_changes_from_a_core_task` show the core drops the foreign listener when the handle is cancelled or dropped.
- **Both apps.** `Core::live_listeners()` counts listeners the core still holds, and the apps show it (polled every 250 ms). Turning
  off the "Subscribed" switch must take it from 2 to 1. This is not run yet: it is one of the owner's checks below.
- **Not done.** No memory profiler run (dotMemory, Instruments). The weak-reference design means the model cannot be kept alive by
  the core, but nobody has measured it.

## Build steps per platform
The steps lived in `spikes/uniffi-bridge/windows/build.ps1` and `macos/build.sh`. CI ran those same scripts.

**Windows (C#)**
1. Build the library with `cargo build --release --locked -p bridge-spike --target x86_64-pc-windows-msvc`. This produces
   `bridge_spike.dll` (a cdylib).
2. Generate the bindings with `uniffi-bindgen-cs --library --no-format target\x86_64-pc-windows-msvc\release\bridge_spike.dll
   --out-dir windows\Generated`. This writes one `bridge_spike.cs`, namespace `uniffi.bridge_spike`, with every type `internal`.
3. Publish with `dotnet publish BridgeSpike.csproj -c Release -r win-x64 -p:Platform=x64 -p:BridgeLibDir=<dir of the dll>`.
   The project file:
   - targets `net10.0-windows10.0.26100.0`;
   - sets `UseWinUI`, `WindowsPackageType=None`, `WindowsAppSDKSelfContained=true`, `SelfContained=true` and
     `AllowUnsafeBlocks=true` (the generated code uses `unsafe`);
   - copies the DLL next to the exe as a `None` item with `CopyToOutputDirectory` and `CopyToPublishDirectory`;
   - picks up `Generated\*.cs` through the SDK's default glob (an explicit `Compile` item would duplicate it).
4. The app is at `windows\bin\x64\Release\net10.0-windows10.0.26100.0\win-x64\publish\BridgeSpike.exe`.

On ARM64, use `-Platform ARM64` (target `aarch64-pc-windows-msvc`, `win-arm64`). This is not built in CI.

**macOS (Swift)**
1. Build the library with `cargo build --release --locked -p bridge-spike --target aarch64-apple-darwin`. This produces
   `libbridge_spike.a` (staticlib, linked into the app) and the `.dylib` (used only by the generator).
2. Generate the bindings with `cargo run --release --locked -p uniffi-bindgen -- generate --library <dylib> --language swift
   --out-dir Generated/raw`. This writes `bridge_spike.swift`, `bridge_spikeFFI.h` and `bridge_spikeFFI.modulemap`.
3. Copy the header, plus the module map renamed to `module.modulemap`, into `Generated/include`. Copy the Swift file into
   `Generated/swift`.
4. Make the XCFramework with `xcodebuild -create-xcframework -library libbridge_spike.a -headers Generated/include -output
   Generated/BridgeSpikeFFI.xcframework`.
5. Build with `swift build -c release --package-path macos`. The package has three targets:
   - a `binaryTarget` for the XCFramework;
   - a `target` compiling the generated Swift as module `BridgeSpikeBindings`;
   - an `executableTarget` for the SwiftUI app.
   The app sets `NSApp.setActivationPolicy(.regular)` because a bare executable is not a bundle.

**Real setup (D5 and later), not built here**
- **`current-ffi`.**
  - Its own crate in the main workspace with `crate-type = ["lib", "cdylib", "staticlib"]`.
  - The `uniffi-bindgen` binary becomes a small workspace member (or an `[[bin]]` of `current-ffi`).
  - `uniffi` goes into `[workspace.dependencies]` and deny.toml gets the MPL-2.0 exceptions for `uniffi`, `uniffi_core`,
    `uniffi_macros`, `uniffi_meta`, `uniffi_bindgen`, `uniffi_pipeline`, `uniffi_udl`, `uniffi_internal_macros` (check the licence
    list with `cargo deny` then).
- **CI.**
  - Cache the uniffi-bindgen-cs install (with a cache action, which needs approval) or install it from a release binary if one
    appears.
  - Build both architectures. On macOS, use a universal static library via `lipo`, or one XCFramework slice per platform for iOS
    and tvOS later.
  - Upload the built app as an artifact (upload-artifact needs approval).
  - Lock NuGet with `RestorePackagesWithLockFile`.
- **Xcode project.** A real `.app` needs one (bundle, Info.plist, signing). SwiftPM is enough to prove the bridge, not to ship.

## Pitfalls
- **A throwing callback panics the core.** For a callback interface method that returns nothing, UniFFI handles a foreign
  exception with `panic!("Callback interface failure: …")` (`uniffi_core` 0.31.0, `ffi_converter_traits.rs`). In a tokio task that
  ends the subscription silently. Every listener catches everything. Alternatively, make callbacks return `Result` and log.
- **Calls are never cancelled implicitly.** Abandoned awaits keep their Rust work running (see the table above).
- **Handles left to the GC.** In C#, every UniFFI object is `IDisposable` with a finaliser, so a handle that is not disposed lives
  until the next GC. Dispose handles explicitly.
- **Generated C# is `internal`.** The app's public view models must not expose generated types. The spike maps `Item` to its own
  public `ItemRow` record, because XAML `x:DataType` needs a type it can see.
- **Platform in RIDs.** WinUI projects need `-p:Platform=x64|ARM64` with `-r win-x64|win-arm64`. AnyCPU is not supported.
- **Global MSBuild properties are read-only.** A property passed with `-p:` cannot be changed inside the project. The spike derives
  `BridgeLib` (with a trailing slash) from `BridgeLibDir` instead of rewriting it.
- **Swift module map name.** UniFFI writes `bridge_spikeFFI.modulemap`. An XCFramework's headers need `module.modulemap`. Two
  XCFrameworks that both put a `module.modulemap` at the top of their headers clash in one app, so the real build ships a single
  FFI module.

## What did not work, or surprised
- uniffi-bindgen-cs prints a backtrace when it cannot run its C# formatter. `--no-format` avoids it; the output is still valid.
- **The first WinUI build failed: `Task`, `IDisposable`, `CancellationToken` not found.** The hand-written project lacked
  `<ImplicitUsings>enable</ImplicitUsings>`, which the WinUI templates set. Its knock-on XAML errors (`WMC0909: Cannot resolve
  DataType vm:ItemRow`, `WMC1111`) went away with it. The first run also showed the XAML compiler warning
  `WMC1509: No LocalAssembly parameter given during MarkupCompilePass2`; it did not stop the build.
- **The Swift side needed nothing beyond the plan.**
  - The generated Swift compiled in Swift 6 language mode with no warnings and no `nonisolated(unsafe)` or language-mode override.
  - The Rust static library needed no extra linker flags.
  - `swift --version` on `macos-latest` printed Apple Swift 6.3.3. The runner readme lists Xcode 26.6 as default; this reading is
    from the build log, not cross-checked.
- **CI cost.**
  - The NuGet restore of the Windows App SDK took about 40 s (uncached).
  - Building the `uniffi-bindgen` binary took about 2.5 min on macOS.
  - Installing uniffi-bindgen-cs took about 2.5 min on Windows.
- `unsafe_code = "forbid"` was expected to need an exception for the bridge crate. It does not: UniFFI 0.31.0's macro output passes
  (checked by adding an `unsafe {}` block, which the lint then rejected).

## What the owner runs by hand
Nothing here has been run with a display. On a Windows 11 PC and on a Mac, check out `1df3660` and run:

**Windows.** Needs Rust (rustup), the .NET 10 SDK, and
`cargo install --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.1+v0.31.0 uniffi-bindgen-cs`. Then run
`pwsh spikes/uniffi-bridge/windows/build.ps1` (add `-Platform ARM64` on an ARM PC) and start the printed `BridgeSpike.exe`.

**macOS.** Needs Rust (rustup) and Xcode 26. Run `sh spikes/uniffi-bridge/macos/build.sh`, then
`swift run -c release --package-path spikes/uniffi-bridge/macos`.

Check on each:
1. **Value.**
   - "Increment" raises the number. The thread line says the callback came on `core-worker` and was applied on the UI thread or
     main actor (UI access / main thread: true).
   - "Start ticker" counts up about 5 times a second, with "last change made on core-worker".
2. **Unsubscribing.** Turn "Subscribed" off.
   - The number stops changing while the ticker runs.
   - "listeners held by the core" drops from 2 to 1.
   - Turn it on again: it jumps to the current value and the count is back to 2.
3. **List.**
   - "Step" and "Start feed" change the list one item at a time (inserts and removals animate, not a full redraw).
   - The batch counter rises.
   - Every few steps the list resets to 3 items.
4. **Async call.**
   - "Run" shows the reversed text after 3 s.
   - "Cancel" during the wait shows "cancelled" at once, and "calls running in the core" goes back to 0.
   - "Run with empty text" shows the `InvalidInput` error.
5. **Abandoned call.** "Run and abandon" stops waiting after 0.3 s. "calls running in the core" stays at 1 for about 3 s, then
   goes to 0. On macOS, the message shows the await returned only after about 3 s.
6. **Closing.** Close the window while the ticker and feed run. The app exits cleanly, with no crash and no hang.

Report anything else (wrong thread, out-of-order list, a crash) to the manager with the step number.

## The real surface (M1 wave 3)
The M1 flows went onto the bridge in `feat/ffi-sign-in`. What it added to the pattern above, and what was learned:

- **The runtime needs I/O.** The spike's runtime enabled only time. The first request through the bridge panicked on a core worker
  thread ("A Tokio 1.x context was found, but IO is disabled"), which ended the call's task. The runtime now calls `enable_io()`
  too. The Rust flow tests caught it; the existing tests had no network call.
- **Opening is an async free function**, `open_core`, which runs `App::open` on the runtime's blocking pool. UniFFI exports it as
  Swift's `openCore(dataDir:deviceName:secrets:)` and as C#'s `CurrentFfiMethods.OpenCore(...)` (`internal static class
  <Namespace>Methods` in uniffi-bindgen-cs's `wrapper.cs`, read 2026-10-22). It has no `CancelToken`: it is short, and an abandoned
  open would leave a store half-opened.
- **Flows are objects** that the core hands out (`Core::add_server`, `AddServerFlow::sign_in`, ...). Each async action wraps
  `run_cancellable`. On `Cancelled` it also undoes the flow's state (reset, or the sign-in's own cancel), but only if no newer action
  started on the same flow since. A counter per flow tracks that (tested in `a_cancel_leaves_a_newer_probe_alone`).
- **Sign-out takes no token.** Aborting it halfway, between forgetting the user and updating the status, would leave the state
  inconsistent. It is bounded by its own 10 s timeout instead.
- **The saved-server list** is an `ObservableList` in `current-ffi`, synced from `App::saved_servers()` (a `watch` of the whole list)
  by server key. Removals, inserts and in-place changes become `Remove`, `Insert` and `Set`; a reorder becomes a `Reset`.
- **Shapes in the generated code** (uniffi-bindgen-cs `v0.11.1+v0.31.0` and uniffi 0.31.0 Swift, generated in a Linux container on
  2026-10-22):
  - A data-carrying enum is an `internal record` in C# with one nested `record` per variant. Its fields are PascalCase properties
    (`Failed(ProbeProblem Problem)`).
  - A field named like its variant gets a `Value` suffix in C#. The bridge avoids this by naming status variants
    `UnexpectedStatus { status }`.
  - Error variants keep `@field` names in C# (`CoreException.Store.@problem`).
  - Fieldless enums become plain C# `enum`s and Swift enums with lowerCamel cases (`InterfaceKind::Wifi` is `.wifi`).
  - `Option<u8>` is `byte?` in C# and `UInt8?` in Swift. Sequences are C# arrays.
- **Testing against a server.** `tools/jellyfin-stand-in` answers setup, sign-in, Quick Connect, `/Users/Me` and logout on 127.0.0.1
  with a fake account (`stand-in-user` / `not-a-real-password`).
  - Rust tests use it as a library.
  - The C# and Swift tests start its binary and read `listening <address>` from its first line. They send `revoke` and
    `quick-connect off|on` on stdin. It exits when stdin closes, so a crashed test run cannot leave it behind; test.ps1 also stops
    any leftover one.
- **Keyrings in CI.** The C# tests use Windows Credential Manager once (`KeyringKeepsTheSession`) and sign out in a `finally` so the
  entry is deleted. The macOS keychain is not tested: CI's login keychain may prompt or be locked (not verified), so the Swift tests
  use `SecretStore::Memory` only.
- **Discovery in tests.** Whether a runner may broadcast and whether anything answers depends on the machine. The tests check only
  that a search ends (`Done` or `Failed`) and that cancelling and resetting return to `Idle`. Picking a discovered server is not
  tested end to end: no test can make a server answer the broadcast.

## Sources (read 2026-10-22)
- uniffi-bindgen-cs `v0.11.1+v0.31.0`: `Cargo.toml`, `bindgen/Cargo.toml`, `Cargo.lock`; generated C# from a local install at that
  tag. https://github.com/NordSecurity/uniffi-bindgen-cs
- UniFFI 0.31.0:
  - manual `futures.md`, "Cancelling async code": https://github.com/mozilla/uniffi-rs/blob/v0.31.0/docs/manual/src/futures.md
  - crate sources `uniffi_bindgen-0.31.0/src/bindings/swift/templates/{Async,Protocol,HandleMap}.swift` and
    `uniffi_core-0.31.0/src/ffi_converter_traits.rs`
- matrix-rust-sdk `bindings/matrix-sdk-ffi/src/task_handle.rs` and `src/timeline/mod.rs` (`add_listener`, `TimelineListener`):
  https://github.com/matrix-org/matrix-rust-sdk
- GitHub runner images: https://github.com/actions/runner-images. The `README.md` maps `windows-latest` to
  `windows-2025-vs2026` and `macos-latest` to macOS 26 arm64. Used: `images/windows/Windows2025-VS2026-Readme.md` (.NET SDKs,
  Windows SDK 10.0.26100) and `images/macos/macos-26-arm64-Readme.md` (Xcode 26.6 default).
- NuGet: `microsoft.windowsappsdk` 2.5.1 and `microsoft.windowsappsdk.base` 2.0.4 nuspecs: https://api.nuget.org/v3-flatcontainer/
- crates.io index: tokio 1.54.1 latest; uniffi 0.31.0, 0.31.1, 0.31.2 published.
