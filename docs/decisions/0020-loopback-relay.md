# 0020: A loopback relay for streams behind a gateway

Status: accepted (2026-10-26); open choices decided by the owner 2026-10-26 (see "Decided by the owner"). From the loopback relay
spike. Amended 2026-10-29 (ADR 0025, W3): choice 1 widens to every stream for a platform engine; see "What the relay is".
Research, quotes and sources: `docs/notes/loopback-relay.md`. No product code was written.

## Context
Some servers sit behind an authentication gateway (ADR 0017): Basic, a sign-in portal's cookies, or extra headers such as a service
token. The core's own requests carry those credentials through `current-net`. The player fetches streams itself, and today:
- **Windows (libmpv, FFmpeg 9.0.2):** the core hands FFmpeg the cookies and headers per file (`stream-lavf-o`). FFmpeg copies
  `headers` into every HLS sub-request, to whatever host a playlist names, and matches a cookie's domain as a plain suffix of the
  host. So the core checks each HLS playlist before handing it over, and `current-mpv` refuses unchecked playlists and sets
  `access-references=no` otherwise. mpv fetches the playlists again itself, so this is a check, not enforcement (ADR 0019,
  "Playlists and headers"; `crates/jellyfin/src/hls.rs`, `crates/mpv/src/options.rs`).
- **macOS (AVPlayer, M3):** AVPlayer takes cookies (`AVURLAssetHTTPCookiesKey`) but has no supported API for request headers, so Basic
  and header gateways cannot play (ADR 0017).
- **The owner's choice (ADR 0019, 2026-10-25):** "check now, relay in M3". The roadmap now puts the relay before M3 and M4 (owner,
  2026-10-26).

What the research found (details and quotes in the notes):
- **A resource loader cannot serve HLS segments.** An Apple engineer (Developer Forums thread 113063, 2020): "The only response to an
  AVAssetResourceLoadingRequest for a segment (.ts) file that AVPlayer will accept is a redirect to an HTTP URL. respondWithData will be
  rejected", for "any media segment (TS, fmp4, packed audio, WebVTT, whatever)"; and as the alternative, "an HTTP server running on the
  device". A redirect carries no header Current adds (a developer's report in the same thread; not verified).
- **ATS covers AVFoundation's loads** (`NSAllowsArbitraryLoadsForMedia` exists to relax it), and since macOS 14 "ATS no longer allows
  connections to IP addresses by default"; `NSAllowsLocalNetworking` re-allows "IP addresses using IPv4 or IPv6" and needs no App Store
  justification. Whether loopback is treated apart is not documented (not verified).
- **Listening needs no Local Network permission** on Apple platforms (TN3179: "Listening for and accepting incoming TCP connections":
  not required). That a 127.0.0.1-only listener shows no firewall prompt on Windows or macOS is widely reported but not documented by
  Microsoft or Apple (not verified; checked by hand in the tasks).
- **hyper's HTTP/1.1 server is one feature away.** hyper 1.12.0 is already built for reqwest (`client`, `http1`, `http2`); its `server`
  feature adds one crate to the lock file, `httpdate` 1.0.3 (MIT OR Apache-2.0), shown by a scratch build with the workspace's lock.
- **The core can stream with what it has:** reqwest 0.13.5's `Response::chunk()` and `ClientBuilder::read_timeout` need no new feature.
  `ServerClient::send` today buffers the body and has one deadline for the whole request, so the relay needs a streaming send.
- **Today a gateway refusal mid-play is invisible to the core:** mpv's request fails, the controller sees an engine error and falls back
  or fails with `Engine { kind }` (`crates/playback/src/controller.rs`, `engine_failed`). A relay sees the status.

## Decision

### What the relay is
A small HTTP/1.1 server in the core, bound to `127.0.0.1` on a random port, that forwards a player's GET and HEAD requests to the
server's origin through that server's `ServerClient`, which adds the selected address's gateway credentials and the session token, and
streams the answer back. The player gets a URL such as `http://127.0.0.1:50123/<route>/Videos/<id>/stream?static=true&...` and holds
no credential.

It is used for every stream whose address has gateway credentials, on every platform (choice 1). Streams on an address without a
gateway keep today's direct URL (AirPlay keeps working for them on macOS).
Amended 2026-10-29 (ADR 0025, task W3, feat/platform-engine-bridge; needs the owner's approval): every stream for a platform engine
(the platform's own player behind `current-ffi`) goes through the relay as well, gateway or not, so the URL that crosses the bridge
holds the route's token only (`MediaServer::stream_url`'s `always_relay`). libmpv's streams keep this rule as it is.

### Routes and who may use it
- **A route per playback:** 128 random bits (`getrandom`, base64url) as the first path segment, mapped to one server, the path prefixes
  of the item playing (`/Videos/{itemId}/`, `/Audio/{itemId}/`) and a sink for the controller's events. Created when the controller
  loads a file, removed on stop, item change, re-plan and when the player closes; removing it aborts its open connections.
- **No host from the request, ever.** The upstream URL is the server's selected address (its base URL) plus the request's path and
  query. The relay accepts only origin-form request targets (`/...`); absolute-form targets, `CONNECT` and every method but GET and HEAD
  get 405 or 400. Same-origin enforcement is structural.
- **Refused before anything goes upstream:** a wrong route token (404, constant-time comparison), a path outside the route's prefixes
  (404), a `Host` other than `127.0.0.1:<port>` (400, against DNS rebinding), any `Origin` header (403; players send none), a request
  body (400).
- **Bound to 127.0.0.1 only,** never a wildcard or LAN address; the port is the OS's choice (`:0`). Other local processes and users can
  connect but cannot guess the token. TLS on loopback adds nothing: whoever can capture loopback traffic can read Current's memory.

### Requests and answers
- **Request headers forwarded:** `Range`, `If-Range`, `If-None-Match`, `If-Modified-Since`, `Accept`. Everything else from the player is
  dropped, `Cookie` and `Authorization` included. `current-net` adds the address's gateway credentials and `RequestAuth`'s token, as for
  every core request (ADR 0017, "Where the credentials go"), and its own `User-Agent`.
- **Response headers returned:** status, `Content-Type`, `Content-Length`, `Content-Range`, `Accept-Ranges`, `ETag`, `Last-Modified`,
  `Cache-Control`. `Set-Cookie` never reaches the player; `current-net` reads it into the address's cookies (ADR 0017 choice 4), so
  cookies a gateway rotates during playback are kept, which FFmpeg's private copy did not do (ADR 0019 choice 5's consequence).
- **Redirects:** none followed for streams; an upstream 3xx is a refusal (below). The relay itself never answers 3xx. TLS verification
  is `current-net`'s (OS verifier, ADR 0014), so it is the same for the player as for the core.
- **Streaming:** one chunk at a time from reqwest's `chunk()` into hyper's body; hyper polls only when the socket can take more, so a
  paused or full player slows the server through TCP flow control. No buffering beyond a chunk per connection, except playlists.
- **Concurrency and time:** a task per connection, HTTP/1.1 keep-alive, at most 16 connections per relay (to check on devices). No
  total deadline for a body; a 30 s read timeout upstream and 30 s for a client to send its headers; the controller's 60 s stall rule
  (ADR 0019) stays above both.
- **The request log** (ADR 0006): relay requests are logged only when they fail (status 400 and up, or a network error), so a two-hour
  HLS stream (about 1,200 requests) does not push everything else out.

### HLS playlists
- A response is a playlist when its `Content-Type` is an HLS type (`application/vnd.apple.mpegurl`, `application/x-mpegurl`,
  `audio/mpegurl`) or its path ends in `.m3u8`. The relay reads it whole (at most 4 MiB, today's limit), rewrites it and sends it with a
  `Content-Length`. Everything else streams unchanged.
- **Rewriting:** every URI line and every `URI` / `*-URI` attribute, in any tag (today's parser in `hls.rs`, moved), is resolved against
  the playlist's upstream URL. One on the server's origin and inside the route's prefixes becomes an absolute path on the relay
  (`/<route>/...`), with `ApiKey`/`api_key` taken out (choice 4). Anything else fails the playlist (choice 5): the relay answers 502 and
  tells the controller, which fails with `PlaylistElsewhere`.
- Byte ranges (`EXT-X-BYTERANGE`, `EXT-X-MAP`'s `BYTERANGE`) carry no URI and become the player's `Range` requests, forwarded as above.
  Keys (`EXT-X-KEY`) and maps are rewritten like any `URI`.
- Every fetch is rewritten, so a playlist that changes (Live TV later) is enforced each time, which the check could not do.
- **ADR 0019's playlist check goes** (choice 8): `MediaServer::check_playlist`, `EngineFile::playlist_checked` and
  `JellyfinClient::check_playlist_origin` are removed. `current-mpv` keeps refusing a load with credentials and an unchecked playlist and
  keeps `access-references=no` for any other load with credentials, as a guard that no relayed load triggers (none carries
  credentials).

### Direct play
Range requests pass through; a seek is a new range request, a stop or seek closes the old connection and the relay drops the upstream
response. Jellyfin's static stream answers `206` with `Content-Range` (ADR 0019's direct-play URL), and nothing in the relay depends on
the container.

### Lifecycle
- **One relay per app** (choice 7), owned by `current-app`. The listener starts when the first route is created and stops when the last
  is removed, so nothing listens while nothing plays; each start gets a new port.
- **Address roaming mid-play** (choice 6): the relay forwards through the server's `ServerClient`, which sends each request to the
  address selected now, with that address's credentials only (ADR 0017, per address). A failed GET is retried once on a newly selected
  address within its time (ADR 0014). The identity check guarantees it is the same server, so direct play continues at the next range
  request and an HLS transcode at the next segment, with the same `PlaySessionId`.
- **Refusals mid-play:** an upstream 401, 403, redirect, or an HTML page where media was expected is answered with 502 to the player and
  reported to the controller as `RelayRefused { shape }` (`current-net`'s gateway shape, no URL). The controller runs ADR 0017's 401 rule
  (probe with the address's gateway credentials, then ADR 0015's token check, as amended 2026-10-25) and fails with
  `GatewaySignInNeeded` or `SignedOut`; 404 is `NotFound`, other statuses `Server { status }`, a network failure that roaming cannot fix
  `Network`. Each is reported once per route.
- **Gateway expiry:** as above, `GatewaySignInNeeded`; the session and token are kept. After the user signs in to the gateway again,
  playing the item again resumes at the reported position. The relay reads `current-net`'s credentials per request, so no restart is
  needed.
- **A relay that cannot start** (no port): `PlaybackProblem::LocalRelay`, new, shown as "Current could not start its local stream
  connection". No fallback to giving the player credentials.

### Where it lives
- **`current-relay`, a new crate** (choice 3): listener, routes, forwarding, playlist rewriting, its events. It depends on
  `current-net` (an `Upstream` trait that `ServerClient` implements, so tests can use a fake) and on hyper's server (choice 2). It knows no
  service: Stash scenes (later) use it unchanged.
- **`current-net`:** `ServerClient::send_stream`: the same address selection, gateway layer, `RequestAuth`, roaming retry and
  `Set-Cookie` updates as `send`, but no redirects, no total deadline, a read timeout, the header lists above, and a body read chunk by
  chunk. Logged per "The request log" above.
- **`current-playback`:** `StreamAuth` loses `cookies` and `headers` and keeps `secrets` (now the route token, and the session token for
  unrelayed streams); `EngineFile::playlist_checked` and `MediaServer::check_playlist` go; `MediaServer` gains `stream_url(...)` returning
  either the direct URL or a relayed one; the controller handles `RelayRefused` events and `LocalRelay`.
- **`current-app`:** owns the relay, creates and removes routes with the player, joins relay events to the controller.
- **`current-mpv`:** no change to code; it simply never gets credentials for a relayed stream. `max_redirects=0` and `tls-verify=yes`
  stay for unrelayed streams. The Linux roadmap item about FFmpeg before 9.0 following redirects with gateway headers no longer applies
  to gateway streams (FFmpeg has no header to carry); it still applies to the session token in unrelayed URLs, which needs no header.
- **`current-ffi` and the UIs:** nothing. No relay URL crosses the bridge (the engine is in the core).
- **AVPlayer (M3):** the engine is given the relay URL for gateway streams; the app sets `NSAllowsLocalNetworking`; no resource loader.
- **Images and subtitles:** no change. The core fetches image bytes (ADR 0018 choice A) and external subtitles (ADR 0019) itself.

## Decided by the owner (2026-10-26)
"All as recommended."
1. Where the relay applies: **A**, every stream on an address with gateway credentials, on every platform.
2. The HTTP server: **B, hyper's server.** Approved dependencies, at exact versions: `hyper` 1.12.0 (features `server`, `http1`),
   `hyper-util` 0.1.21 (feature `tokio`), `http-body-util` 0.1.5 and `httpdate` 1.0.3, plus `http` and `bytes` if the code names them
   directly (at the versions in the lock file: `http` 1.5.0, `bytes` 1.12.1).
3. Which crate: **A**, a new `current-relay`.
4. The session token in player URLs: **B**, the relay strips `ApiKey` and `current-net` adds the token upstream.
5. A playlist URI off the server's origin: **A**, the playlist fails (`PlaylistElsewhere`).
   **Amended 2026-10-26 (owner)**, from task 4's live tests on libmpv, so that nothing on a relayed HLS route can name a host for
   the player to open directly:
   - **HLS loads open with `demuxer=lavf`** (`FileRequest::lavf_only`), relayed ones included, so mpv's own demuxers are skipped.
     Reason: relayed HLS keeps `access-references` on, and with it mpv's playlist and EDL readers take a `[playlist]`,
     `[Reference]`, `[InternetShortcut]` or `# mpv EDL v0` body and open the URLs it names (mpv v0.41.0 `demux/demux_playlist.c`,
     `demux/demux_edl.c`, read 2026-10-26). Seen on libmpv: a `pls` body on a relayed HLS route made mpv connect to the port it
     named.
   - **The relay refuses an HLS route's entry body unless it is an `#EXTM3U` playlist** (`RouteConfig::hls_entry`, the URL the
     route was made for; anything else fails as `PlaylistUnreadable`, shown as `PlaylistElsewhere`). Reason: FFmpeg's own probing
     would still take another manifest there, such as a DASH MPD.
   - **A 206 must be the part asked for:** its `Content-Range` must start where the request's `Range` does, and a 206 to a request
     without `Range` fails (`RelayEvent::BadRange`, a 502 to the player, `Server { status: 502 }` to the user). Reason: the relay
     sniffs a body for `#EXTM3U` only from the resource's first byte, and FFmpeg accepts any start in `Content-Range` (FFmpeg 9.0.2
     `libavformat/http.c`, `parse_content_range`, read 2026-10-26), so a playlist sent as if from its second byte would pass
     unrewritten.
   Consequence: external subtitles keep working on HLS. `demuxer-lavf-format=hls` is not forced, because every lavf demuxer of the
   file reads it, the one `sub-add` opens included (mpv v0.41.0 `demux/demux_lavf.c`, `player/loadfile.c`), and FFmpeg's `hls`
   demuxer refuses a subtitle (seen in task 4's test before the change).
6. Roaming during playback: **A**, relayed streams follow the selected address with its own credentials.
7. The relay's lifetime: **A**, one per app, listening only while a route exists.
8. The playlist check: **A**, removed when the relay lands; `current-mpv`'s refusal kept as a guard.

The amendments below are written into the other ADRs by the implementation tasks, not by this record.

The options as they were put to the owner follow.

## Open choices (as proposed)

1. **Where the relay applies.** A (recommended): every stream on an address with gateway credentials, on every platform. B: every
   stream, gateway or not: one path for all, but AirPlay of video stops working everywhere on macOS (loopback URLs fail on receivers,
   ADR 0008) and every stream takes an extra hop. C: macOS only; Windows keeps the check and FFmpeg's credentials: the relay is built
   anyway, and Windows keeps a check that cannot enforce. D: no relay; the check on Windows and `AVAssetResourceLoaderDelegate` on
   macOS: Apple's engineer says segments can only be redirected to an HTTP URL, so Basic and header gateways still cannot play on
   macOS; only cookie gateways would, and those need neither. **Recommendation: A.** Consequence: two stream paths (direct and relayed),
   chosen per address, both tested.

2. **The HTTP server.** A: a hand-written HTTP/1.1 subset over tokio: no dependency, about 400 to 600 lines plus tests (estimate), and
   a parser that every local process can feed before the token is checked. B (recommended): hyper 1.12.0's server (`server`, `http1`),
   with `hyper-util` 0.1.21 (`tokio`, for `TokioIo`) and `http-body-util` 0.1.5 as direct workspace dependencies, all already in the
   lock file, plus `httpdate` 1.0.3 (MIT OR Apache-2.0), the only new crate. Needs the owner's approval for each direct dependency
   (CLAUDE.md). C: a framework on hyper (axum or similar): routing we do not need, more crates (not counted). **Recommendation: B**:
   the parser reqwest already trusts, the smallest addition, and limits (`max_buf_size`, `max_headers`, `header_read_timeout`) built in.
   Consequence: `deny.toml` and ADR 0003 unchanged (MIT and Apache-2.0 are allowed; ADR 0003 lists only exceptions); the
   approvals are recorded in the implementing PR, with `http` and `bytes` too if the code names them directly (both already in the lock).

3. **Which crate.** A (recommended): a new `current-relay` (hyper's server stays out of `current-net`, and the relay is tested on its
   own). B: a module in `current-net` ("HTTP client per server" becomes client and relay). C: inside `current-app` (closest to its user,
   hardest to test alone). **Recommendation: A.** Consequence: CLAUDE.md's layout and ADR 0002's target layout gain a crate.

4. **The session token in player URLs.** A: unchanged: `ApiKey` stays in every URL the player holds, as today (ADR 0019 choice 3).
   B (recommended): the relay strips `ApiKey`/`api_key` from the URLs it gives the player and from rewritten playlists, and
   `current-net`'s `RequestAuth` adds the token upstream as for every core request (header placement per ADR 0017). The player then holds
   only the route token, good for one playback and only from this machine. That Jellyfin 12.2 takes the header on every stream and
   segment endpoint is read from `AuthorizationContext.cs` (ADR 0017's notes), not run: the relay's tests against the stand-in, and a
   device check, confirm it. **Recommendation: B.** Consequence: unrelayed streams keep `ApiKey`; the redaction of ADR 0019 stays for
   them.

5. **A playlist URI off the server's origin.** A (recommended): the playlist fails (`PlaylistElsewhere`), as with today's check.
   B: leave it as it is; the player fetches it directly with no credentials (nothing secret leaks, but the user's address and the fact
   of playback reach that host, and the URL may hold what the server put in it). **Recommendation: A.** Consequence: a server whose
   playlists point at a CDN cannot play through a gateway; none is known for Jellyfin.

6. **Roaming during playback.** A (recommended): relayed streams follow the selected address, with that address's credentials. B: pin
   each route to the address selected at start, as ADR 0014 says for streams today; a switch fails the stream and the user plays again.
   **Recommendation: A**, since the relay makes it free and the identity check makes it safe. Consequence: an amendment to ADR 0014
   ("Playback streams"); unrelayed streams keep B until a later task.

7. **The relay's lifetime.** A (recommended): one per app, listening only while a route exists. B: one per player (several players
   later, M5, would each open a port). C: one per app, listening from start to exit (a port open while nothing plays).
   **Recommendation: A.**

8. **The playlist check.** A (recommended): removed when the relay lands, `current-mpv`'s refusal kept as a guard. B: kept as well (a
   second fetch of every playlist before play, for no added protection). **Recommendation: A.**

## Amendments to merged ADRs (approved with the choices; written by the tasks)
- **ADR 0019, "Gateways, tokens and the player":** gateway cookies and headers no longer go to FFmpeg; streams on gateway addresses go
  through the relay; "Playlists and headers" is replaced by this record (the check, `playlist_checked` and `check_playlist` removed);
  with choice 4 B, `ApiKey` is not in relayed URLs; refusals on streams reach the 401 rule through `RelayRefused`; choice 5's
  consequence (rotated cookies not read back) no longer holds for relayed streams; `PlaybackProblem` gains `LocalRelay`.
- **ADR 0017, "The player":** the libmpv paragraph becomes "through the loopback relay (ADR 0020)"; "headers have no supported API, so
  header and Basic gateways on Apple need a resource loader or a loopback relay in the core, decided in M3" becomes "use the loopback
  relay (ADR 0020); a resource loader cannot serve HLS segments". AirPlay and casting: unchanged.
- **ADR 0014, "Playback streams":** with choice 6 A, relayed streams follow the selected address.
- **ADR 0008, "Consequences":** the app sets `NSAllowsLocalNetworking`; the existing note that anything proxied on 127.0.0.1 cannot be
  AirPlayed stands, and ADR 0017 already refuses AirPlay on gateway addresses.
- **ADR 0006:** relay requests enter the request log only when they fail.
- **ADR 0002 and CLAUDE.md's layout:** with choice 3 A, `crates/relay` (`current-relay`).
- ADR 0010 needs no change (fonts and subtitle streams are fetched by the core).

## Task split for implementation
Each task is its own branch and PR (CLAUDE.md). 1 and 2 can run in parallel; 3 needs both; 4 and 5 need 3; 6 is M3.

1. **`current-net`: streaming send** (feat/net-stream-send). `ServerClient::send_stream` with the header lists, no redirects, read
   timeout, roaming retry, `Set-Cookie` updates, failure-only logging. Done when: tests on 127.0.0.1 stand-ins show a 50 MB body
   streamed with bounded memory (the stand-in pauses and the client's reads stop), `Range` and `If-Range` forwarded and `206` with
   `Content-Range` returned, a cookie set mid-stream updates the gateway, a redirect is a refusal with nothing followed, a dropped
   response closes the upstream connection, and no log line holds a token, cookie or header value.
2. **`current-relay`: the relay** (feat/loopback-relay). Listener, routes, refusals, forwarding over
   an `Upstream` trait, playlist rewriting (the parser moved from `hls.rs`), events, connection limit, cancellation. Done when: tests
   with a fake upstream cover a wrong token, a path outside the prefixes, a foreign `Host`, an `Origin` header, other methods, an
   absolute-form target and a body (all refused, nothing upstream); playlists with relative, absolute same-origin, off-origin, `URI=`
   in known and unknown tags, keys, maps and byte ranges (rewritten or refused as above, `ApiKey` gone); 16 concurrent range requests
   byte-exact; a slow reader keeps memory bounded; removing a route aborts its connections; the listener stops with the last route.
3. **`current-app` and `current-playback`: relayed playback** (feat/relayed-playback). The relay in `App`, routes per playback,
   `stream_url`, `StreamAuth` slimmed, `RelayRefused` into the 401 rule, `LocalRelay`, roaming mid-play, the playlist check removed;
   the jellyfin stand-in gains a gateway mode in front of its stream endpoints if it has none. Done when: bridge tests with the test
   engine play through a cookie gateway, a Basic gateway and a header gateway on the stand-in; an expired cookie mid-play gives
   `GatewaySignInNeeded` with the session kept; a Jellyfin 401 gives `SignedOut` only after the token check; an address switch mid-play
   continues on the second address with its own credentials only (checked on the stand-in's received requests); an address without a
   gateway gets a direct URL.
4. **`current-mpv` live tests through the relay** (test/mpv-relay). Done when, on the Windows CI runner with the D6 libmpv: direct play
   and an HLS stream play through the relay in front of a gateway stand-in, FFmpeg's requests carry no cookie or header (checked on the
   relay's received requests), a hostile playlist naming another port is refused and that port receives nothing, a seek opens a new
   range, and stopping closes every upstream connection. The amendments to ADR 0019, 0017, 0014 and 0006 are written here.
5. **Windows on the owner's PC** (by hand, with the Windows player). Done when the owner has played through the cookie and Basic test
   gateways (direct play, remux, transcode, seeks, a pause of several minutes), switched networks mid-play, let a gateway session
   expire, and seen no Windows Security Alert on a fresh user profile with the firewall on.
6. **AVPlayer through the relay** (M3, with the macOS engine). Done when: AVPlayer plays direct and HLS through the relay with a Basic
   and a header gateway; ATS behaviour checked with and without `NSAllowsLocalNetworking`; no firewall or Local Network prompt appears
   with the macOS firewall on; ADR 0008's amendment written.

## Consequences
- The player never holds a gateway credential (and, with choice 4 B, no session token) for a gateway address; the same-origin rule is
  enforced on every request and every playlist fetch, not checked once.
- Basic and header gateways become playable on Apple platforms; cookie gateways keep working everywhere, with rotation during play.
- Refusals during playback reach the 401 rule with the real status, so "Sign in to the gateway again" appears instead of an engine error.
- The core runs a local listener while gateway streams play: one more thing to keep safe, with its own tests.
- Two stream paths exist (direct for addresses without a gateway, relayed with one).
- One new crate in the lock file (`httpdate`) and three to five new direct dependencies already in it (hyper, hyper-util,
  http-body-util; `http` and `bytes` if named directly), approved by the owner (2026-10-26).
- Not verified: firewall prompts on Windows and macOS, ATS for `http://127.0.0.1` with and without `NSAllowsLocalNetworking`,
  connection counts per player, and Jellyfin's header token on segment endpoints. Each has a task above.

## Sources (read 2026-10-26)
Quotes and paths in `docs/notes/loopback-relay.md`.
- This repository at `d82a18c`: `crates/jellyfin/src/hls.rs`, `crates/mpv/src/options.rs`, `crates/playback/src/`, `crates/app/src/player/`,
  `crates/net/src/`, `Cargo.lock`, `deny.toml`.
- hyper 1.12.0 (`Cargo.toml`, `src/server/conn/http1.rs`), hyper-util 0.1.21 (`Cargo.toml`, `src/rt/mod.rs`), reqwest 0.13.5
  (`src/async_impl/response.rs`, `src/async_impl/client.rs`), http-body-util 0.1.5, httpdate 1.0.3; a scratch build with the
  workspace's lock (notes, section 2).
- Apple: `AVAssetResourceLoaderDelegate`, `resourceLoader(_:shouldWaitForLoadingOfRequestedResource:)`, `AVURLAssetHTTPCookiesKey`,
  `NSAppTransportSecurity`, `NSAllowsLocalNetworking`, `NSAllowsArbitraryLoadsForMedia`, `NSExceptionDomains`, "Preventing Insecure
  Network Connections", TN3179; Developer Forums threads 113063 and 6396.
- Microsoft Learn, "Filter Origin Audit Log" (updated 2026-10-08).
- RFC 8216 (as quoted in `hls.rs`, read 2026-10-25); ADRs 0003, 0006, 0008, 0014, 0015, 0017, 0018, 0019.
