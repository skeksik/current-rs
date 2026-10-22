// Cancellation (ADR 0004): UniFFI does not cancel a Rust future when a Swift task is cancelled, so every cancellable call goes
// through this wrapper, which cancels the call's CancelToken when the task is cancelled.

import CurrentFFIBindings

/// Runs a cancellable core call with a fresh token. Cancelling the current task cancels the token; the core's `Cancelled` error then
/// becomes `CancellationError`.
public func withCoreCancellation<T: Sendable>(
    _ operation: @Sendable (CancelToken) async throws -> T
) async throws -> T {
    let token = CancelToken()
    do {
        return try await withTaskCancellationHandler {
            try await operation(token)
        } onCancel: {
            token.cancel()
        }
    } catch CoreError.Cancelled where Task.isCancelled {
        throw CancellationError()
    }
}

extension Core {
    /// A diagnostics report for a bug report. Cancelled with the calling task.
    public func exportDiagnostics() async throws -> String {
        try await withCoreCancellation { token in
            try await self.exportDiagnostics(token: token)
        }
    }

    /// The capabilities state, mirrored on the main actor. Starts empty; the core's current value follows at once.
    @MainActor
    public func observeCapabilities() -> ObservedValue<CapabilitiesSnapshot> {
        ObservedValue(initial: CapabilitiesSnapshot(servers: [])) { forwarder in
            self.subscribeCapabilities(listener: forwarder)
        }
    }
}
