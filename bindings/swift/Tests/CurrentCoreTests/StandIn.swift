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

    static let quickConnectCode = "123456"

    let address: String
    private let process: Process
    private let input: Pipe
    private let lines: LineReader

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
        lines = LineReader(output.fileHandleForReading)
        try process.run()

        // The first line is "listening <address>".
        guard let line = lines.next(timeout: 10), line.hasPrefix("listening ") else {
            Self.stop(process, input)
            throw Failure(description: "the stand-in did not report its address within 10 s")
        }
        address = String(line.dropFirst("listening ".count))
    }

    /// Sends a command: `revoke`; `quick-connect off`, `on` or `hold` (requests stay unapproved); `hold <path>`, `release <path>`
    /// (requests to the path wait, unanswered, until released).
    ///
    /// The stand-in reads commands on a thread of its own, so a request sent right after a bare write could arrive before the command
    /// applies. Each command is therefore followed by a count, which the stand-in answers only after every earlier line: when this
    /// returns, the command has taken effect (or the stand-in did not answer within 10 s, which the test's next step shows).
    func send(_ command: String) {
        write(command)
        _ = try? count("/")
    }

    private func write(_ command: String) {
        input.fileHandleForWriting.write(Data("\(command)\n".utf8))
    }

    /// How many requests to `path` the stand-in received so far.
    func count(_ path: String) throws -> Int {
        write("count \(path)")
        guard let line = lines.next(timeout: 10), line.hasPrefix("count "), let count = Int(line.dropFirst("count ".count)) else {
            throw Failure(description: "the stand-in did not answer the count")
        }
        return count
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

/// The lines a process writes, read as they arrive (a readability handler), so that waiting for one can time out.
final class LineReader: @unchecked Sendable {
    private let condition = NSCondition()
    private var buffer = Data()
    private var closed = false

    init(_ handle: FileHandle) {
        handle.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            self.condition.lock()
            if data.isEmpty {
                self.closed = true
                handle.readabilityHandler = nil
            } else {
                self.buffer.append(data)
            }
            self.condition.broadcast()
            self.condition.unlock()
        }
    }

    /// The next line without its newline, or nil if none came within `timeout` seconds or the output ended.
    func next(timeout: TimeInterval) -> String? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let end = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                let line = String(decoding: buffer[buffer.startIndex..<end], as: UTF8.self)
                buffer.removeSubrange(buffer.startIndex...end)
                return line.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            if closed || !condition.wait(until: deadline) { return nil }
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
