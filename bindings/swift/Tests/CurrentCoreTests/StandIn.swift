// The Jellyfin stand-in (tools/jellyfin-stand-in) as a child process, for the flow tests. build.sh builds it into target/release.
// Stopping it closes its stdin, which ends it; if the test process dies first, the pipe closes all the same. Tests call `stop()` in a
// `defer`, which also keeps it alive to the end of the test.

import Foundation

final class StandIn: @unchecked Sendable {
    // The stand-in's fake account (tools/jellyfin-stand-in/src/lib.rs).
    static let userName = "stand-in-user"
    static let password = "not-a-real-password"
    static let userId = "57a2d1e0000000000000000000000002"
    static let serverId = "57a2d1e0000000000000000000000001"
    static let serverName = "Stand-in Server"

    let address: String
    private let process: Process
    private let input: Pipe

    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    /// The built binary. The repository root is four folders above this file's folder.
    static var binary: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("target/release/jellyfin-stand-in")
    }

    init() throws {
        let binary = Self.binary
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw Failure(description: "\(binary.path) is missing: run bindings/swift/build.sh")
        }
        process = Process()
        process.executableURL = binary
        input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        try process.run()

        // The first line is "listening <address>".
        var line = Data()
        let deadline = Date().addingTimeInterval(10)
        while !line.contains(UInt8(ascii: "\n")), Date() < deadline {
            let chunk = output.fileHandleForReading.availableData
            if chunk.isEmpty { break }
            line.append(chunk)
        }
        let text = String(decoding: line, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard text.hasPrefix("listening ") else {
            Self.stop(process, input)
            throw Failure(description: "the stand-in did not report its address: \(text)")
        }
        address = String(text.dropFirst("listening ".count))
    }

    /// Sends a command: `revoke`, `quick-connect off` or `quick-connect on`.
    func send(_ command: String) {
        input.fileHandleForWriting.write(Data("\(command)\n".utf8))
    }

    func stop() {
        Self.stop(process, input)
    }

    deinit { Self.stop(process, input) }

    private static func stop(_ process: Process, _ input: Pipe) {
        try? input.fileHandleForWriting.close()
        if process.isRunning {
            let deadline = Date().addingTimeInterval(5)
            while process.isRunning, Date() < deadline { usleep(10_000) }
            if process.isRunning { process.terminate() }
        }
    }
}

/// A fresh data folder. Tests call `remove()` in a `defer`, which also keeps the folder alive to the end of the test (ARC may
/// release a value after its last use).
final class DataFolder: Sendable {
    let path: String

    init() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("current-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        path = url.path
    }

    func remove() { try? FileManager.default.removeItem(atPath: path) }
}
