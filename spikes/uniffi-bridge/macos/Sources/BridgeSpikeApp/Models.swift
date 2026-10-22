import BridgeSpikeBindings
import Foundation
import Observation

/// What a listener saw: the payload plus the thread it arrived on, so the app can show the hop to the main actor.
struct Delivery<T: Sendable>: Sendable {
    let payload: T
    let thread: String
    let onMainThread: Bool
}

/// The listener the core holds. Runs on a core worker thread; it only yields into an AsyncStream, which keeps the
/// order and hands each value to the model on the main actor. It holds no reference to the model, so the core can
/// never keep the model alive. When the core drops it (handle cancelled or released), the stream finishes.
final class Forwarder<T: Sendable>: Sendable {
    private let continuation: AsyncStream<Delivery<T>>.Continuation

    init(_ continuation: AsyncStream<Delivery<T>>.Continuation) {
        self.continuation = continuation
    }

    func forward(_ payload: T) {
        continuation.yield(Delivery(payload: payload, thread: currentThreadName(), onMainThread: Thread.isMainThread))
    }

    deinit {
        continuation.finish()
    }
}

final class ValueForwarder: ValueListener {
    let inner: Forwarder<Snapshot>
    init(_ inner: Forwarder<Snapshot>) { self.inner = inner }
    func onValue(value: Snapshot) { inner.forward(value) }
}

final class ItemsForwarder: ItemsListener {
    let inner: Forwarder<[ItemDiff]>
    init(_ inner: Forwarder<[ItemDiff]>) { self.inner = inner }
    func onDiffs(diffs: [ItemDiff]) { inner.forward(diffs) }
}

/// Owns a subscription: the core's TaskHandle and the main-actor task that drains the stream.
@MainActor
final class Subscription<T: Sendable> {
    private let handle: TaskHandle
    private let pump: Task<Void, Never>

    /// `subscribe` gets the forwarder to hand to the core; `apply` runs on the main actor for each delivery.
    init(subscribe: (Forwarder<T>) -> TaskHandle, apply: @escaping @MainActor @Sendable (Delivery<T>) -> Void) {
        let (stream, continuation) = AsyncStream.makeStream(of: Delivery<T>.self)
        handle = subscribe(Forwarder(continuation))
        pump = Task { @MainActor in
            for await delivery in stream {
                MainActor.assertIsolated()
                apply(delivery)
            }
        }
    }

    func cancel() {
        handle.cancel()
        pump.cancel()
    }

    // Both are Sendable, so a nonisolated deinit may touch them. Dropping the handle cancels it too; this is explicit.
    deinit {
        handle.cancel()
        pump.cancel()
    }
}

@MainActor
@Observable
final class ValueModel {
    private(set) var count: UInt64 = 0
    private(set) var threads = ""
    private(set) var isSubscribed = false
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var subscription: Subscription<Snapshot>?

    init(core: Core) {
        self.core = core
        subscribe()
    }

    func subscribe() {
        guard subscription == nil else { return }
        // [weak self]: the closure lives in the pump task, which this model owns.
        subscription = Subscription(
            subscribe: { [core] in core.subscribeValue(listener: ValueForwarder($0)) },
            apply: { [weak self] in self?.apply($0) })
        isSubscribed = true
    }

    /// What a view does when it goes away.
    func unsubscribe() {
        subscription?.cancel()
        subscription = nil
        isSubscribed = false
    }

    func increment() { core.increment() }
    func startTicker() { core.startTicker(intervalMs: 200) }
    func stopTicker() { core.stopTicker() }

    private func apply(_ d: Delivery<Snapshot>) {
        count = d.payload.count
        threads = "callback on \(d.thread) (main thread: \(d.onMainThread)); applied on the main actor "
            + "(main thread: \(Thread.isMainThread)); last change made on \(d.payload.changedOn)"
    }
}

struct Row: Identifiable, Equatable {
    let id: UInt64
    let title: String
}

@MainActor
@Observable
final class ItemsModel {
    private(set) var rows: [Row] = []
    private(set) var batches = 0
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var subscription: Subscription<[ItemDiff]>?

    init(core: Core) {
        self.core = core
        subscription = Subscription(
            subscribe: { [core] in core.subscribeItems(listener: ItemsForwarder($0)) },
            apply: { [weak self] in self?.apply($0.payload) })
    }

    func step() { core.feedStep() }
    func startFeed() { core.startFeed(intervalMs: 300) }
    func stopFeed() { core.stopFeed() }

    private func apply(_ diffs: [ItemDiff]) {
        for diff in diffs {
            switch diff {
            case .append(let values): rows.append(contentsOf: values.map(Row.init))
            case .insert(let index, let value): rows.insert(Row(value), at: Int(index))
            case .set(let index, let value): rows[Int(index)] = Row(value)
            case .remove(let index): rows.remove(at: Int(index))
            case .reset(let values): rows = values.map(Row.init)
            }
        }
        batches += 1
    }
}

extension Row {
    init(_ item: Item) { self.init(id: item.id, title: item.title) }
}

extension Core {
    /// The Swift side of the cancellation rule (docs/notes/uniffi-bridge.md): the generated async function ignores
    /// task cancellation, so every call gets a CancelToken that the task's cancellation handler cancels.
    func slowReverse(text: String, delayMs: UInt64) async throws -> String {
        let token = CancelToken()
        do {
            return try await withTaskCancellationHandler {
                try await slowReverse(text: text, delayMs: delayMs, token: token)
            } onCancel: {
                token.cancel()
            }
        } catch BridgeError.Cancelled where Task.isCancelled {
            throw CancellationError()
        }
    }
}

@MainActor
@Observable
final class OperationModel {
    var input = "hello bridge"
    private(set) var result = ""
    private(set) var coreState = ""
    private(set) var isRunning = false
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var running: Task<Void, Never>?

    init(core: Core) {
        self.core = core
        // Polled, not observed: these counters exist only to show what the core still holds. Ends with the model.
        Task { [weak self] in
            while let self {
                self.coreState = "listeners held by the core: \(self.core.liveListeners()); "
                    + "calls running in the core: \(self.core.inFlight())"
                try? await Task.sleep(for: .milliseconds(250))
            }
        }
    }

    func run() { start(text: input) }
    func runEmpty() { start(text: "") }
    func cancel() { running?.cancel() }

    /// Cancels the Swift task without the CancelToken: the await does not end early, the Rust work runs its full delay.
    func runAndAbandon() {
        let core = core, text = input
        result = "cancelling the Swift task after 300 ms, without the token…"
        let call = Task {
            let started = ContinuousClock.now
            let value = try? await core.slowReverse(text: text, delayMs: 3000, token: CancelToken())
            return (value, ContinuousClock.now - started)
        }
        Task {
            try? await Task.sleep(for: .milliseconds(300))
            call.cancel()
            let (value, elapsed) = await call.value
            result = "task cancelled at 300 ms, await still returned \(value ?? "nil") after \(elapsed)"
        }
    }

    private func start(text: String) {
        guard running == nil else { return }
        result = "running…"
        isRunning = true
        running = Task {
            defer {
                running = nil
                isRunning = false
            }
            do {
                result = try await core.slowReverse(text: text, delayMs: 3000)
            } catch is CancellationError {
                result = "cancelled"
            } catch BridgeError.InvalidInput(let reason) {
                result = "BridgeError.InvalidInput: \(reason)"
            } catch {
                result = "\(error)"
            }
        }
    }
}
