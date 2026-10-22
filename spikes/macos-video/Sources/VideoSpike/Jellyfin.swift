import Foundation
import Observation
import SwiftUI

/// Optional check against the owner's Jellyfin: address and credentials are typed at runtime and live in memory only
/// (ephemeral URLSession, nothing written to disk or the log).
@MainActor @Observable
final class JellyfinSession {
    struct Item: Identifiable {
        let id: String
        let name: String
        let summary: String
        let directURL: URL
        let planURL: URL?
        let plan: String
    }

    var server = ""
    var username = ""
    var password = ""
    var query = ""
    var status = ""
    var items: [Item] = []
    private(set) var signedIn = false
    private var token = ""
    private var userId = ""
    private let deviceId = "spike-" + UUID().uuidString
    private let session = URLSession(configuration: .ephemeral)

    private var authHeader: String {
        var value = #"MediaBrowser Client="Current Spike", Device="Mac", DeviceId="\#(deviceId)", Version="0.0.1""#
        if !token.isEmpty { value += #", Token="\#(token)""# }
        return value
    }

    private var base: URL? { URL(string: server.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "/"))) }

    func signIn() async {
        status = "Signing in…"
        do {
            let json = try await request("Users/AuthenticateByName", method: "POST", body: ["Username": username, "Pw": password])
            token = json["AccessToken"] as? String ?? ""
            userId = (json["User"] as? [String: Any])?["Id"] as? String ?? ""
            password = ""
            signedIn = !token.isEmpty
            status = signedIn ? "Signed in" : "No token in the response"
        } catch {
            status = "Sign-in failed: \(error.localizedDescription)"
        }
    }

    func search() async {
        status = "Searching…"
        do {
            let json = try await request(
                "Items", query: [
                    "userId": userId, "searchTerm": query, "includeItemTypes": "Movie,Episode", "recursive": "true",
                    "fields": "MediaSources", "limit": "25",
                ])
            var found: [Item] = []
            for raw in json["Items"] as? [[String: Any]] ?? [] {
                guard let id = raw["Id"] as? String else { continue }
                found.append(try await describe(id: id, name: raw["Name"] as? String ?? id))
            }
            items = found
            status = "\(found.count) items"
        } catch {
            status = "Search failed: \(error.localizedDescription)"
        }
    }

    /// Asks the server for a plan with a device profile describing AVPlayer, as the route planner would.
    private func describe(id: String, name: String) async throws -> Item {
        let info = try await request(
            "Items/\(id)/PlaybackInfo", method: "POST", query: ["userId": userId],
            body: ["DeviceProfile": Self.avPlayerProfile, "EnableDirectPlay": true, "EnableDirectStream": true, "EnableTranscoding": true])
        let source = (info["MediaSources"] as? [[String: Any]])?.first ?? [:]
        let sourceId = source["Id"] as? String ?? id
        let streams = source["MediaStreams"] as? [[String: Any]] ?? []
        let summary = ([source["Container"] as? String ?? "?"] + streams.map { stream in
            let type = (stream["Type"] as? String ?? "?").prefix(1).lowercased()
            var codec = stream["Codec"] as? String ?? "?"
            if let range = stream["VideoRangeType"] as? String, range != "SDR", type == "v" { codec += " \(range)" }
            return "\(type):\(codec)"
        }).joined(separator: " ")

        var plan = "direct play \(source["SupportsDirectPlay"] as? Bool ?? false), direct stream \(source["SupportsDirectStream"] as? Bool ?? false)"
        var planURL: URL?
        if let path = source["TranscodingUrl"] as? String, let base {
            planURL = URL(string: base.absoluteString + path)
            let reasons = URLComponents(string: path)?.queryItems?.first { $0.name == "TranscodeReasons" }?.value ?? "-"
            plan += ", server HLS (reasons: \(reasons))"
        }
        var direct = URLComponents(url: base!.appending(path: "Videos/\(id)/stream"), resolvingAgainstBaseURL: false)!
        direct.queryItems = [.init(name: "static", value: "true"), .init(name: "mediaSourceId", value: sourceId), .init(name: "api_key", value: token)]
        return Item(id: id, name: name, summary: summary, directURL: direct.url!, planURL: planURL, plan: plan)
    }

    private func request(_ path: String, method: String = "GET", query: [String: String] = [:], body: [String: Any]? = nil)
        async throws -> [String: Any]
    {
        guard let base else { throw URLError(.badURL) }
        var components = URLComponents(url: base.appending(path: path), resolvingAgainstBaseURL: false)!
        if !query.isEmpty { components.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) } }
        var request = URLRequest(url: components.url!)
        request.httpMethod = method
        request.setValue(authHeader, forHTTPHeaderField: "Authorization")
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.badServerResponse, userInfo: [NSLocalizedDescriptionKey: "HTTP \((response as? HTTPURLResponse)?.statusCode ?? 0)"])
        }
        return (try JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    /// What AVPlayer plays directly according to this spike's probe; everything else goes to HLS with fMP4 segments.
    static let avPlayerProfile: [String: Any] = [
        "Name": "Current spike (AVPlayer)",
        "MaxStreamingBitrate": 120_000_000,
        "DirectPlayProfiles": [
            ["Type": "Video", "Container": "mp4,m4v,mov", "VideoCodec": "h264,hevc", "AudioCodec": "aac,ac3,eac3,alac,flac,mp3,opus"],
        ],
        "TranscodingProfiles": [
            [
                "Type": "Video", "Container": "mp4", "Protocol": "hls", "Context": "Streaming", "VideoCodec": "hevc,h264",
                "AudioCodec": "aac,ac3,eac3,flac,alac", "MaxAudioChannels": "8", "BreakOnNonKeyFrames": true, "MinSegments": 1,
            ],
        ],
        "SubtitleProfiles": [
            ["Format": "vtt", "Method": "Hls"],
            ["Format": "mov_text", "Method": "Embed"],
        ],
        "CodecProfiles": [],
        "ContainerProfiles": [],
    ]
}

struct JellyfinSheet: View {
    @Bindable var session: JellyfinSession
    let model: PlayerModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !session.signedIn {
                Form {
                    TextField("Server", text: $session.server, prompt: Text("https://media.example.com"))
                    TextField("User", text: $session.username)
                    SecureField("Password", text: $session.password)
                }
                Button("Sign In") { Task { await session.signIn() } }
                    .keyboardShortcut(.defaultAction)
            } else {
                HStack {
                    TextField("Search", text: $session.query)
                        .onSubmit { Task { await session.search() } }
                    Button("Search") { Task { await session.search() } }
                }
                List(session.items) { item in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(item.name).bold()
                        Text(item.summary).font(.caption.monospaced())
                        Text(item.plan).font(.caption).foregroundStyle(.secondary)
                        HStack {
                            Button("Play file directly") {
                                model.note("jellyfin direct: \(item.summary)")
                                model.loadRemote(item.directURL, displayName: item.name)
                                dismiss()
                            }
                            if let plan = item.planURL {
                                Button("Play server HLS") {
                                    model.note("jellyfin HLS: \(item.summary) | \(item.plan)")
                                    model.loadRemote(plan, displayName: item.name)
                                    dismiss()
                                }
                            }
                        }
                    }
                }
                .frame(minHeight: 300)
            }
            Text(session.status).font(.caption).foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(width: 640)
    }
}
