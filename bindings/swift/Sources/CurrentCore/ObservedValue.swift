// Observable values from the core (ADR 0004): the core calls a listener on its own worker thread; the listener only forwards into an
// AsyncStream, and one main-actor task drains the stream in order into an @Observable model.

import CurrentFFIBindings
import Observation

/// The listener the core holds. It keeps only a stream continuation, never the model or the core, so the core cannot keep the model
/// alive. Swift callbacks cannot throw, so nothing can escape into the core.
public final class ValueForwarder<Value: Sendable>: Sendable {
    private let continuation: AsyncStream<Value>.Continuation

    init(_ continuation: AsyncStream<Value>.Continuation) {
        self.continuation = continuation
    }

    /// Called on a core worker thread: hands the value on and returns.
    public func forward(_ value: Value) {
        continuation.yield(value)
    }

    // The core dropped the listener: end the stream, which ends the pump task.
    deinit { continuation.finish() }
}

extension ValueForwarder: CapabilitiesListener where Value == CapabilitiesSnapshot {
    public func onValue(value: CapabilitiesSnapshot) {
        forward(value)
    }
}

extension ValueForwarder: AppThemeListener where Value == AppTheme {
    public func onValue(value: AppTheme) {
        forward(value)
    }
}

extension ValueForwarder: AddServerStateListener where Value == AddServerState {
    public func onValue(value: AddServerState) {
        forward(value)
    }
}

extension ValueForwarder: DiscoveryStateListener where Value == DiscoveryState {
    public func onValue(value: DiscoveryState) {
        forward(value)
    }
}

extension ValueForwarder: SignInStateListener where Value == SignInState {
    public func onValue(value: SignInState) {
        forward(value)
    }
}

/// A value the core owns, mirrored on the main actor. The owner (a view or its model) calls `cancel()` when it goes away; `deinit` is
/// only the backstop.
@MainActor
@Observable
public final class ObservedValue<Value: Sendable> {
    public private(set) var value: Value

    private let handle: TaskHandle
    private let pump: Task<Void, Never>

    /// - Parameters:
    ///   - initial: shown until the core's first delivery, which comes at once.
    ///   - subscribe: subscribes the forwarder, e.g. `{ core.subscribeCapabilities(listener: $0) }`.
    public init(initial: Value, subscribe: (ValueForwarder<Value>) -> TaskHandle) {
        value = initial
        // Only the newest value matters, as in the core's watch channel.
        let (stream, continuation) = AsyncStream.makeStream(of: Value.self, bufferingPolicy: .bufferingNewest(1))
        let target = WeakTarget<ObservedValue<Value>>()
        handle = subscribe(ValueForwarder(continuation))
        pump = Task { @MainActor in
            for await value in stream {
                MainActor.assertIsolated()
                target.object?.value = value
            }
        }
        target.object = self
    }

    /// Stops the subscription: the core drops its listener.
    public func cancel() {
        handle.cancel()
        pump.cancel()
    }

    deinit {
        handle.cancel()
        pump.cancel()
    }
}

/// A weak reference the pump task holds, set once the model is fully initialised.
@MainActor
final class WeakTarget<Object: AnyObject> {
    weak var object: Object?
}
