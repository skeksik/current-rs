// The M1 flows through the adapters, against the Jellyfin stand-in: opening, adding a server, signing in, the saved-server list,
// restoring and signing out. Secrets stay in memory: the macOS keychain is not tested here (CI's login keychain may prompt or be
// locked; not verified), the Windows keyring is, in the C# tests.

import CurrentCore
import CurrentFFIBindings
import Foundation
import Testing

private let firstServer = ServerRef(service: "jellyfin", key: "1")

/// Waits, off the main actor, until `done` holds.
private func waitFor(_ what: String, _ done: () throws -> Bool) async throws {
    for _ in 0..<1000 {
        if try done() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("timed out waiting for \(what)")
}

/// Waits on the main actor until `done` holds.
@MainActor
private func eventually(_ what: String, _ done: () -> Bool) async throws {
    for _ in 0..<1000 {
        if done() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("timed out waiting for \(what)")
}

private func ready(_ add: AddServerFlow, _ standIn: StandIn) async throws -> NewServerInfo {
    try await add.probe(input: standIn.address)
    guard case .ready(let server) = add.state() else {
        throw StandIn.Failure(description: "not ready: \(add.state())")
    }
    return server
}

private func signIn(_ core: Core, _ standIn: StandIn) async throws {
    let add = core.addServer()
    _ = try await ready(add, standIn)
    let flow = try add.signIn()
    try await flow.withPassword(username: StandIn.userName, password: StandIn.password)
    guard case .signedIn = flow.state() else {
        throw StandIn.Failure(description: "not signed in: \(flow.state())")
    }
}

/// The first server's status once `done` holds for it.
@MainActor
private func status(_ core: Core, _ done: (ServerStatusSnapshot) -> Bool) async throws -> ServerStatusSnapshot? {
    let capabilities = core.observeCapabilities()
    defer { capabilities.cancel() }
    try await eventually("the status") { capabilities.value.servers.first.map(done) ?? false }
    return capabilities.value.servers.first
}

@Suite struct FlowTests {
    @Test func openingCreatesThenLoadsTheSettings() async throws {
        let folder = try DataFolder()
        defer { folder.remove() }
        let core = try await openCore(dataDir: folder.path, deviceName: "Example PC", secrets: .memory)
        #expect(core.loadReport() == .created)
        #expect(core.servers().isEmpty)
        let report = try await core.exportDiagnostics()
        #expect(report.hasPrefix("Current \(core.appInfo().version)\nPlatform: macos aarch64\nSettings: created\n"))
        #expect(!report.contains(folder.path))
        #expect(!report.contains("Example PC"))

        let again = try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory)
        #expect(again.loadReport() == .loaded)
        #expect(try Core().loadReport() == nil)
    }

    @Test func openingReportsAFolderItCannotUse() async throws {
        let folder = try DataFolder()
        defer { folder.remove() }
        let file = folder.path + "/file"
        try Data("x".utf8).write(to: URL(fileURLWithPath: file))
        do {
            _ = try await openCore(dataDir: file, deviceName: "", secrets: .memory)
            Issue.record("opening succeeded")
        } catch CoreError.Store(problem: .io) {
        }
    }

    @MainActor @Test func addingAServerAndSigningInWithAPassword() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let folder = try DataFolder()
        defer { folder.remove() }
        let core = try await openCore(dataDir: folder.path, deviceName: "Example PC", secrets: .memory)
        let servers = core.observeServers()
        defer { servers.cancel() }

        let add = core.addServer()
        let addState = add.observeState()
        defer { addState.cancel() }
        #expect(addState.value == .idle)
        #expect(throws: CoreError.NotReady) { _ = try add.signIn() }
        let server = try await ready(add, standIn)
        #expect(server == NewServerInfo(
            id: StandIn.serverId, name: StandIn.serverName, version: "12.2.0", product: "Jellyfin Server", httpsFailedTls: false))
        try await eventually("the model sees ready") { if case .ready = addState.value { true } else { false } }

        let flow = try add.signIn()
        let signInState = flow.observeState()
        defer { signInState.cancel() }
        try await flow.checkQuickConnect()
        #expect(flow.state() == .ready(quickConnect: true))
        try await flow.withPassword(username: StandIn.userName, password: StandIn.password)
        try await eventually("the model sees signed in") { if case .signedIn = signInState.value { true } else { false } }
        guard case .signedIn(let saved, let user, let remembered) = signInState.value else { return }
        #expect(saved == firstServer)
        #expect(user.id == StandIn.userId && user.name == StandIn.userName)
        #expect(remembered)

        try await eventually("the list shows the server") {
            servers.items.count == 1 && servers.items[0].activeUser == StandIn.userId
        }
        #expect(servers.items[0].name == StandIn.serverName)
        #expect(servers.items[0].users == [SavedUserSnapshot(id: StandIn.userId, name: StandIn.userName)])
        let first = try await status(core) { $0.signedIn }
        #expect(first?.reachability == .reachable)
    }

    @Test func aWrongPasswordFailsAndSavesNothing() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        try await flow.withPassword(username: StandIn.userName, password: "wrong")
        #expect(flow.state() == .failed(problem: .wrongCredentials))
        #expect(core.servers().isEmpty)
    }

    @Test func aBadAddressFailsWithItsReason() async throws {
        let core = try Core()
        let add = core.addServer()
        try await add.probe(input: "ftp://media.example.com")
        #expect(add.state() == .failed(problem: .invalidAddress(reason: .unsupportedScheme)))
        add.reset()
        #expect(add.state() == .idle)
    }

    @Test func quickConnectSignsIn() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        try await flow.withQuickConnect()
        guard case .signedIn = flow.state() else {
            Issue.record("not signed in: \(flow.state())")
            return
        }
    }

    @Test func quickConnectTurnedOffFails() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        standIn.send("quick-connect off")
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        try await flow.withQuickConnect()
        #expect(flow.state() == .failed(problem: .quickConnectDisabled))
    }

    @Test func aCancelledSignInReturnsToReady() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            try await flow.withQuickConnect()
        }
        await #expect(throws: CancellationError.self) { try await task.value }
        guard case .ready = flow.state() else {
            Issue.record("not ready: \(flow.state())")
            return
        }
    }

    @MainActor @Test func signingOutClearsTheUser() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let folder = try DataFolder()
        defer { folder.remove() }
        let core = try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory)
        try await signIn(core, standIn)
        let result = try await core.signOut(server: firstServer)
        #expect(result.serverTold)
        #expect(core.servers()[0].activeUser == nil)
        _ = try await status(core) { !$0.signedIn }
        let again = try await core.signOut(server: firstServer)
        #expect(!again.serverTold)
        await #expect(throws: CoreError.UnknownServer) {
            _ = try await core.signOut(server: ServerRef(service: "jellyfin", key: "9"))
        }
    }

    @MainActor @Test func removingAServerForgetsIt() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let folder = try DataFolder()
        defer { folder.remove() }
        let core = try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory)
        try await signIn(core, standIn)
        let result = try await core.removeServer(server: firstServer)
        #expect(result.removed && !result.credentialsLeft)
        #expect(core.servers().isEmpty)
        let capabilities = core.observeCapabilities()
        defer { capabilities.cancel() }
        try await eventually("no status") { capabilities.value.servers.isEmpty }
        try await waitFor("the server is told") { try standIn.count("/Sessions/Logout") == 1 }
        let again = try await core.removeServer(server: firstServer)
        #expect(!again.removed)
        let next = try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory)
        #expect(next.servers().isEmpty)
    }

    @MainActor @Test func withoutTheTokenRestoringSignsOut() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let folder = try DataFolder()
        defer { folder.remove() }
        try await signIn(try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory), standIn)
        // Memory secrets die with their core: the next one finds the saved user but no token.
        let next = try await openCore(dataDir: folder.path, deviceName: "", secrets: .memory)
        #expect(next.servers()[0].activeUser == StandIn.userId)
        try await next.restoreSessions()
        _ = try await status(next) { !$0.signedIn }
        let servers = next.observeServers()
        defer { servers.cancel() }
        try await eventually("the active user is cleared") { servers.items.count == 1 && servers.items[0].activeUser == nil }

        let again = try next.signInAgain(server: firstServer)
        try await again.withPassword(username: StandIn.userName, password: StandIn.password)
        guard case .signedIn = again.state() else {
            Issue.record("not signed in: \(again.state())")
            return
        }
    }

    @Test func discoveryEnds() async throws {
        let core = try Core()
        let discovery = core.discoverServers()
        try await discovery.search()
        // Whether the runner may broadcast, and whether anything answers, depends on the machine: only the shape is checked.
        switch discovery.state() {
        case .done, .failed: break
        default: Issue.record("still searching: \(discovery.state())")
        }
        #expect(!core.addServer().pick(discovery: discovery, serverId: "not-listed"))
        discovery.reset()
        #expect(discovery.state() == .idle)
    }

    @MainActor @Test func flowModelsReleaseTheirListeners() async throws {
        let core = try Core()
        let add = core.addServer()
        let state = add.observeState()
        let servers = core.observeServers()
        #expect(core.liveListeners() == 2)
        state.cancel()
        servers.cancel()
        try await eventually("the core drops both") { core.liveListeners() == 0 }
    }

    @Test func networkReportsAreAccepted() throws {
        let core = try Core()
        core.networkChanged(facts: NetworkFacts(
            connectivity: .full, metered: false, constrained: nil, interface: .wifi, vpn: nil, networkKey: "home"))
        core.returnedToForeground()
    }

    @Test func diagnosticsAfterASignInHoldNothingPrivate() async throws {
        let standIn = try StandIn()
        defer { standIn.stop() }
        let folder = try DataFolder()
        defer { folder.remove() }
        let core = try await openCore(dataDir: folder.path, deviceName: "Example PC", secrets: .memory)
        try await signIn(core, standIn)
        let report = try await core.exportDiagnostics()
        #expect(report.contains("/Users/AuthenticateByName"))
        let port = String(standIn.address.split(separator: ":").last ?? "")
        // The device name goes to the server in every request's header; it must not reach the report.
        for secret in ["Example PC", StandIn.password, StandIn.userName, "127.0.0.1", port, folder.path] {
            #expect(!report.contains(secret), "\(secret) in the report")
        }
    }

    @Test func cancellingWhileSigningInReturnsToReady() async throws {
        let path = "/Users/AuthenticateByName"
        let standIn = try StandIn()
        defer { standIn.stop() }
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        standIn.send("hold \(path)")
        let call = Task { try await flow.withPassword(username: StandIn.userName, password: StandIn.password) }
        try await waitFor("the request waits at the server") { try standIn.count(path) == 1 && flow.state() == .signingIn }
        call.cancel()
        await #expect(throws: CancellationError.self) { try await call.value }
        guard case .ready = flow.state() else {
            Issue.record("not ready: \(flow.state())")
            return
        }

        // The server answers after all: too late, nothing is saved.
        standIn.send("release \(path)")
        try await Task.sleep(for: .milliseconds(300))
        if case .ready = flow.state() {} else { Issue.record("not ready after the answer: \(flow.state())") }
        #expect(core.servers().isEmpty)
    }

    @Test func cancellingWhileQuickConnectWaitsStopsPolling() async throws {
        let path = "/QuickConnect/Connect"
        let standIn = try StandIn()
        defer { standIn.stop() }
        standIn.send("quick-connect hold")
        let core = try Core()
        let add = core.addServer()
        _ = try await ready(add, standIn)
        let flow = try add.signIn()
        let call = Task { try await flow.withQuickConnect() }
        try await waitFor("waiting for approval") {
            try flow.state() == .quickConnectWaiting(code: StandIn.quickConnectCode) && standIn.count(path) >= 1
        }
        call.cancel()
        await #expect(throws: CancellationError.self) { try await call.value }
        guard case .ready = flow.state() else {
            Issue.record("not ready: \(flow.state())")
            return
        }
        // The core asks every 5 s (Jellyfin's interval): a poll would show within 6 s.
        let polls = try standIn.count(path)
        try await Task.sleep(for: .seconds(6))
        #expect(try standIn.count(path) == polls)
    }

    @Test func networkFactsHideTheKey() {
        let facts = NetworkFacts(
            connectivity: .full, metered: nil, constrained: nil, interface: .wifi, vpn: nil, networkKey: "home-wifi-192.168.1.10")
        for text in [String(describing: facts), "\(facts)", String(reflecting: facts)] {
            #expect(text.contains("[redacted]"))
            #expect(!text.contains("home-wifi"))
        }
    }
}
