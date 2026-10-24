// Tests for the adapters, against the real core.

import CurrentCore
import CurrentFFIBindings
import Testing

@Suite struct ListDiffTests {
    @Test func appliesABatchInOrder() throws {
        var items = ["a", "b"]
        let batch: [ListDiff<String>] = [.insert(index: 0, item: "z"), .remove(index: 2), .set(index: 1, item: "y"), .append(["c"])]
        for diff in batch { try diff.apply(to: &items) }
        #expect(items == ["z", "y", "c"])
        try ListDiff.reset(["r"]).apply(to: &items)
        #expect(items == ["r"])
    }

    @Test func aBadIndexChangesNothing() {
        var items = ["a"]
        #expect(throws: ListDiffOutOfRange(index: 2, count: 1)) { try ListDiff.insert(index: 2, item: "b").apply(to: &items) }
        #expect(throws: ListDiffOutOfRange(index: 1, count: 1)) { try ListDiff<String>.remove(index: 1).apply(to: &items) }
        #expect(throws: ListDiffOutOfRange(index: -1, count: 1)) { try ListDiff.set(index: -1, item: "b").apply(to: &items) }
        #expect(items == ["a"])
    }
}

@Suite struct CoreTests {
    @Test func exportsDiagnostics() async throws {
        let core = try Core()
        let report = try await core.exportDiagnostics()
        #expect(report.hasPrefix("Current \(core.appInfo().version)"))
    }

    @Test func aCancelledTaskCancelsTheCall() async throws {
        let core = try Core()
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await core.exportDiagnostics()
        }
        await #expect(throws: CancellationError.self) { try await task.value }
    }

    @MainActor @Test func theCoresValueReplacesTheInitialOne() async throws {
        let core = try Core()
        // Start from a value the core does not have, so only a real delivery can replace it.
        let placeholder = ServerStatusSnapshot(
            service: "placeholder", key: "0", signedIn: false, reachability: .unknown, addressPosition: nil, user: nil, version: nil,
            features: [], gatewaySignInNeeded: false)
        let capabilities = ObservedValue(initial: CapabilitiesSnapshot(servers: [placeholder])) { forwarder in
            core.subscribeCapabilities(listener: forwarder)
        }
        #expect(capabilities.value.servers.count == 1)
        for _ in 0..<200 where !capabilities.value.servers.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        // The pump asserts it runs on the main actor (MainActor.assertIsolated) before it sets the value.
        #expect(capabilities.value.servers.isEmpty)
        capabilities.cancel()
    }

    @MainActor @Test func cancellingTheModelReleasesTheListener() async throws {
        let core = try Core()
        let capabilities = core.observeCapabilities()
        #expect(core.liveListeners() == 1)
        #expect(capabilities.value.servers.isEmpty)
        capabilities.cancel()
        for _ in 0..<200 where core.liveListeners() != 0 {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(core.liveListeners() == 0)
    }
}
