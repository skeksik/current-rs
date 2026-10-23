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

// `openCore(dataDir:deviceName:secrets:)` needs no wrapper: it is not cancellable, and the core does the blocking work on its own
// threads, so awaiting it on the main actor does not block it.

extension Core {
    /// A diagnostics report for a bug report. Cancelled with the calling task.
    public func exportDiagnostics() async throws -> String {
        try await withCoreCancellation { token in
            try await self.exportDiagnostics(token: token)
        }
    }

    /// Checks every saved session; the capabilities state shows the outcome. Cancelled with the calling task.
    public func restoreSessions() async throws {
        try await withCoreCancellation { token in
            try await self.restoreSessions(token: token)
        }
    }

    /// The capabilities state, mirrored on the main actor. Starts empty; the core's current value follows at once.
    @MainActor
    public func observeCapabilities() -> ObservedValue<CapabilitiesSnapshot> {
        ObservedValue(initial: CapabilitiesSnapshot(servers: [])) { forwarder in
            self.subscribeCapabilities(listener: forwarder)
        }
    }

    /// The saved servers, mirrored on the main actor with diffs. Starts empty; the core's list follows at once.
    @MainActor
    public func observeServers() -> ObservedList<SavedServerSnapshot> {
        ObservedList { forwarder in
            self.subscribeServers(listener: forwarder)
        }
    }
}

extension AddServerFlow {
    /// Probes the address the user typed; the outcome is the state. Cancelled with the calling task, which returns the flow to idle.
    public func probe(input: String) async throws {
        try await withCoreCancellation { token in
            try await self.probe(input: input, token: token)
        }
    }

    /// The flow's state, mirrored on the main actor.
    @MainActor
    public func observeState() -> ObservedValue<AddServerState> {
        ObservedValue(initial: state()) { forwarder in
            self.subscribeState(listener: forwarder)
        }
    }
}

extension DiscoveryFlow {
    /// Searches the local network once, from an unbound socket and from each of `interfaces`, the device's up IPv4 interfaces
    /// listed just before (ADR 0016; the macOS app lists them in a later task). Cancelled with the calling task, which returns
    /// the flow to idle.
    public func search(interfaces: [LocalInterface] = []) async throws {
        try await withCoreCancellation { token in
            try await self.search(interfaces: interfaces, token: token)
        }
    }

    /// The flow's state, mirrored on the main actor.
    @MainActor
    public func observeState() -> ObservedValue<DiscoveryState> {
        ObservedValue(initial: state()) { forwarder in
            self.subscribeState(listener: forwarder)
        }
    }
}

extension SignInFlow {
    /// Asks whether the server offers Quick Connect; the answer shows in the ready state.
    public func checkQuickConnect() async throws {
        try await withCoreCancellation { token in
            try await self.checkQuickConnect(token: token)
        }
    }

    /// Signs in with a user name and password. Cancelled with the calling task, which returns the flow to ready.
    public func withPassword(username: String, password: String) async throws {
        try await withCoreCancellation { token in
            try await self.withPassword(username: username, password: password, token: token)
        }
    }

    /// Signs in with Quick Connect: the state shows the code until another client approves it. Cancelled with the calling task,
    /// which stops polling and returns the flow to ready.
    public func withQuickConnect() async throws {
        try await withCoreCancellation { token in
            try await self.withQuickConnect(token: token)
        }
    }

    /// The flow's state, mirrored on the main actor.
    @MainActor
    public func observeState() -> ObservedValue<SignInState> {
        ObservedValue(initial: state()) { forwarder in
            self.subscribeState(listener: forwarder)
        }
    }
}
