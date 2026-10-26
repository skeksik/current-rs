# Loopback relay spike notes (2026-10-26)

Decision record: `docs/decisions/0020-loopback-relay.md`. This file has the evidence: what the code does today, what Apple and
Microsoft document, what the lock file already holds, and one throw-away experiment. No product code was written.

Sources, all read 2026-10-26 unless a line says otherwise:
- This repository at `d82a18c` (main): `crates/jellyfin/src/hls.rs`, `crates/mpv/src/options.rs`, `crates/playback/src/{stream,engine,
  controller,server}.rs`, `crates/app/src/player/{server,mpv}.rs`, `crates/net/src/{client,gateway,log}.rs`, `Cargo.toml`,
  `Cargo.lock`, `deny.toml`; ADRs 0003, 0006, 0008, 0010, 0014, 0015, 0017, 0018, 0019; `docs/notes/{gateway-auth,playback,
  mpv-engine,macos-video}.md`.
- Crate sources from the registry at the versions the lock file pins: hyper 1.12.0, hyper-util 0.1.21, reqwest 0.13.5,
  http-body-util 0.1.5, httpdate 1.0.3 (their `Cargo.toml` and `src/`).
- Apple developer documentation, read through its JSON form (`https://developer.apple.com/tutorials/data/documentation/<path>.json`,
  the data behind each page), pages named below.
- Apple Developer Forums thread 113063 (https://developer.apple.com/forums/thread/113063).
- Microsoft Learn, "Filter Origin Audit Log" (https://learn.microsoft.com/en-us/windows/security/operating-system-security/
  network-security/windows-firewall/filter-origin-documentation, `ms.date` 2025-04-07, updated 2026-10-08).

## 1. What the code does today

### The credentials reach the player
- `current-net`'s `ServerClient::stream_credentials(url)` gives the address's gateway headers (Basic, extra headers) and the cookies
  that apply to the URL (`crates/net/src/client.rs`, `crates/net/src/gateway.rs`, `StreamCredentials`).
- `current-app`'s `JellyfinMedia` turns them into `current-playback`'s `StreamAuth { cookies, headers, secrets }`
  (`crates/app/src/player/server.rs`), the controller puts it into `EngineFile`, and `current-app`'s mpv engine adapter into
  `current-mpv`'s `FileRequest` (`crates/app/src/player/mpv.rs`).
- `FileRequest::lavf_options` writes `max_redirects=0`, `cookies` (each cookie's domain set to the stream's host and port; Secure only
  over https) and `headers` into `stream-lavf-o`; `file_options` adds `access-references=no` for any load with credentials that is not
  marked `playlist_checked_same_origin`, and `check_header_scope` refuses a `.m3u8`/`.m3u`/`.mpd` URL with credentials and no mark
  (`crates/mpv/src/options.rs`).

### The playlist check
- `JellyfinClient::check_playlist_origin` (`crates/jellyfin/src/hls.rs`) fetches the master playlist and up to 32 media playlists
  (4 MiB each) through the address's client and checks every URI line and every `URI`/`*-URI` attribute, after RFC 8216 §4.1, §4.2,
  §4.3 and §6.3.1 (quoted in the file's header). The controller calls it through `MediaServer::check_playlist` and sets
  `EngineFile::playlist_checked`; a failure is `PlaybackProblem::PlaylistElsewhere`.
- The file says itself why it is not enough: "A check, not enforcement: mpv fetches the playlists again itself, so a playlist that
  changes after the check could still name another host." ADR 0019 ("Playlists and headers") gives the owner's choice: check now, the
  relay enforces later.

### What a gateway refusal mid-play does
- `current-app` maps the client's errors to `PlaybackProblem` in `problem()` (`crates/app/src/player/server.rs`): `AuthError::Gateway`
  to `GatewaySignInNeeded`, a revoked session to `SignedOut`, 404 to `NotFound`, other statuses to `Server { status }`.
- That mapping covers the core's own requests (PlaybackInfo, reports, subtitles, the playlist check). A refusal of a request that mpv
  made itself reaches the controller only as an engine end-of-file error (`engine_failed`, `crates/playback/src/controller.rs`), which
  on direct play re-plans once without direct play (choice 9) and otherwise fails with `Engine { kind }`. The status FFmpeg got is not
  seen by the core. Whether a stream's 401 reaches ADR 0017's 401 rule some other way was not traced further.

### The core's HTTP client buffers, but can stream
- `ServerClient::send` reads the whole body (`read_body`, with `max_body`) and has one deadline for the whole request
  (`ClientConfig::timeout`, 30 s by default). Neither suits a two-hour direct-play body.
- reqwest 0.13.5 offers what a streaming send needs without a new feature: `Response::chunk()` (`src/async_impl/response.rs` line
  310, not behind a feature; `bytes_stream` is behind `stream`, which is not enabled) and `ClientBuilder::read_timeout`
  (`src/async_impl/client.rs` line 1462), a per-read timeout beside the total `timeout`.
- The request log (ADR 0006) keeps the path and redacts every query value not on a short harmless list (`crates/net/src/log.rs`,
  `redact_query`), so `ApiKey` never reaches it. A two-hour HLS stream with 6-second segments is about 1,200 requests, which would
  push everything else out of the bounded log.

## 2. HTTP servers: what the lock file already has

### hyper is there, without its server
`cargo tree -e features -i hyper` on main: hyper 1.12.0 with features `client`, `default`, `http1`, `http2`, reached only through
reqwest 0.13.5 (and `hyper-util`, `hyper-rustls`). Not `server`. `hyper-util` 0.1.21 has `client`, `client-legacy`, `client-proxy`,
`client-proxy-system`, `http1`, `http2`, `tokio`. `http-body-util` 0.1.5, `http` 1.5.0, `http-body` 1.1.0, `bytes` 1.12.1, `httparse`
1.10.1 and `tower` are in the lock too.

hyper 1.12.0's `Cargo.toml`, `[features]`:
```
server = ["dep:httpdate", "dep:pin-project-lite", "dep:smallvec"]
http1  = ["dep:atomic-waker", "dep:futures-core", "dep:httparse", "dep:itoa"]
```
`pin-project-lite` and `smallvec` are already built for `client`; `httpdate` is the only crate `server` adds. hyper-util's
`TokioIo` (the adapter from tokio's sockets to hyper's I/O traits) is behind its `tokio` feature (`src/rt/mod.rs`:
`#[cfg(feature = "tokio")] pub use self::tokio::{TokioExecutor, TokioIo, TokioTimer};`), which is already on.

### Experiment (scratch folder, outside the repository, deleted with the session)
A crate with the workspace's `Cargo.lock` copied in and these dependencies:
```
reqwest = { version = "=0.13.5", default-features = false, features = ["rustls-no-provider", "http2", "charset", "system-proxy"] }
tokio = { version = "=1.54.1", default-features = false, features = ["net", "rt", "io-util", "macros"] }
hyper = { version = "=1.12.0", default-features = false, features = ["server", "http1"] }
hyper-util = { version = "=0.1.21", default-features = false, features = ["tokio"] }
http-body-util = "=0.1.5"
bytes = "=1.12.1"
```
- `cargo tree -e normal` with and without the last four lines, diffed: the only crate added is **`httpdate v1.0.3`** (`tokio-macros`
  appeared too, from the experiment's own `macros` feature; it is already in the workspace's lock through the tests).
- A 40-line server (`http1::Builder::serve_connection` over `TokioIo`, `service_fn`) bound to `127.0.0.1:0` answered a raw
  `GET /<secret>/v` with `Range: bytes=0-4` with `HTTP/1.1 206 Partial Content`, and `GET /wrong/v` with `404 Not Found`. So the
  feature set above is enough for an HTTP/1.1 server with range answers; nothing about streaming bodies was measured.

### Licences (from each crate's `Cargo.toml`)
| Crate | Version | Licence | In the lock today |
|---|---|---|---|
| hyper | 1.12.0 | MIT | yes (client) |
| hyper-util | 0.1.21 | MIT | yes |
| http-body-util | 0.1.5 | MIT | yes |
| httparse | 1.10.1 | MIT OR Apache-2.0 | yes |
| httpdate | 1.0.3 | MIT OR Apache-2.0 | **no: new** |

All are in `deny.toml`'s allow list (ADR 0003). `http`, `http-body` and `bytes` are reached already; making any of these direct
dependencies needs the owner's approval (CLAUDE.md), even where nothing new is compiled.

### Other options considered
- **A hand-written HTTP/1.1 subset over tokio.** The relay needs very little of HTTP: GET and HEAD, no request body, a few request
  headers (`Host`, `Range`, `If-Range`, `Connection`), responses with `Content-Length` or a closed connection as the end. Roughly
  400 to 600 lines with limits (request line and headers under 16 KiB, a header count limit, keep-alive with an idle timeout) and its
  own tests. The parsing happens before the secret is checked, so any local process can feed it bytes: a bug there is reachable from
  every local user. Estimate, not measured.
- **axum or another framework on hyper:** adds routing we do not need (axum and its own crates; not counted, not verified).
- **A synchronous server crate on threads** (not counted, not verified): new crates, and a second I/O model beside tokio.
- **libmpv's stream callbacks** (`mpv_stream_cb_add_ro`, a custom protocol whose bytes the core supplies): would avoid a socket on
  Windows and Linux. Not researched further: it does not help AVPlayer, and whether FFmpeg's HLS demuxer opens its sub-requests through
  mpv's stream layer (and so through a custom protocol) is not verified; mpv's `nested_io_open` (`demux/demux_lavf.c`, read
  2026-10-25 for ADR 0019) wraps FFmpeg's own `io_open`.

## 3. Apple

### AVAssetResourceLoaderDelegate and HLS segments
- `AVAssetResourceLoaderDelegate`, `resourceLoader(_:shouldWaitForLoadingOfRequestedResource:)`: "The resource loader object calls this
  method when assistance is required of your code to load the specified resource. For example, the resource loader might call this
  method to load decryption keys that have been specified using a custom URL scheme." The page does not say which HLS requests reach
  the delegate.
- Developer Forums thread 113063, replies marked "Apple Staff", "Systems Engineer" (June and July 2020):
  - "AVFoundation requires the ability to load HLS media segments directly in order to drive its bit rate adaptation."
  - "The only response to an AVAssetResourceLoadingRequest for a segment (.ts) file that AVPlayer will accept is a redirect to an HTTP
    URL. respondWithData will be rejected."
  - "Yes, the same limitation exists with any media segment (TS, fmp4, packed audio, WebVTT, whatever)."
  - "Until you get that you can look into using LL-HLS vended from an HTTP server running on the device." and "there's no particular
    reason that a web server needs to hit the disk. At the end of the day it's just an application that is listening on a TCP port and
    responding to HTTP GET requests."
  - A developer in the same thread (not Apple, July 2020): with `setRedirect`, "an authorization header (Authorization: XXX) added
    to the redirect request is not sent in the request." Not verified.
- Earlier evidence in `docs/notes/gateway-auth.md` (read 2026-10-22): `AVURLAssetHTTPHeaderFieldsKey` "is not a supported API"
  (Apple engineer, forum thread 20421), and "Video AirPlay is not supported when using a custom resource loader" (a 2015 forum post
  quoting Apple support; not verified).
- `AVURLAssetHTTPCookiesKey` (re-read today): "In HTTP Live Streaming, the system may issue many HTTP requests (for example, media,
  crypt key, variant index) to different paths or hosts. In these cases, HTTP requests won't contain any cookies that don't apply to
  the AVURLAsset URL." So cookie gateways work on AVPlayer without a relay or a resource loader; header and Basic gateways do not.

Conclusion: a resource loader can serve playlists (custom scheme) but not segments. Segments must come from an HTTP URL that AVPlayer
fetches itself, with no header Current can add. Header and Basic gateways on Apple therefore need an HTTP server the app runs: the
relay. Apple's own engineer names that route.

### App Transport Security and http://127.0.0.1
- `NSAppTransportSecurity`: "ATS requires that all HTTP connections made with the URL Loading System—typically using the URLSession
  class—use HTTPS."
- `NSAllowsArbitraryLoadsForMedia`: "disable App Transport Security restrictions for media loaded using the AVFoundation framework,
  without affecting your URLSession connections". So AVFoundation's loads are under ATS; this key needs a justification in App Store
  review ("Media streaming exceptions", `security/preventing-insecure-network-connections`).
- `NSAllowsLocalNetworking`: "controls whether App Transport Security (ATS) allows your app to connect to unqualified domains, `.local`
  domains, and IP addresses using IPv4 or IPv6." ... "In iOS 17, iPadOS 17, and macOS 14, ATS no longer allows connections to IP
  addresses by default. Add individual IP addresses and classless inter-domain routing (CIDR) ranges in the NSExceptionDomains
  dictionary." ... "The local networking exception tells newer versions of the OS to ignore the arbitrary loads key, and enable access
  to unqualified domains, `.local` domains, and IP addresses that they would otherwise restrict." It is not in the list of exceptions
  that need a justification.
- `NSExceptionDomains`: "In iOS 17, iPadOS 17, and macOS 14, you can use an IPv4 address, for example `192.168.42.63`".
- No page read says whether loopback (`127.0.0.1`, `localhost`) is treated differently from other IP addresses. **Not verified:**
  whether AVPlayer loads `http://127.0.0.1:<port>/...` without any key on macOS 14 or later. The proposal sets
  `NSAllowsLocalNetworking` (a plain-http LAN server played by AVPlayer directly needs it anyway), and M3 checks the relay URL with
  and without it.

### Local Network privacy and the macOS firewall
- TN3179, "Local network operations": "Listening for and accepting incoming TCP connections" does not require local network access;
  "Making an outgoing TCP connection" to a local network address does. A local network is "an IP network associated with a
  broadcast-capable network interface. Such interfaces include Wi-Fi and Ethernet". Loopback is not named. So listening on 127.0.0.1
  needs no Local Network permission; whether AVPlayer's connection to 127.0.0.1 counts as local network is not stated (not verified;
  expected not, since loopback is not a broadcast-capable interface).
- The application firewall's "accept incoming network connections" prompt: no current Apple document found. Older reports: a
  PostgreSQL mailing-list test on OS X 10.10 (2014, https://www.postgresql.org/message-id/11427.1414263156%40sss.pgh.pa.us) found that
  binding to 127.0.0.1 did not prompt while a link-local address did. In forum thread 6396 (July 2015) the Apple DTS engineer had no
  answer; another user wrote "I think that's caused by the firewall, and has nothing to do with sandboxing" (not Apple). **Not
  verified** on macOS 26; checked by hand in M3 with the firewall on.

### AirPlay
ADR 0008 (case 4 of `docs/notes/macos-video.md`): "Local files and loopback URLs fail on this receiver ... anything we proxy on
127.0.0.1 can't be AirPlayed as video there." ADR 0017 already refuses AirPlay and casting while the address in use has gateway
credentials, so a relay used only for those addresses takes nothing away.

## 4. Windows

- Microsoft Learn, "Filter Origin Audit Log", section "Query user default": "When an application binds to a socket but doesn't have a
  corresponding inbound rule to allow packets on that port, Windows generates a pop-up for the user to allow or deny the app to receive
  packets on the available network categories." The page does not say whether a socket bound to 127.0.0.1 only triggers it.
- Same page, "AppContainer loopback": drops "occur when localhost loopback isn't enabled properly for the Universal Windows Platform
  (UWP) app". Current is an unpackaged desktop app installed by Inno Setup (ADR 0009), not in an AppContainer, and the relay and libmpv
  run in the same process; this filter is not expected to apply (not verified).
- Same page, "Quarantine default": "blocks any new non-loopback inbound connections", so loopback is treated apart there.
- **Not verified:** that binding to 127.0.0.1 shows no Windows Security Alert. Widely reported, but no Microsoft page read says so.
  The Windows smoke run cannot see a prompt (CI has no desktop session that would show it); the owner checks on the PC in the
  implementation's Windows task, on a fresh user profile.

## 5. HLS details that matter for the relay

From RFC 8216 as quoted in `crates/jellyfin/src/hls.rs` (read 2026-10-25) and Jellyfin as read for ADR 0019:
- URIs are URI lines (variant playlists after `EXT-X-STREAM-INF`, segments after `EXTINF`) and `URI` attributes (`EXT-X-MEDIA`,
  `EXT-X-I-FRAME-STREAM-INF`, `EXT-X-KEY`, `EXT-X-MAP`, `EXT-X-SESSION-DATA`, `EXT-X-SESSION-KEY`, and later tags with `*-URI`).
- `EXT-X-BYTERANGE` and the `BYTERANGE` attribute of `EXT-X-MAP` carry no URI; the player turns them into `Range` requests on the
  segment's URI, which the relay forwards like any other `Range`.
- Jellyfin's `TranscodingUrl` is relative (`/videos/{itemId}/master.m3u8?...`), and its playlists name variants and segments
  relatively, with the master's query (and so `ApiKey`) copied into each (docs/notes/playback.md, docs/notes/mpv-engine.md).
- A relative URI resolves against the playlist's own URL. If the player fetched the playlist from the relay, a relative URI already
  points back at the relay; only absolute URIs need rewriting to reach it.
- Jellyfin's VOD playlists do not change during playback; a Live TV playlist does (re-fetched by the player). Rewriting on every
  fetch handles both; the current check only the first.

## 6. Design notes behind the proposal

### Why the player can hold nothing secret
With the relay, the engine gets `http://127.0.0.1:<port>/<route>/<path>`, no cookie, no header and (proposed) no `ApiKey`. Whatever a
playlist names, the player has no gateway credential to send, so FFmpeg's `ffio_copy_url_options` and its suffix cookie match stop
mattering. The relay itself never takes a host from the request: a route maps to one server, and the upstream URL is the selected
address's base plus the request's path. Same-origin enforcement is therefore structural, not a check on URLs.

The one secret the player does hold is the route token (128 bits from `getrandom`, already a workspace dependency). It is good for
one playback and only from this machine. `current-mpv`'s log redaction gets it as one of the `secrets`, as it gets the session token
today.

### Who can reach a loopback port
- Any process of any user on the machine can connect to 127.0.0.1:<port>. Binding to 127.0.0.1 keeps other machines out (a wildcard or
  LAN bind would not); the port is random (`bind 127.0.0.1:0`) and says nothing.
- Without the route token every request gets the same 404 after a constant-time comparison. 2^128 guesses are out of reach; the relay
  can also close a connection after a few wrong tokens.
- A web page in the user's browser can make requests to 127.0.0.1 (and with DNS rebinding, under another name). It cannot know the
  token. The relay also refuses any request whose `Host` is not `127.0.0.1:<port>` and any request with an `Origin` header (a player
  sends none), so a rebinding page gets nothing even by chance.
- A process that can read Current's memory can read the token, but it can read the gateway secrets too: no new exposure.
- Loopback traffic is not encrypted. Capturing it needs administrator rights on Windows and root on macOS (not verified as a source
  quote; the usual capture drivers need them), and such a user can read the keyring anyway.

### What a hostile playlist can and cannot do
A playlist is the server's answer, or a gateway's, or (with plain http accepted under ADR 0012) a network attacker's.
- It cannot send a gateway credential or the token anywhere: the player has neither, and the relay sends only to the server's origin.
- It can make the player fetch any URL it names directly. With the proposal's choice A, an off-origin URI fails the playlist instead.
  Under choice B the player would fetch it with no credentials: the user's IP address and the fact of playback reach that host.
- It can make the player request other paths on the server through the relay. The relay allows only GET and HEAD, and only under the
  route's path prefixes (`/Videos/{itemId}/`, `/Audio/{itemId}/` for the item playing), so it cannot reach `/Users`, `/Sessions` or a
  delete. Jellyfin's GET endpoints under `/Videos/{itemId}/` start or continue transcodes of that item, which the playback does anyway.
- It can be large or never end: the relay reads at most 4 MiB of a playlist (the current check's limit) before failing it.
- It can name many variants: the player decides what to fetch; the relay's connection limit bounds concurrency.

### Streaming without buffering
- hyper polls a response body only when the socket can take more, and reqwest's `chunk()` awaits the next upstream bytes: forwarding
  one chunk at a time keeps at most one chunk (typically up to 16 KiB per read, not measured) per connection in memory, and TCP flow
  control slows the server when the player stops reading (paused, buffer full). Back-pressure is end to end with no queue of our own.
- Range: `Range` and `If-Range` go upstream unchanged; `206`, `Content-Range`, `Content-Length`, `Accept-Ranges` come back. A seek in
  direct play is a new range request; an HLS seek is the player asking for other segments.
- Concurrency: one tokio task per connection, HTTP/1.1 keep-alive, a limit of 16 connections per relay (players open a few: FFmpeg one
  or two per stream, AVPlayer several; neither measured).
- Cancelling: when the player closes a connection, hyper drops the body, which drops the reqwest response and closes or frees the
  upstream stream. When a route is removed (stop, next item), its open connections are aborted at once.
- Timeouts: no total deadline for a body (a direct-play response lasts as long as the film); a 30 s read timeout (reqwest's
  `read_timeout`, matching `network-timeout=30` and `current-net`'s request timeout) and the controller's 60 s stall rule above it.

### Mid-play events
- Upstream 401, 403, a redirect, or HTML where media was expected: the relay answers the player with 502 and tells the controller
  `RelayRefused { route, shape }` (the gateway shape from `current-net`'s classifier). The controller then runs ADR 0017's 401 rule
  through the server (probe, then the token check of ADR 0015 as amended 2026-10-25) and fails with `GatewaySignInNeeded`,
  `SignedOut` or `Server { status }`. Today the core never sees these statuses (section 1).
- Connect or TLS failure: `current-net`'s roaming race runs (ADR 0014); a GET is retried once on the newly selected address within its
  deadline. With choice A of "roaming mid-play" the next request goes to the new address with that address's credentials (or none).
- `Set-Cookie` on media answers updates the address's gateway cookies through `current-net` (ADR 0017 choice 4), so rotating cookies
  keep working during playback. Today FFmpeg keeps rotated cookies to itself (ADR 0019 choice 5's consequence).
- Gateway sign-in again: the credentials in `current-net`'s `Gateway` are replaced; the relay reads a snapshot per request, so the
  next request carries the new cookies with no restart of the relay.

## 7. Not verified
- That binding 127.0.0.1 shows no prompt on Windows 11 or macOS 26 with the firewall on.
- That AVPlayer loads `http://127.0.0.1` under ATS with `NSAllowsLocalNetworking`, or without any key.
- That AVPlayer's connections to 127.0.0.1 need no Local Network permission.
- How many connections FFmpeg and AVPlayer open per HLS stream, and chunk sizes; the 16-connection limit is a guess to check.
- That Jellyfin answers HLS segment requests with the token in the header instead of `ApiKey` for every 12.2 segment endpoint (choice
  4 B). `AuthorizationContext.cs` reads both for every authorized request (ADR 0017's notes, read 2026-10-22); not run here.
- Whether a stream's 401 reaches the 401 rule today by another path than the one traced in section 1.
- hyper's behaviour under a slow or hostile local client beyond the limits its server builder offers (hyper 1.12.0
  `src/server/conn/http1.rs`: `max_buf_size`, `max_headers`, `header_read_timeout`, `keep_alive`): to be covered by the relay's own
  tests.
