# Server APIs, discovery and networking (from platform notes)

> Reference only, copied unchanged from the earlier Kotlin Multiplatform project (`skeksik/Current` at `66452b2`, 2026-10-22).
> Facts carry the dates they were checked on. ADR numbers, module names and "the app" refer to that project. Nothing here is a
> decision for this repo; see `README.md` in this folder.

## Jellyfin 12.x (checked 2026-10-18 against a 12.2 server, its OpenAPI and the v12.2 source)
- Versioning: the release after 10.11 is 12.0 (2026-09-07); tags are `v12.0`..`v12.2`, servers report `12.2.0`.
  Sources: https://jellyfin.org/posts/jellyfin-release-12.0/ , https://github.com/jellyfin/jellyfin/releases/tag/v12.2
- Spec: https://api.jellyfin.org/openapi/stable/jellyfin-openapi-12.2.json ; every server serves its own at `/api-docs/openapi.json`.
  Its security scheme does not describe the `MediaBrowser` header format, so generated clients get auth wrong.
- Auth: only `Authorization: MediaBrowser Client="..", Device="..", DeviceId="..", Version="..", Token=".."` (key names are case-sensitive,
  values quoted and URL-encoded) and the `ApiKey` query parameter. `X-Emby-Token`, `X-MediaBrowser-Token`, `X-Emby-Authorization` and `api_key`
  are off by default since 12.0 (`EnableLegacyAuthorization`, https://github.com/jellyfin/jellyfin/pull/15559); live they return 401.
- Sign-in: `POST /Users/AuthenticateByName` `{Username, Pw}` with the header minus `Token` (400 if Client, Device, DeviceId or Version is
  missing) returns `AccessToken`, `User.Id`, `ServerId`. Signing in again with the same DeviceId and user revokes the previous token, so keep
  one random DeviceId per install. Sign-out: `POST /Sessions/Logout`.
- Quick Connect: `GET /QuickConnect/Enabled`; `POST /QuickConnect/Initiate` returns a 6-digit code (valid 10 minutes) and a secret; poll
  `GET /QuickConnect/Connect?secret=`; finish with `POST /Users/AuthenticateWithQuickConnect {Secret}`. A signed-in client can approve a
  code with `POST /QuickConnect/Authorize?code=`.
- Browsing (the user comes from the token; `userId` is optional): `/UserViews`, `/Items`, `/Items/Latest` (bare array), `/UserItems/Resume`,
  `/Shows/NextUp`, `/Shows/{id}/Seasons`, `/Shows/{id}/Episodes`, `/Items/{id}`, `/Items/{id}/Similar`, `/Genres`, `/Search/Hints`.
  The `/Users/{userId}/Items...` routes left the spec in 10.9; they still answer as hidden routes, do not use them.
  List parameters are comma-separated except `genres`, `tags`, `studios` (`|`); values the server cannot parse are dropped silently;
  pass `recursive` explicitly (12.x changed its default). Played/favorite: `POST|DELETE /UserPlayedItems/{id}`, `/UserFavoriteItems/{id}`.
- Images: `/Items/{id}/Images/{type}` needs no auth; with `tag=` the response is cached as immutable. Blurhashes: `ImageBlurHashes[type][tag]`.
- Provider ids: `ProviderIds["Tmdb"]` is a string and may be missing (most movies on the test server have none), so joins with Seerr cannot rely on it alone.
- JSON: PascalCase by default, camelCase with `Accept: application/json; profile="CamelCase"`. Ids are 32 hex characters without dashes.
  Nulls are usually omitted; enums gain values between versions (parse leniently). Ticks are 10,000,000 per second; `0001-01-01` means unset.
- Playback: `POST /Items/{id}/PlaybackInfo` needs a DeviceProfile (without one there is no `TranscodingUrl`), and `MediaSourceId` for audio and
  subtitle choices to apply. `SupportsDirectStream` equals `SupportsDirectPlay` in practice. Direct play:
  `/Videos/{id}/stream.{container}?static=true&mediaSourceId=..&deviceId=..&ApiKey=..` (byte ranges). HLS: use `TranscodingUrl` as given
  (relative, already contains `ApiKey`; the HLS routes are hidden from the spec since 12.0). Media URLs authenticate with `ApiKey`;
  trickplay tiles need it, item images do not.
- Reporting: `POST /Sessions/Playing`, `/Sessions/Playing/Progress` about every 10 s, `/Sessions/Playing/Stopped`, and
  `/Sessions/Playing/Ping?playSessionId=` while paused. The server stops an HLS transcode after 60 s without progress or ping.
  `Stopped` without `PositionTicks` marks the item fully watched.
- PlaybackInfo with a device profile (checked live 2026-10-19, read-only with an API key; ADR 0011):
  - An API key works as `Token` in the `MediaBrowser` header. `/UserViews`, `/Items` and `PlaybackInfo` then need the user passed
    (`userId`, `UserId`).
  - A non-default `AudioStreamIndex` keeps `SupportsDirectPlay: true`, although a browser plays only a file's default track, so a web
    client turns direct play off itself to switch audio.
  - The transcode reasons are in the `TranscodingUrl` query (`TranscodeReasons=`); the media source JSON had none.
  - HLS profile:
    - With an fMP4 HLS transcoding profile (`Container: mp4`) whose `VideoCodec` lists `h264,hevc`, HEVC sources that need only an audio
      transcode keep their video (`TranscodeReasons=AudioCodecNotSupported`).
    - With MPEG-TS and H.264 only, they were re-encoded.
  - Device profile conditions (checked live 2026-10-19):
    - A `CodecProfile` condition `VideoRangeType EqualsAny SDR|...` sends HDR10 sources to a tone-mapped transcode
      (`VideoRangeTypeNotSupported`).
    - `Width LessThanEqual 1920` caps the transcode (`MaxWidth=1920`, `VideoResolutionNotSupported`). Codec conditions also travel
      in the `TranscodingUrl` (`hevc-rangetype=`, `hevc-videobitdepth=`).
  - HLS codec lists:
    - In the fMP4 HLS profile, the first video and audio codec is what the server encodes to; any listed codec is copied when the
      source has it. E-AC-3 was copied with `aac,ac3,eac3,...`.
    - A source with more channels than `MaxAudioChannels` is transcoded anyway (`AudioChannelsNotSupported`).
  - `AudioStreamIndex` is ignored without `MediaSourceId` (the server then uses the default track).
  - Subtitles:
    - ASS, with a profile listing ASS as `Encode`, came back with `DeliveryMethod: Encode` and `SubtitleMethod=Encode` in the URL
      (burn-in).
    - SubRip, with only `vtt` as `External`, came back as an external `.vtt` `DeliveryUrl` (carrying `ApiKey`), under direct play too.
    - The test server had no PGS file, so PGS burn-in was not seen live.
- Subtitles: delivery `Encode`, `Embed`, `External`, `Hls`, `Drop`; external files at
  `/Videos/{id}/{mediaSourceId}/Subtitles/{index}[/{startTicks}]/Stream.{vtt|srt|ass|ssa|ttml|json}`.
- Intro/outro: `/MediaSegments/{id}`, but core ships no segment provider (a plugin must supply them), so expect none.
  Trickplay: the `Trickplay` field, `/Videos/{id}/Trickplay/{width}/tiles.m3u8` and `{index}.jpg`.
- CORS: `ServerConfiguration.CorsHosts` (default `["*"]`, not in the UI or docs) allows any origin without credentials. Live preflights return
  204 with `access-control-allow-origin: *` and echo the requested methods and headers, so a browser app on another origin works with header auth.
  Source: https://jellyfin.org/docs/general/post-install/networking/ (does not mention CORS); v12.2 source.

## Seerr (checked 2026-10-18 against upstream https://github.com/seerr-team/seerr (develop) and a live server)
- Spec: https://raw.githubusercontent.com/seerr-team/seerr/refs/heads/develop/seerr-api.yml . Reference: https://docs.seerr.dev/api/seerr-api/ .
- Requests are validated against the spec before any route runs (`express-openapi-validator` in `server/index.ts`): unknown query parameters
  get 400, unknown paths 404, no session 401. Send only documented parameters.
- Session: cookie `connect.sid`, 30 days from sign-in, HttpOnly, SameSite Lax (Strict when CSRF protection is on), `secure: auto` (Secure only
  when Seerr itself sees HTTPS, see its trust-proxy setting). Source: `server/index.ts`.
- Sign-in (`server/routes/auth.ts`): `POST /api/v1/auth/jellyfin {username, password}`, `POST /api/v1/auth/local {email, password}`, or Jellyfin
  Quick Connect through Seerr (`POST /auth/jellyfin/quickconnect/initiate` returns code + secret, `GET .../check?secret=`,
  `POST .../authenticate {secret}`). Login responses are partial: call `GET /auth/me` after. `POST /auth/logout`.
  A client already signed in to Jellyfin can approve Seerr's Quick Connect code itself (Jellyfin `POST /QuickConnect/Authorize?code=`), so it
  never stores a password and can renew the session silently: see "Seerr 3.5.0: sign-in with the app's Jellyfin session".
- No per-user API tokens. The global API key (`X-Api-Key`) acts as admin and, with `X-API-User: <id>`, as any user
  (`server/middleware/auth.ts`): never put it in a client.
- `permissions` is a JSON number bitmask: use `Long`.
- CORS: none, and OPTIONS preflights fail (405 from the validator). A browser app must be served on the same origin as Seerr's API
  (reverse proxy) or Seerr needs CORS support (server change). TMDB's image CDN sends `Access-Control-Allow-Origin: *`; `/imageproxy` needs no auth.
- Shapes: catalogue lists `{page, totalPages, totalResults, results}`; database lists `{pageInfo: {pages, pageSize, results, page}, results}`
  with `take`/`skip`. Media records carry `jellyfinMediaId` / `jellyfinMediaId4k`; `mediaUrl` is a web link, not a stream (`server/entity/Media.ts`).
  TMDB image paths are relative: prefix `https://image.tmdb.org/t/p/<size>`.
- Status values (`server/constants/media.ts`): request 1 pending, 2 approved, 3 declined, 4 failed, 5 completed; media 1 unknown, 2 pending,
  3 processing, 4 partially available, 5 available, 6 blocklisted, 7 deleted.
- `GET /api/v1/settings/discover` (the configured Discover rows) is readable by any signed-in user (`server/routes/index.ts`).
- Gotchas: error bodies come in several shapes (`{message}`, `{message, errors}`, `{status, error}`, `{error}`); `POST /request` can answer
  202 `{message}` instead of a request; `POST /user/{id}/settings/main` clears fields left out (send the whole object); push is Web Push only.

## Stash v0.31.1 (checked 2026-10-18 against a live server, introspection and the v0.31.1 source, https://github.com/stashapp/stash/tree/v0.31.1)
- Auth: `ApiKey` header or `?apikey=`. One key per server: full admin, no scopes, no expiry, no users. Without credentials, extensionless
  media paths redirect (302) to `/login` and `/graphql` or paths with an extension return 401. HEAD returns 405. Docs: https://docs.stashapp.cc/api/
- **The server copies its API key into every stream URL it returns** (`paths.stream`, `sceneStreams[].url`), however the caller authenticated
  (`internal/api/urlbuilders/scene.go`). A client that holds Stash stream URLs holds the admin key; they end up in player logs and history.
- Returned URLs are built from request headers (`X-Forwarded-*`) or `external_host`: rebase them onto the configured server origin.
- Scenes: `findScene(id)` returns null for an unknown id; for batches use aliased `findScene` (one missing id fails `findScenes(ids:)`).
  `files[0]` is the primary file every stream uses; choose routes from its `format`, `video_codec`, `audio_codec`, `width`, `height`, never from
  `sceneStreams[].mime_type` ("Direct stream" and "MKV" always say `video/mp4`). `date` may be partial (`YYYY`, `YYYY-MM`); `captions` is null
  when empty; `files[].size` is Int64.
- Streams: `/scene/{id}/stream` serves the raw primary file with byte ranges. Transcodes: `stream.m3u8` (H.264 + AAC in MPEG-TS, 2 s segments,
  seekable), `stream.mp4` and `stream.webm` (no byte ranges, seek with `?start=`), `stream.mpd`, each with
  `?resolution=ORIGINAL|FOUR_K|FULL_HD|STANDARD_HD|STANDARD|LOW`; `stream.mkv` (video copied, audio to Opus) for Matroska sources only.
- Lookup by stash-box id: `scene_filter: {stash_id_endpoint: {endpoint, stash_id, modifier: EQUALS}}` (the endpoint must match exactly);
  `stash_ids_endpoint` for batches.
- Resume and plays, as the web player does (`ui/v2.5/src/components/ScenePlayer/track-activity.ts`): `sceneSaveActivity(id, resume_time,
  playDuration)` every 10 s and on pause/stop (`playDuration` is added, so send seconds since the last save), `sceneAddPlay(id)` once per session.
- Seek thumbnails: `paths.vtt` + `paths.sprite`; chapters: `scene_markers` or `/scene/{id}/vtt/chapter`; captions: `paths.caption` + `?lang=&type=` (WebVTT).
- CORS: allow-all, hard-coded (`access-control-allow-origin: *`, methods POST, headers `Apikey, Content-Type`, no credentials); no setting.
- GraphQL: send `Accept: application/json`; validation errors are HTTP 422.

## Jellyfin 12.2: sign-in shapes (checked 2026-10-20 in the 12.2 OpenAPI spec, https://api.jellyfin.org/openapi/stable/jellyfin-openapi-12.2.json, and live)
- Live, 2026-10-20, against a 12.2.0 server behind HTTPS (`LiveJellyfinSmokeTest`, Quick Connect approved by hand in the web UI):
  - The address typed without a scheme resolved to `https`, and `/System/Info/Public` answered without credentials.
  - `GET /QuickConnect/Enabled`, `POST /QuickConnect/Initiate` (with the `MediaBrowser` header, no token), then `GET /QuickConnect/Connect`
    polled every 3 s until approved, then `POST /Users/AuthenticateWithQuickConnect`: signed in, and the answer's `ServerId` matched
    the probed `Id`.
  - `GET /Users/Me` with the token answered 200, again from a restored session. `POST /Sessions/Logout` answered 204, after which the same
    token got 401 on `/Users/Me`: logout ends the session on the server.
  - The request log showed paths with `secret=***` and the server key, never the host or a token.
  - Password sign-in, run by the owner with their own credentials: `POST /Users/AuthenticateByName` answered 200 with a token, then the
    same session checks and sign-out as above passed.
- `POST /Users/AuthenticateByName`, body `AuthenticateUserByName {Username, Pw}` (both nullable strings), answers 200
  `AuthenticationResult {User: UserDto, SessionInfo: SessionInfoDto, AccessToken, ServerId}` (all nullable) or 503. No security
  scheme in the spec; the `MediaBrowser` header without `Token` carries the client and device (see "Jellyfin 12.x").
- `UserDto`: `Id` (string, not nullable), `Name`, `ServerId`, `PrimaryImageTag`, `HasPassword`, `Policy: UserPolicy` (nullable).
  `UserPolicy.IsAdministrator` is a boolean; the spec marks `AuthenticationProviderId` and `PasswordResetProviderId` required.
- Quick Connect:
  - `GET /QuickConnect/Enabled` answers a JSON boolean.
  - `POST /QuickConnect/Initiate` answers `QuickConnectResult` (or 401 `ProblemDetails`).
  - `GET /QuickConnect/Connect?secret=` (required) answers `QuickConnectResult`, or 404 for an unknown secret.
  - `QuickConnectResult`: `Authenticated` (boolean), `Secret`, `Code`, `DeviceId`, `DeviceName`, `AppName`, `AppVersion`, `DateAdded`.
  - `POST /Users/AuthenticateWithQuickConnect`, body `QuickConnectDto {Secret}` (required), answers `AuthenticationResult` or 400.
  - `POST /QuickConnect/Authorize?code=&userId=` (`userId` optional) needs a signed-in session and answers a boolean, or 401/403.
- `GET /Users/Me` answers `UserDto`, or 400, 401, 403. `POST /Sessions/Logout` answers 204, or 401, 403. Both need a signed-in session.

## Jellyfin 12.2: local discovery and server identity (checked 2026-10-19 in the v12.2 source and docs; no live server queried)
Tag `v12.2` (https://github.com/jellyfin/jellyfin/releases/tag/v12.2). Files below are under `https://github.com/jellyfin/jellyfin/blob/v12.2/`.
- **Discovery is a UDP listener**, `src/Jellyfin.Networking/AutoDiscoveryHost.cs` (it left `Emby.Server.Implementations/Udp` earlier).
  - Transport: plain UDP, no multicast group. `new UdpClient(new IPEndPoint(IPAddress.Any, 7359))`: IPv4 only, all interfaces, port
    hard-coded. Docs list `7359 UDP, Configurable: No, Client Discovery`, and say it does not work outside the local subnet
    (https://jellyfin.org/docs/general/post-install/networking/).
  - Request: the datagram text (UTF-8) must contain `who is JellyfinServer?`; the match is case-insensitive and a substring (`text.Contains`).
  - Reply: unicast to the sender's source address and port, JSON from `MediaBrowser.Model/ApiClient/ServerDiscoveryInfo.cs` with default
    serializer options, so PascalCase and null emitted: `{"Address":"http://192.168.1.10:8096","Id":"<32 hex>","Name":"<server name>","EndpointAddress":null}`.
    `EndpointAddress` is always null from the server. `Id` is `SystemId`, `Name` the server's friendly name.
  - `Address` is `GetSmartApiUrl(sender IP)` (`Emby.Server.Implementations/ApplicationHost.cs`): the `PublishedServerUrl` startup setting if
    set; otherwise `PublishedServerUriBySubnet` entries matched against the sender's address (`NetworkManager.GetBindAddress`), else the
    server interface in the sender's subnet. It is a full URL: scheme, host, port, and the `BaseUrl` path. Containers or a reverse proxy
    can make it an address the client cannot reach, so treat it as a candidate and probe it. If the server cannot work out an address it
    logs a warning and sends no reply.
  - Off switch: `NetworkConfiguration.AutoDiscovery` (`MediaBrowser.Common/Net/NetworkConfiguration.cs`), default `true`;
    `AutoDiscoveryHost` returns without listening when it is false. The admin UI has an `AutoDiscovery` checkbox in jellyfin-web `master`
    (`src/apps/dashboard/routes/networking/index.tsx`); not checked at the 12.2 tag. There is no `AutoDiscoveryTracing` key.
  - DLNA/SSDP (UDP 1900) is a separate first-party plugin since 10.9, not in the base install
    (https://jellyfin.org/docs/general/post-install/networking/dlna/). I found no evidence that the server advertises `_jellyfin._tcp` or any
    other mDNS service; the csproj files I read list no mDNS package. Not exhaustive (see Not verified).
  - Official clients (read through a summarizer, not re-read):
    - Kotlin SDK `LocalServerDiscovery.kt`: the same message to `255.255.255.255:7359` only, an unbound `DatagramSocket`, 500 ms receive
      timeout, 1024-byte buffer, at most about 15 servers, duplicates removed by `Id`, `endpointAddress` set from the packet's source address.
    - Swift SDK `ServerDiscovery.swift`: sends to the broadcast address of each IPv4 interface plus `255.255.255.255`, listens 5 s, reads
      `Id`, `Name`, `Address`, skips loopback, IPv6 and interfaces with no broadcast address.
    - Kotlin SDK `RecommendedServerDiscovery`: probes candidates with `GET /System/Info/Public`, accepts only HTTP 200, 3.5 s timeout, no
      redirects, three at a time, preferring HTTPS and port 8096.
- **Public endpoints** (`Jellyfin.Api/Controllers/SystemController.cs`; no `[Authorize]` on them and no fallback policy is set in
  `Jellyfin.Server/Extensions/ApiServiceCollectionExtensions.cs`):
  - `GET /System/Info/Public` returns `PublicSystemInfo` (`MediaBrowser.Model/System/PublicSystemInfo.cs`): `LocalAddress`, `ServerName`,
    `Version`, `ProductName`, `OperatingSystem` (obsolete, empty), `Id`, `StartupWizardCompleted` (nullable). Filled in
    `Emby.Server.Implementations/SystemManager.cs`.
  - `LocalAddress` is built per request: the `PublishedServerUrl` setting, else (with `EnablePublishedServerUriByRequest`) the request's
    own host, scheme and port, else the subnet logic above. Behind a reverse proxy it may be an internal address. Do not use it as the connect URL.
  - `Id` is the server id: a random GUID in `N` format (32 hex, no dashes) created once and saved as `device.txt` in the data directory
    (`Emby.Server.Implementations/Devices/DeviceId.cs`). It survives restarts and upgrades and changes only when the data directory is lost
    (a container without a persistent volume). It is the same value as the discovery `Id` and, per `SessionManager`, as the `ServerId` that
    `AuthenticateByName` returns (read through a summarizer).
  - `GET`/`POST /System/Ping` is anonymous and returns the product name as a string. `GET /System/Info` needs a policy
    (`FirstTimeSetupOrIgnoreParentalControl`), so it answers unauthenticated during first-run setup at most (inferred from the policy name).
    `GET /health` (`MapHealthChecks`) appears open. `GET /Users/Public` is anonymous and lists the users who are not hidden or disabled
    (all users until the startup wizard is complete): never call it for identity.
  - Base URL: everything is mounted under `BaseUrl` (for example `/jellyfin/System/Info/Public`). A request outside the prefix is redirected
    to it by `BaseUrlRedirectionMiddleware`, so a probe that does not follow redirects sees a redirect, not JSON.
  - `RequireHttps` with a certificate adds `UseHttpsRedirection`, so an `http` probe can be redirected; the status code is not verified.
    `PublicSystemInfo` has no field that says so.
  - `IpBasedAccessValidationMiddleware` answers 503 to clients that remote-access settings reject, on every path including
    `/System/Info/Public`. I found no rate limiter and no setting to turn the endpoint off.

## Server identity without credentials: Seerr 3.5.0 and Stash v0.31.1 (checked 2026-10-19 in the v3.5.0 and v0.31.1 source; no live server queried)
Input for "send credentials only to an address that reports the server id saved at setup". Jellyfin is in its own section above.
- **Seerr 3.5.0** (https://github.com/seerr-team/seerr/tree/v3.5.0, `server/routes/index.ts`, `server/lib/settings/index.ts`):
  - `GET /api/v1/settings/public` needs no session (no `isAuthenticated`; the spec has `security: []`). It returns `plexClientIdentifier`,
    which is `settings.clientId`: `randomUUID()` on first start, saved in `settings.json`. The name is misleading (it is Seerr's own
    instance id, also sent as the Plex OAuth client identifier), but it is the only stable per-instance id a client can read without signing in.
  - Weaknesses: a restored or cloned `settings.json` shares the id; wiping the config volume makes a new one. Whether an admin can change it
    is not verified.
  - The spec types the response as `{initialized, plexClientIdentifier}`, both required, but the code returns about 30 more fields
    (`applicationTitle`, `applicationUrl`, `mediaServerType`, `localLogin`, `mediaServerLogin`, `jellyfinExternalHost`, `vapidPublic`, ...).
    Responses are not validated (`validateResponses` is not set), so read only `initialized` and `plexClientIdentifier` as dependable.
  - `GET /api/v1/status` is public too (`security: []`): `{version, commitTag, restartRequired}` plus `updateAvailable` and `commitsBehind`
    when the version check is on. With it on Seerr calls GitHub, so send `?checkUpdateAvailable=false`. It carries no id.
  - No Jellyfin `serverId` is public: it sits in `settings.jellyfin`, behind `GET /settings/jellyfin` (admin). `GET /auth/me` and the other
    `/settings/*` routes need a session; without one the validator answers 401, as in "Seerr" above.
  - No base path: "Base URLs cannot be configured in Seerr. With this limitation, only subdomain configurations are supported."
    Source: https://docs.seerr.dev/extending-seerr/reverse-proxy/
- **Stash v0.31.1** (https://github.com/stashapp/stash/tree/v0.31.1, `internal/api/server.go`, `internal/api/authentication.go`): no stable
  instance id is readable, with or without credentials.
  - Middleware order in `server.go`: `middleware.Heartbeat("/healthz")`, then CORS, then `authenticateHandler()`. So `GET /healthz` answers
    before authentication (200 with a one-character body, from chi's middleware; it proves a Go service is there, not that it is Stash).
  - Paths that skip authentication (`allowUnauthenticated`): `/login*`, `/logout`, `/css`, `/assets*`. `/graphql`, `/playground`, `/`,
    `/manifest.json` and the media routes need the key once a username and password are set (302 to `/login` for extensionless paths and
    `.html`, 401 for `/graphql` and other extensions, as in "Stash" above). `version` and `systemStatus` are GraphQL queries, so they are behind the key.
  - Per `pkg/session/session.go` (read through a summarizer, not re-read): any non-empty `ApiKey` that differs from the configured key gives
    401 on every path except `/healthz`, even on servers with no credentials. A client that always sends the header must not do so while probing.
  - Per the same source, a server with no username and password is open to local-network clients only (private, loopback, link-local and
    100.64.0.0/10 count as local; others get 403), so a client on the same network can read `version` and `systemStatus` unauthenticated there.
  - The schema has `version {version hash build_time}` and `systemStatus {databasePath configPath appSchema databaseSchema ...}`. None is
    unique per install (`hash` is per build); the paths are semi-stable but leak the file layout. The generated secrets (`jwt_secret_key`,
    `session_store_key`, `api_key`) are not ids and are never public.
  - Base path: no setting; the `X-Forwarded-Prefix` request header is honoured for redirects and `<base href>`
    (`getProxyPrefix`), and `external_host` overrides scheme and host in generated URLs.
  - So for Stash the id can only be checked after the key is sent, or against something the user confirms at setup (for example the
    database path from `systemStatus`, or the server's configured name). Not verified: the `systemStatus` resolver, the open `/graphql`
    response, `/css` and `/assets` bodies.
- Implication (inference): Jellyfin (`Id`) and Seerr (`plexClientIdentifier`) can be verified before any credential leaves the app. Stash
  cannot, so a Stash address needs a different rule: for example send the key only to addresses the user entered or confirmed, and treat a
  new discovered address as untrusted until the user accepts it.

## Seerr 3.5.0: sign-in with the app's Jellyfin session (checked 2026-10-20 in the Seerr v3.5.0 and Jellyfin v12.2 source and specs; nothing sent to a live server)
Input for the M3 item "Seerr sign-in without storing a password". Seerr files are under
`https://github.com/seerr-team/seerr/blob/v3.5.0/` (spec `seerr-api.yml`), Jellyfin files under
`https://github.com/jellyfin/jellyfin/blob/v12.2/` (spec https://api.jellyfin.org/openapi/stable/jellyfin-openapi-12.2.json). The route
code, the Jellyfin client headers, the session options and `QuickConnectManager.cs` were re-read by hand; the rest came from a
research agent reading the same files.
- **Seerr's three routes** (`server/routes/auth.ts`), all under `/api/v1/auth`, with no `isAuthenticated` and `security: []` in the spec:
  - `POST /jellyfin/quickconnect/initiate`, no body: 200 `{code, secret}` (Jellyfin's `Code` and `Secret`, passed through).
    403 `{message: "Quick Connect is only supported by Jellyfin."}` when `mediaServerType` is not Jellyfin. Any Jellyfin error,
    Quick Connect being off included, becomes 500 `{message: "Failed to initiate quick connect."}`. The spec lists 200 and 500 only.
  - `GET /jellyfin/quickconnect/check?secret=`: 200 `{authenticated: boolean}`. 400 `Invalid secret format` (zod: 8 to 128 hex
    characters), 403 as above; any Jellyfin error passes its status through (404 for an unknown or expired secret, 401 when Quick Connect
    is off) with `{message: "Failed to check Quick Connect status"}`, a Jellyfin that does not answer becomes 502. The spec lists 200 and 404.
  - `POST /jellyfin/quickconnect/authenticate`, body `{secret}`: 200 with the user (`User.filter()`: no `email`, tokens or device id, but
    `jellyfinUserId`, `permissions`, `userType`). In order: 400 `Secret required`; 403 `Quick Connect is not available during initial setup.`
    while `mediaServerType` is not configured or Seerr has no user yet; 403 Jellyfin only; 403 `Access denied.` (unimported user, below);
    otherwise Jellyfin's status with `{message: "INVALID_CREDENTIALS"}` (404 for a secret that is unknown, expired or not yet
    approved; 500 if there is none). The spec lists 200, 403 and 500 and no hex pattern.
  - Error bodies go through the error handler in `server/index.ts`: `{message, errors}` with the given status.
- **Seerr only proxies.** It keeps no state for the code or secret (the client holds the secret) and calls the Jellyfin server in its
  settings: `getHostname()` (`server/utils/getHostname.ts`) builds `http(s)://<ip>:<port><urlBase>` from `settings.jellyfin`, never from
  `externalHostname`. It sends `POST /QuickConnect/Initiate`, `GET /QuickConnect/Connect?secret=`, `POST /Users/AuthenticateWithQuickConnect
  {Secret}` with `MediaBrowser Client="Seerr", Device="Seerr", DeviceId="Qk9UX3NlZXJy", Version="<Seerr version>"` (`server/api/jellyfin.ts`):
  the Quick Connect routes pass no device id, so it is the fallback, base64 of `BOT_seerr`, the same for every user.
- **What Seerr does on success:** finds the Seerr user by `jellyfinUserId == User.Id` from Jellyfin's answer. There is no server id check
  on this path (the password route only writes `settings.jellyfin.serverId`). If none is found and `settings.main.newPlexLogin` (shown
  as the setting for new media server sign-ins; default `true`, `server/lib/settings/index.ts`) is off: 403 `Access denied.`; if on,
  it creates a Jellyfin user (`userType` 3) with `settings.main.defaultPermissions` (default `REQUEST`, 32). It stores Jellyfin's
  `AccessToken` as `jellyfinAuthToken` and `base64("BOT_seerr_" + name)` as `jellyfinDeviceId` (both never returned to clients), then sets
  `req.session.userId`. `settings.main.mediaServerLogin` (the admin switch for Jellyfin sign-in) is checked only by the password route,
  not by Quick Connect.
- **Jellyfin's side of the flow** (`Emby.Server.Implementations/QuickConnect/QuickConnectManager.cs`, `Jellyfin.Api/Controllers/QuickConnectController.cs`):
  - Quick Connect is on by default (`ServerConfiguration.QuickConnectAvailable = true`). `GET /QuickConnect/Enabled` and
    `GET /QuickConnect/Connect?secret=` are anonymous. Codes are 6 digits; secrets are 64 hex characters (32 random bytes).
  - State is in the server's memory and expires lazily (on the next Quick Connect call): a request lives 10 minutes from `Initiate`; after
    approval its entry lives 1 more minute plus 10 (so `Connect` still shows `Authenticated: true`) and the stored sign-in result 10
    minutes. A secret is not single-use: `AuthenticateWithQuickConnect` returns the same result and token until it expires. A code can be
    approved once. Pending requests are not capped.
  - Approval creates the token for the device that initiated, so for Seerr's `Qk9UX3NlZXJy` "Seerr" device, not ours. Creating it first
    logs out any device with the same device id and user (`SessionManager`), so each Quick Connect sign-in through Seerr replaces that
    user's previous Seerr Quick Connect token; our app's own Jellyfin token is untouched. Device tokens do not expire.
  - Seerr's sign-out (`POST /auth/logout`) deletes the Jellyfin device `jellyfinDeviceId` with Seerr's admin key, which is
    `base64("BOT_seerr_<name>")`, not `Qk9UX3NlZXJy`, so (inference) the Quick Connect device stays on Jellyfin after a Seerr sign-out.
- **Approving the code from our side:** Jellyfin `POST /QuickConnect/Authorize?code=&userId=` with our user's token.
  - `[Authorize]` with the default policy: any signed-in user, no admin needed; API keys also pass (we never use one). The usual caller
    checks apply: a disabled account, remote access without `EnableRemoteAccess`, a parental schedule.
  - `userId` (`RequestHelpers.GetUserId`): left out means the caller; another user's id needs an administrator, else 403. Always leave it out.
  - 200 with the JSON boolean `true`. Errors, through `ExceptionMiddleware` (outside Development the body is the plain text
    `Error processing request.`, so go by status): Quick Connect off 401 (the action catches every `AuthenticationException` as
    `Quick connect is disabled`); unknown or expired code 404; code already approved 500 (`InvalidOperationException`, unmapped); the target
    user not allowed on Seerr's device or at their session limit 403. The spec lists only 200, 401, 403.
  - The approved user's own policy is not checked again (no password path): `IsDisabled` and remote access are checked on the caller, which
    is the same user when `userId` is left out.
- **Same Jellyfin server on both sides:**
  - Seerr shows no Jellyfin server id without a session. `GET /api/v1/settings/public` has `mediaServerType` (1 Plex, 2 Jellyfin, 3 Emby,
    4 not configured; `server/constants/server.ts`), `mediaServerLogin`, `newPlexLogin`, `localLogin`, `jellyfinExternalHost` (an optional
    link address the admin typed, default empty), `jellyfinForgotPasswordUrl` and others. `jellyfinServerName` is declared but never set.
    `settings.jellyfin.serverId` is admin only (`GET /settings/jellyfin`). `jellyfinExternalHost` is a hint, not proof.
  - Proof before approving (inference from the code above): after Seerr's `initiate`, ask **our** Jellyfin `GET /QuickConnect/Connect?secret=`
    with Seerr's secret. Quick Connect state lives in one Jellyfin process's memory and the secret is 32 random bytes, so 200 with the same
    `Code` (and `AppName` "Seerr", `DeviceId` `Qk9UX3NlZXJy`) means the pending request is on our server; 404 means Seerr uses another
    Jellyfin, and the code must not be approved (it could match an unrelated pending request on ours).
  - Proof after sign-in: `GET /auth/me` returns `jellyfinUserId`, which must equal our Jellyfin user's `Id` (Jellyfin ids are 32 hex
    characters; compare without dashes and case).
- **The Seerr session** (`server/index.ts`, mounted on `/api`): express-session with cookie `connect.sid`, `maxAge` 30 days, `httpOnly`,
  `sameSite` Lax (Strict with CSRF protection on), `secure: 'auto'` (Secure only when Seerr sees HTTPS; `trust proxy` only with the
  `trustProxy` setting), `resave: false`, no `rolling`, TypeORM store with a 30-day ttl.
  - Per express-session and connect-typeorm (read at v1.18.1 and `master`, not the exact versions Seerr pins): each request with an
    unchanged session touches the store, so the server-side expiry slides, but `Set-Cookie` is sent only when the session changes, so the
    cookie's own `Expires` stays 30 days from sign-in. A client cookie store drops the cookie then. Not verified live.
  - `GET /api/v1/auth/me`: the user (`id`, `displayName`, `username`, `jellyfinUsername`, `jellyfinUserId`, `userType`, `permissions`, `avatar`,
    quotas, `requestCount`, `warnings`, ...) plus `settings {locale, discoverRegion, streamingRegion, originalLanguage, notificationTypes,
    watchlistSyncMovies, watchlistSyncTv}`. Without a cookie the validator answers 401; with a cookie for a missing or expired session,
    `isAuthenticated` answers 403 `{status: 403, error: "You do not have permission to access this endpoint"}` (`server/middleware/auth.ts`).
  - `permissions` (`server/lib/permissions.ts`): NONE 0, ADMIN 2, MANAGE_SETTINGS 4, MANAGE_USERS 8, MANAGE_REQUESTS 16, REQUEST 32,
    VOTE 64, AUTO_APPROVE 128, AUTO_APPROVE_MOVIE 256, AUTO_APPROVE_TV 512, REQUEST_4K 1024, REQUEST_4K_MOVIE 2048, REQUEST_4K_TV 4096,
    REQUEST_ADVANCED 8192, REQUEST_VIEW 16384, AUTO_APPROVE_4K 32768, AUTO_APPROVE_4K_MOVIE 65536, AUTO_APPROVE_4K_TV 131072,
    REQUEST_MOVIE 262144, REQUEST_TV 524288, MANAGE_ISSUES 1048576, VIEW_ISSUES 2097152, CREATE_ISSUES 4194304, AUTO_REQUEST 8388608,
    AUTO_REQUEST_MOVIE 16777216, AUTO_REQUEST_TV 33554432, RECENT_VIEW 67108864, WATCHLIST_VIEW 134217728, MANAGE_BLOCKLIST 268435456,
    VIEW_BLOCKLIST 1073741824. `hasPermission` treats ADMIN as every permission.
  - Renewal: `authenticate` only sets `req.session.userId` (no `regenerate`), so a client that still sends its `connect.sid` keeps the same
    id and gets a new `Set-Cookie` with a fresh 30 days; without one it gets a new session. Another Quick Connect round therefore renews
    the session silently. No rate limiter is applied to the auth routes (`express-rate-limit` is a dependency; where it is used was not found).
  - CSRF (`settings.network.csrfProtection`, default off): `@dr.pogodin/csurf` with an `_csrf` cookie (HttpOnly, SameSite Strict, Secure
    outside development) and a readable `XSRF-TOKEN` cookie set on every response; POSTs, Quick Connect ones included, must echo it in
    `X-XSRF-TOKEN` (or `CSRF-Token`, `XSRF-Token`, `X-CSRF-Token`, per the package README). Because those cookies are Secure, this
    presumably works only over HTTPS (inference).
  - `POST /api/v1/auth/logout`: 200 `{status: "ok"}`, with or without a session.
- **Failure cases, as the app sees them:**

  | Situation | What answers |
  |---|---|
  | Quick Connect off on Jellyfin | Our `GET /QuickConnect/Enabled` is `false`; Seerr `initiate` 500 |
  | Seerr not on Jellyfin, or not set up | `settings/public` `mediaServerType` not 2, or `initialized` false; `initiate` 403 |
  | Seerr on another Jellyfin server | Our `GET /QuickConnect/Connect?secret=` 404 (do not approve) |
  | Our user not imported and new sign-ins off | `authenticate` 403 `Access denied.` (`newPlexLogin` false in `settings/public` predicts it) |
  | Our user not allowed on Seerr's device, or at their session limit | Our `Authorize` 403 |
  | Code expired (10 minutes) | Our `Authorize` 404; Seerr `check` and `authenticate` 404 |
  | Session gone | `GET /auth/me` 403 (cookie sent) or 401 (none) |
- **Clients:**
  - Kotlin/Ktor: a cookie store per Seerr server (Ktor's `HttpCookies` plugin with our own `CookiesStorage`). `connect.sid` is a
    credential: keep it in the `SecretStore` (ADR 0017), never in settings or logs, and redact `Cookie` and `Set-Cookie` in the request
    log; Seerr requests need no other auth. With CSRF on, read `XSRF-TOKEN` from the store and send it
    in `X-XSRF-TOKEN` on every POST. Whether Ktor's storage keeps a Secure cookie for an `http` URL was not checked.
  - Browser: the browser owns the HttpOnly cookie, so the app never sees it. Seerr has no CORS and its validator answers OPTIONS
    preflights with 405 (express-openapi-validator 5.6.2, no `cors` package in `server/index.ts`), so Seerr's API must be served on the
    app's origin (reverse proxy) or Seerr needs CORS (see "Seerr" and the roadmap's M3 decision). Same origin, the cookie travels with
    `fetch` by default and SameSite does not matter. The calls to Jellyfin (`Connect`, `Authorize`) use our token header, and Jellyfin's
    CORS is open.
- **Recommended flow** (inference; to confirm end to end against live servers):
  1. Before offering it: `GET /api/v1/settings/public` shows `mediaServerType` 2 and `initialized`; the app holds a Jellyfin session; our
     Jellyfin's `GET /QuickConnect/Enabled` is `true`. Otherwise offer the fallback (password or local account, roadmap M3).
  2. Seerr `POST /api/v1/auth/jellyfin/quickconnect/initiate` → `{code, secret}`.
  3. Our Jellyfin `GET /QuickConnect/Connect?secret=<secret>` must answer 200 with the same `Code`; on 404 stop ("Seerr uses a different
     Jellyfin server").
  4. Our Jellyfin `POST /QuickConnect/Authorize?code=<code>` (no `userId`) with our token → `true`.
  5. Seerr `POST /api/v1/auth/jellyfin/quickconnect/authenticate {secret}` → 200 and `Set-Cookie: connect.sid` (no polling of `check` is
     needed, since we approved it ourselves). 403 `Access denied.` means an administrator has to import the user.
  6. Seerr `GET /api/v1/auth/me`: `jellyfinUserId` must equal our Jellyfin user id; otherwise sign out of Seerr and keep nothing. Keep the
     cookie in the `SecretStore` and the user, `permissions` (a `Long`) and the time of sign-in in settings.
  7. Renew by running steps 2 to 6 again well before 30 days from the last `Set-Cookie` (for example after 25 days, at start or when
     online), and at once when `GET /auth/me` answers 401 or 403, as long as the Jellyfin session is valid. Signing out of Jellyfin does not
     sign out of Seerr; sign out of both if the user asks.

## Local network, discovery and plain HTTP: Apple (checked 2026-10-19 in Apple's documentation, read as JSON from developer.apple.com; nothing run on a device)
- Local network privacy: TN3179 (revised 2026-10-06), https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy
  - Platforms: iOS and iPadOS 14, macOS 15, visionOS 1. **tvOS: "no"** in the table. The reference page of `NSLocalNetworkUsageDescription`
    still lists tvOS 14, so set the key anyway; it is harmless. Not verified: what tvOS does with local traffic.
  - A local network is an IP network on a broadcast-capable interface (Wi-Fi, Ethernet; not cellular or VPN). `255.255.255.255` and all
    multicast addresses count as local. The check sits "deep in the networking stack", so it applies to URLSession, Network framework and BSD sockets alike.
  - Needs access (prompt on first use): outgoing TCP to a local address, sending UDP (unicast, multicast, broadcast), connecting a UDP socket,
    receiving UDP multicast and broadcast, resolving a `.local` name, all Bonjour operations. Does not: accepting TCP, **receiving a UDP unicast**.
    Traffic from WKWebView, SFSafariViewController and Safari is exempt.
  - So plain HTTP to `http://192.168.1.10:8096` triggers the prompt, and the app needs `NSLocalNetworkUsageDescription` in `Info.plist`
    (the app, not an extension). `NSBonjourServices` lists the service types to browse or register; only needed if we use Bonjour.
  - Denied access has no query API. An `NWConnection` goes to `.waiting` with `currentPath?.unsatisfiedReason == .localNetworkDenied`; an
    `NWBrowser` reports `kDNSServiceErr_PolicyDenied` (-65570). The system retries a connection by itself when the user grants access later.
    The first operation may be refused at once, before the user answers the alert, so use `NWConnection`/Network framework or URLSession
    with `waitsForConnectivity`, or retry.
  - In the background with the state undetermined, the operation is refused and no alert shows. The simulator does not support local
    network privacy: test on a device. macOS keeps the state per user account and has `defaults` keys for allowing networks without a prompt (admin use).
  - TN3179 on custom UDP broadcast discovery: use Bonjour if you can; otherwise "run your service discovery code on all broadcast-capable
    interfaces" and never hard-code `en0`.
- Multicast entitlement `com.apple.developer.networking.multicast`: "Your app must have this entitlement to send or receive IP multicast or
  broadcast on iOS", and it "requires permission from Apple" through the Multicast Networking Entitlement Request page.
  - Table in TN3179: sending a UDP broadcast, and receiving UDP broadcast or multicast, need it on iOS; **UDP unicast does not**.
    "The multicast entitlement isn't required on macOS." The entitlement's page lists iOS, iPadOS and visionOS. **tvOS is not mentioned;
    not verified.** With Xcode's automatic signing the Capabilities editor adds it once Apple has granted it.
  - Source: https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.networking.multicast
  - Consequence: the iOS build can do unicast probes of saved and typed addresses without the entitlement. Broadcast discovery of the
    Jellyfin kind needs Apple's approval first. Not verified: whether Apple approves it for a media client, and how long it takes.
- macOS App Sandbox (if the Mac build is sandboxed): for UDP, `com.apple.security.network.client` allows only sending and
  `com.apple.security.network.server` only receiving, "Apps using UDP usually require both". For TCP the entitlements only limit who initiates.
  Sources: https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.network.client and `.server`.
  Whether the Mac app will be sandboxed is not decided.
- App Transport Security applies to "all HTTP connections made with the URL Loading System—typically using the URLSession class"
  (https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity). `ktor-client-darwin`
  is built on `NSURLSession` (its sources reference it), so ATS governs our Apple HTTP calls. Network framework and BSD sockets are not
  URLSession; the page does not name them. The ATS keys' reference pages list iOS, iPadOS, macOS and visionOS, **not tvOS; not verified for tvOS**.
  - `NSAllowsLocalNetworking`: "controls whether ATS allows your app to connect to unqualified domains, `.local` domains, and IP addresses
    using IPv4 or IPv6." Since iOS 17 and macOS 14 ATS does not allow IP addresses by default; this key re-allows them.
    (https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking)
    The page does not say whether "IP addresses" means only local ones. Not verified.
  - So `http://192.168.1.10:8096`, `http://nas:8096` (unqualified) and `http://media.local:8096` are allowed with the key. By the page's list,
    **`http://media.lan:8096` is not**: a dotted name that does not end in `.local` is none of the three kinds. Inferred from the text; not tried on a device.
  - `NSExceptionDomains` takes names, IPv4/IPv6 addresses and CIDR ranges (iOS 17, macOS 14), without ports or wildcards, but it is
    fixed at build time, so it cannot cover addresses a user types.
  - `NSAllowsArbitraryLoads` is "ignored—and the default value of NO used instead" when `NSAllowsLocalNetworking` (or
    `NSAllowsArbitraryLoadsInWebContent` or `...ForMedia`) is present, and it needs a justification in App Store review. So the Store edition
    can have the local-network exemption, or arbitrary loads, not both; arbitrary loads suits the Private edition (ADR 0014) only.
  - Not verified: whether AVPlayer's loads of a local `http` URL follow `NSAllowsLocalNetworking`.

## Device name and the local network keys: Apple (checked 2026-10-21 in Apple's documentation, read as JSON from developer.apple.com; nothing run for this section)
- The launch watchdog kill is in "Roaming on iOS" ("Not roaming, found on the way"). The `hostName` page says only "The name of the
  host computer on which the process is executing" and lists every Apple OS; it does not say it may resolve over the network.
  https://developer.apple.com/documentation/foundation/processinfo/hostname So the device name must come from an API that reads local
  state only, and nothing at start may wait on the network on the main thread.
- iOS and tvOS: `UIDevice.current.name`. The entitlement page `com.apple.developer.device-information.user-assigned-device-name` (iOS,
  tvOS 16): "In iOS 15 and earlier, the [name] property returns this name. In iOS 16 and later, the [name] property returns a generic device
  name by default instead"; the user-assigned name needs the entitlement, granted only for a user-visible multi-device feature, never for
  tracking. We use the generic name ("iPhone", "iPad"), without the entitlement. The generic value's exact text is not listed; not verified on a device.
  https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.device-information.user-assigned-device-name
  The `UIDevice.name` page lists tvOS without an introduction version (https://developer.apple.com/documentation/uikit/uidevice/name);
  what it returns on tvOS is not verified.
- macOS: `SCDynamicStoreCopyComputerName(_:_:)` (SystemConfiguration, macOS 10.1): "Returns the current computer name", or NULL "if the name
  has not been set or if an error occurred"; pass NULL for a temporary session; "You must release the return value" (Kotlin:
  `CFBridgingRelease`). It reads the system configuration store, not DNS. https://developer.apple.com/documentation/systemconfiguration/scdynamicstorecopycomputername(_:_:)
  `NSHost.current().localizedName` (macOS 10.6) is "the name displayed in the Finder sidebar, as well as in the Sharing preference panel";
  its page does not say whether it resolves, so it is not used. https://developer.apple.com/documentation/foundation/host/localizedname
- `NSLocalNetworkUsageDescription`: "A message that tells people why the app is requesting access to the local network. Any app that uses
  the local network, directly or indirectly, should include this description", including "direct unicast or multicast connections to local
  hosts". Lists iOS 14, macOS 11, tvOS 14, visionOS 1 (TN3179 above says tvOS does not enforce local network privacy).
  https://developer.apple.com/documentation/bundleresources/information-property-list/nslocalnetworkusagedescription
- `NSAllowsLocalNetworking` (under `NSAppTransportSecurity`): lists iOS 10, macOS 10.12, visionOS 1, **not tvOS**. "In iOS 17, iPadOS 17, and
  macOS 14, ATS no longer allows connections to IP addresses by default"; the key re-enables unqualified names, `.local` names and IP
  addresses. https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking
- So `appleApp/Current/Info.plist`, shared by iOS, macOS and tvOS (ADR 0012), carries both keys. On tvOS the ATS key is set anyway; whether
  tvOS allows plain `http` to a LAN address with it, or without it, is not verified.

## Privacy manifest: Apple (checked 2026-10-21 in Apple's documentation, read as JSON from developer.apple.com, and in the Kotlin/Native, kotlinx.coroutines and Ktor sources; nothing built for this section)
- "Privacy manifest files": "Apps and third-party SDKs [...] can contain a privacy manifest file, named `PrivacyInfo.xcprivacy`", which
  records "The types of data collected by your app or third-party SDK. You need to provide this information for your app or third-party
  SDK on all platforms" and "The required reasons APIs your app or third-party SDK uses. You need to provide this information for your app
  or third-party SDK on iOS, iPadOS, tvOS, visionOS, and watchOS." "You need to add the privacy manifest file to your target's resources
  for Xcode to use it when you generate a privacy report." https://developer.apple.com/documentation/bundleresources/privacy-manifest-files
- "Describing use of required reason API": "Starting May 1, 2024, apps that don't describe their use of required reason API in their
  privacy manifest file aren't accepted by App Store Connect." "If you use the API in your app's code, then you need to report the API in
  your app's privacy manifest file." "For each executable or dynamic library in an app that uses a required reason API, the bundle that
  includes the executable or dynamic library needs to include a privacy manifest file that reports the API."
  https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api
  `Shared` is a static framework linked into the app's executable (`apple/build.gradle.kts`), so the app's manifest covers its code.
- The categories and reasons are on the `NSPrivacyAccessedAPIType` page
  (https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitype):
  - `NSPrivacyAccessedAPICategoryUserDefaults`, `CA92.1`: "Declare this reason to access user defaults to read and write information that
    is only accessible to the app itself. This reason does not permit reading information that was written by other apps or the system, or
    writing information that can be accessed by other apps." Used: `UserDefaultsKeyValueStore` (core:database `appleMain`) reads and writes
    `NSUserDefaults.standardUserDefaults` under its own `current.` keys. Declared.
  - `NSPrivacyAccessedAPICategorySystemBootTime`: `systemUptime` and `mach_absolute_time()`. Not called by our Kotlin or Swift code (grep).
    `TimeSource.Monotonic` (roaming, request durations; kotlinx.coroutines' native `nanoTime()` and `delay` use it too) is
    `getTimeNanos()` in the Kotlin/Native runtime (`kotlin-native/runtime/src/main/cpp/Porting.cpp`, tag `v2.4.20`), which reads
    `std::chrono::steady_clock` (or `high_resolution_clock` when steady), not either listed API. What Apple's libc++ calls under
    `steady_clock` is not verified, nor whether App Store Connect flags it. Not declared; if App Store Connect asks, the reason is `35F9.1`
    (elapsed time between events within the app).
  - `NSPrivacyAccessedAPICategoryFileTimestamp` (`creationDate`, `modificationDate`, `stat`, `fstat`, `getattrlist`, ...),
    `NSPrivacyAccessedAPICategoryDiskSpace` (`statfs`, `volumeAvailableCapacity...Key`, `systemFreeSize`, ...),
    `NSPrivacyAccessedAPICategoryActiveKeyboards` (`activeInputModes`): none in our Kotlin `appleMain`/`iosMain`/`macosMain`/`tvosMain`
    code or in `appleApp/` (grep), none in the Kotlin/Native runtime's Apple code (`fstat` appears only in its ELF reader), none in
    kotlinx.coroutines 1.11.0 `native`/`nativeDarwin` or in Ktor 3.6.0's client, utils, io and http sources (the "modification-date" hits
    there are HTTP header names). Not declared. Not verified: kotlinx-io's file system code (not called by us) and whatever the system
    frameworks call on our behalf.
  - macOS: `SCDynamicStoreCopyComputerName` (device name) is not on the list.
- Collected data. "Describing data use in privacy manifests": "Record the categories of data that your app or third-party SDK collects about
  the person using the app". https://developer.apple.com/documentation/bundleresources/describing-data-use-in-privacy-manifests
  The definition is on "App privacy details on the App Store": "\"Collect\" refers to transmitting data off the device in a way that
  allows you and/or your third-party partners to access it for a period longer than what is necessary to service the transmitted request
  in real time." and "Data that is processed only on device is not \"collected\"". https://developer.apple.com/app-store/app-privacy-details/
  Current sends sign-in details, the device name and id and playback requests only to the servers the user adds; the developer and no
  partner can read them. So `NSPrivacyCollectedDataTypes` is empty. This is a reading of "you and/or your third-party partners", not a
  case Apple names: the owner confirms it before the first App Store submission. "Copy diagnostics" only copies to the clipboard.
- `NSPrivacyTracking` is false and `NSPrivacyTrackingDomains` empty: no tracking, no analytics, no third-party SDK that contacts a domain.
- The file is `appleApp/Current/PrivacyInfo.xcprivacy`. The target's folder is a synchronized group, so the file is a target member with no
  `project.pbxproj` change. `scripts/build-apple-app.sh` fails when a built bundle lacks it. The privacy report (Product > Archive >
  Generate Privacy Report) has not been generated.

## Local network, discovery and cleartext HTTP: Android (checked 2026-10-19 in the Android docs and AOSP `main`; nothing run on a device)
- Android 17 local network permission, https://developer.android.com/privacy-and-security/local-network-permission and
  https://developer.android.com/about/versions/17/behavior-changes-17
  - `ACCESS_LOCAL_NETWORK`, a runtime permission in the `NEARBY_DEVICES` group, **mandatory for apps that target API 37**. An app with
    `INTERNET` that targets 36 or lower gets it implicitly; do not request it there. Android 16 had an opt-in (`RESTRICT_LOCAL_NETWORK`
    compat flag, `NEARBY_WIFI_DEVICES`).
  - Covered, for "all networking APIs" (raw sockets, Cronet, OkHttp, `NsdManager`): outgoing TCP, accepting TCP, sending **and receiving** UDP
    unicast, multicast and broadcast, and resolving `.local` names. Exempt: DNS to a local DNS server (port 53), and apps that use the system
    picker (`DiscoveryRequest.FLAG_SHOW_PICKER` in `NsdManager`, Output Switcher). WebView inherits the host app's state.
  - Denied: TCP "will typically result in a timeout error", UDP and other denials in `EPERM`. The page does not say which address
    ranges count as the local network. Not verified.
  - So against a target-37 build, plain `http://192.168.1.10:8096` and the UDP discovery both need the permission, and the app should
    handle a denial by showing that local servers cannot be reached. Confirmed on an Android 17 device on 2026-10-20 (`EPERM` for UDP,
    a connect timeout for TCP), see "Local server discovery, run on devices".
- `INTERNET` and `ACCESS_NETWORK_STATE` (for network callbacks, below). A `WifiManager.MulticastLock` ("Allows an application to receive
  Wifi Multicast packets ... can cause a noticeable battery drain") needs `CHANGE_WIFI_MULTICAST_STATE` (normal permission, `AndroidManifest.xml`
  of the framework). It is described for multicast, not broadcast or unicast replies; a broadcast discovery did not need it on Android 16 or 17
  (2026-10-20, see "Local server discovery, run on devices").
- Cleartext, https://developer.android.com/privacy-and-security/security-config :
  - Default for apps targeting API 28+ is `cleartextTrafficPermitted="false"` in `base-config`.
  - `domain-config` matches DNS names (`includeSubdomains`); the page documents no wildcard, no CIDR and no `.local` special case. It only
    mentions IP literals for loopback (Android 17 adds an implicit cleartext allowance for localhost). Whether a `<domain>` can be a LAN
    IP literal is not documented. Not verified.
  - The XML is fixed at build time, so user-chosen addresses need `base-config cleartextTrafficPermitted="true"` (every host over HTTP)
    or the app enforcing its own rule above the platform's. A permissive base config cannot be narrowed later.
  - `android:usesCleartextTraffic` is ignored on API 24+ when a network security config exists, and "is getting deprecated and will be ignored for
    apps targeting API levels 38 and above" (https://developer.android.com/guide/topics/manifest/application-element).
  - `NetworkSecurityPolicy` is enforced "on a best effort basis" by platform stacks (HTTP and FTP, `DownloadManager`, `MediaPlayer`; WebView
    for target 26+). `java.net.Socket` does not; "Third-party libraries are strongly encouraged to honor" it. AOSP `NetworkSecurityPolicy.java`.
    The Android engine (`ktor-client-android`, `HttpURLConnection`) is a platform stack, so the setting applies. Not verified for Media3's
    data sources and Coil.
- Network changes: `ConnectivityManager.registerDefaultNetworkCallback` (`@RequiresPermission(ACCESS_NETWORK_STATE)`; at most 100 callbacks per app;
  unregister them). `NetworkCallback` has `onAvailable`, `onLosing`, `onLost`, `onCapabilitiesChanged`, `onLinkPropertiesChanged`,
  `onBlockedStatusChanged`. AOSP `ConnectivityManager.java`. Nothing run.
  - Reading network state (https://developer.android.com/training/basics/network-ops/reading-network-state, checked 2026-10-20):
    `getActiveNetwork()` gives "the current default network". "When a new network becomes the default, the app receives a call to
    `onAvailable(Network)` for the new network." In its sample sequence, after `registerDefaultNetworkCallback()` "the default network
    callback starts receiving calls to `onAvailable()` ... for the mobile network, because the mobile network is the default network", so
    registering can report the current network at once; the page does not say so as a rule. Not verified on a device.


## Local server discovery, run on devices (checked 2026-10-20 against a live Jellyfin 12.2; M1 spike)
Throwaway probes sent `who is JellyfinServer?` to UDP 7359 and then `GET /System/Info/Public`; none of the code is in the repo. The server
ran in a Docker container in bridge mode on a NAS OS, on the same /24 as every client. Addresses below are the example values
(`192.168.1.10` for the server, `192.168.1.255` for the subnet's broadcast address).
- **Seerr 3.5.0 and Stash v0.31.1 cannot be discovered by the app** (read in the source at the tags; nothing run):
  - Seerr: `package.json` has no mDNS, Bonjour, SSDP or UPnP package, and `server/index.ts` starts only Express over TCP (`PORT`, default
    5055). Seerr finds Plex servers itself through `plex.tv/api/resources` (`server/api/plextv.ts`), not on the LAN. Not checked:
    `dgram` in files other than `index.ts`; a repo-wide search was not possible.
  - Stash: only its DLNA server (`internal/dlna`, `github.com/anacrolix/dms` v1.2.2) speaks SSDP, and it is **off by default**
    (`dlna.default_enabled` has no default; `RefreshDLNA()` in `internal/manager/manager.go`). When on, it advertises a generic
    `urn:schemas-upnp-org:device:MediaServer:1` on `239.255.255.250:1900` (IPv4 only) with `LOCATION` on the DLNA port (default 1338),
    friendlyName and manufacturer from `dlna.server_name` (default `stash`), and **no `presentationURL`**: the UI and GraphQL port
    (default 9999) cannot be learned from it. Guessing Stash from a DLNA advertisement is not worth building.
  - Jellyfin 12.2 again: `Directory.Packages.props` lists no mDNS or SSDP package; UDP 7359 on `IPAddress.Any` (IPv4) is the only
    advertisement, on by default, and jellyfin-web 12.2 has the `AutoDiscovery` checkbox (`src/apps/dashboard/routes/networking/index.tsx`).
    So Seerr and Stash are added by URL, or found next to a Jellyfin server the user already has (out of scope here).
- **Server side** (Jellyfin 12.2.0 in Docker, bridge network):
  - The NAS's app template published `7359:7359`, which Docker publishes as TCP only. Every probe (broadcast or unicast) went unanswered,
    with no error anywhere, while HTTP on 8096 worked. After the mapping became `7359:7359/udp` all probes were answered. Broadcasts
    to the subnet's broadcast address reached the bridge container too (not expected; how Docker forwarded them was not looked into).
  - The reply's `Address` was `http://127.0.0.1:8096` (and `/System/Info/Public` reported `LocalAddress` `http://[::1]:8096`): the server
    in the container could not work out an address the client can reach. `Id` and `Name` were right. **Use the packet's source address,
    with the port and path from `Address`, as the candidate**, never `Address` as it stands, and confirm it with the probe (ADR 0016).
  - On a multi-homed client the same server answered once per network that reaches it (here also through an overlay VPN, from that
    network's gateway address), with the same `Id`. Group replies by `Id` and offer the addresses as one server.
  - Replies took 2 to 60 ms. A 2 s listening window is plenty on a LAN; the Kotlin SDK's 500 ms would also have caught all of them.
- **Windows desktop** (Windows 11 25H2, build 26200; Oracle JDK 21.0.11 and Azul JDK 21; `java.net.DatagramSocket`, no dependency):
  - The PC had a wired LAN adapter (network profile *Public*), two Hyper-V switches (WSL and Default Switch), an overlay VPN and a
    link-local tunnel adapter, each with a broadcast address in `InterfaceAddress.getBroadcast()`.
  - `new DatagramSocket()` sending to `255.255.255.255` (the Kotlin SDK's way) **got no reply, three times out of three**: Windows sent
    the limited broadcast out of one interface only, the one `Find-NetRoute -RemoteIPAddress 255.255.255.255` names (here the Hyper-V
    Default Switch). The same send from a socket bound to the LAN adapter's address was answered.
  - Sending from one wildcard socket to **each interface's directed broadcast address** (`192.168.1.255`, ...) was answered at once, and so
    were per-interface bound sockets and a unicast probe to `192.168.1.10:7359`.
  - Windows Firewall: replies arrived for a `java.exe` that had no firewall rule at all, on the *Public* profile, and **no prompt
    appeared** (`NotifyOnListen` is on for every profile). The app only sends from an ephemeral port and reads the unicast replies; it never
    listens on a fixed port, so it needs no rule. Not tried: a policy that blocks outbound UDP, and a reply arriving after several seconds.
  - Silence looks the same whatever its cause: a closed UDP port gave a timeout, not `PortUnreachableException` (no ICMP reached Java),
    so "no reply" cannot tell a missing server from a blocked port or discovery switched off.
  - Linux was not run on hardware. WSL2 in its default NAT mode (Ubuntu 24.04, Python sockets) sent no broadcast that reached the LAN;
    a unicast probe through the NAT was answered.
- **Android** (Pixel 8 Pro, Android 16, build BP3A, security patch 2025-11-05; the app's `targetSdk` 37, a debug build; `java.net`):
  - With `INTERNET` alone, no `ACCESS_LOCAL_NETWORK` or `NEARBY_WIFI_DEVICES` grant and no `MulticastLock`, `255.255.255.255`, the
    directed broadcast and unicast were all answered (10 to 50 ms), in the foreground with the screen on. A `MulticastLock` changed nothing.
  - Android 16's opt-in, `adb shell am compat enable RESTRICT_LOCAL_NETWORK <package>` (change id 365139289), was accepted, but nothing
    was blocked afterwards, with or without `NEARBY_WIFI_DEVICES`. So this build does not enforce the opt-in.
  - Cleartext `http://192.168.1.10:8096` worked only with a network security config that permits cleartext (the default for target 28+ blocks it).
- **Android 17** (the same Pixel 8 Pro after the update: Android 17, SDK 37, build CP3A, security patch 2026-10-05; the same target-37 debug build):
  - Without `ACCESS_LOCAL_NETWORK` (never asked, or denied): every UDP send, to `255.255.255.255`, to the directed broadcast and to
    `192.168.1.10`, failed at once with `IOException: sendto failed: EPERM (Operation not permitted)`. HTTP to `192.168.1.10:8096` did not
    fail fast: `SocketTimeoutException: failed to connect` after our 3.5 s connect timeout. So a denied app can tell from UDP at once, but an
    HTTP probe only times out; check the permission before probing rather than reading the error.
  - The request shows the `NEARBY_DEVICES` group dialog: "Allow <app> to find, connect to and determine the relative position of nearby
    devices?", Allow / Don't allow. Nothing in it mentions servers or the local network, so the app has to explain first. In the app's
    settings the permission is listed as "Nearby devices".
  - Granted (only `ACCESS_LOCAL_NETWORK`; `NEARBY_WIFI_DEVICES` stayed denied): `255.255.255.255`, the directed broadcast and unicast were
    answered (16 to 65 ms) and HTTP returned 200, with or without a `MulticastLock`.
  - Denials follow the usual runtime-permission rule: after the first "Don't allow" a new request showed the dialog again; after the
    second the permission was `USER_FIXED` and requests returned without a dialog, so the app must send the user to its settings page.
  - Not run: WebView, background or screen-off use, other address ranges (which ranges count as local), and Android TV.
- **iOS** (iPhone 15 Pro Max, iOS 27.0.1; built with Xcode 27.1 on macOS 26.6.2, development-signed with a free personal team; `Info.plist`
  with `NSLocalNetworkUsageDescription` and `NSAllowsLocalNetworking`; **no multicast entitlement**):
  - The prompt came with the first local operation: "Allow "<app>" to find devices on local networks?", the purpose string, a map with the
    current Wi-Fi network and some nearby devices, the line "Information from networks can be used to determine your location and create a
    profile of you.", and Don't Allow / Allow. That first `URLSession` request failed in 21 ms with `NSURLErrorDomain -1009` and
    `_NSURLErrorNWPathKey=unsatisfied (Local network prohibited)` while the alert was still up, as TN3179 says; after Allow it got 200.
  - Allowed, without the entitlement: **unicast UDP to `192.168.1.10:7359` works** with BSD `sendto` and with `NWConnection`
    (replies in 10 to 20 ms), and so does plain HTTP to the IP under `NSAllowsLocalNetworking`.
  - Broadcast without the entitlement: `NWConnection` to `192.168.1.255` gets `EACCES` (13) on send; BSD `sendto` to `255.255.255.255`
    gets `EHOSTUNREACH` (65). But **BSD `sendto` to the subnet's directed broadcast (`192.168.1.255`) went out and was answered**, four
    times out of four, once **with Local Network switched off** in Settings, when unicast BSD got `EHOSTUNREACH`, `NWConnection` sat in
    `.waiting` with `unsatisfiedReason == .localNetworkDenied`, and HTTP failed with `-1009` in 4 ms. That contradicts TN3179's table
    (sending a broadcast needs the entitlement and local network access); it looks like a gap in the check, may be closed in any update,
    and may differ for distribution-signed builds. **Do not build on it.**
  - Not run: macOS (its prompt, and whether it needs anything for broadcast), tvOS, iPadOS, a release or TestFlight build.
- **Browsers** (a page running `fetch` and `new Image()` against `http://192.168.1.10:8096`; Windows 11):
  - No UDP anywhere: `typeof UDPSocket` and `TCPSocket` were `undefined` in Chrome 155, Edge 155, Firefox 158 and an embedded Chromium 152.
  - Chrome 155 and Edge 155, from `https://example.com`: the first `fetch` to the private IP literal showed a permission prompt, "<site> wants to:
    Access other devices on your local network", Allow / Block, and the request waited for the answer. **Allowed**: 200 for `fetch`, with and
    without `targetAddressSpace: "local"`, and the `<img>` loaded (not upgraded to https because the host is an IP); mixed content was
    logged as a warning ("This content should also be served over HTTPS"), not blocked. **Blocked**: every request failed at once
    (`TypeError: Failed to fetch`, about 1 to 5 ms) with the console line "blocked by CORS policy: Permission was denied for this request to
    access the `local` address space"; the `<img>` failed the same way. A page cannot tell a denial from an unreachable server by the error.
  - Firefox 158 on Windows, from `https://example.com`: **no prompt**; `fetch` was "Blocked loading mixed active content"
    (`TypeError: NetworkError when attempting to fetch resource.`, at once), and the `<img>` was upgraded to `https://192.168.1.10:8096`
    and failed, though the host is an IP. So Firefox gives an https page no way to an http server on the LAN.
  - Edge 155 and the embedded Chromium from `http://localhost:8765` (a secure context, loopback address space): `fetch` and `<img>` worked
    with no prompt. Not run from `localhost` in Chrome and Firefox. The embedded Chromium reports the local network permission as denied
    and blocked everything from the https page as mixed content, so it does not stand in for Chrome.
  - Not run: Safari; a LAN host name (`media.lan`, `.local`) instead of an IP literal; a page served from the LAN over plain http.
- **Recommendation** (inference from the runs above; the design belongs in an ADR):
  - Discovery offers Jellyfin servers only. Seerr and Stash are added by URL.
  - Every reply is a candidate: take the source address, the port and path from `Address`, group by `Id`, probe with
    `GET /System/Info/Public`, and show name, address and version. Nothing is saved, and no credential is sent, until the user picks one (ADR 0016).
  - "Found nothing" says the server may have discovery off or UDP 7359 not published (for example a container that publishes it as TCP),
    and offers manual entry. Manual entry is never behind discovery.
  - Desktop (Windows, Linux): broadcast to each interface's directed broadcast address (from `InterfaceAddress.getBroadcast()`) from one
    wildcard socket, plus `255.255.255.255` on each interface-bound socket if cheap, and listen for about 2 s. Run it when the user opens
    "Add server", not at start. `java.net` is enough; no dependency.
  - Android: the same broadcast. With target 37 every local request, discovery or not, needs `ACCESS_LOCAL_NETWORK`. Ask for it with
    a short explanation when the user opens "Add server" or a saved server is local, because the system dialog only speaks of "nearby
    devices". Check the permission before probing (a denied HTTP probe only times out), and after two denials link to the app's settings.
  - iOS and iPadOS: no broadcast discovery without the multicast entitlement, so manual entry first, with unicast probes (UDP 7359 or HTTP)
    of what the user types. Request the entitlement from Apple if discovery is wanted there. Trigger the local network prompt from a user
    action, explain it first (the alert shows a map and a profiling warning), and retry after the answer, because the first request fails while the alert is up.
    macOS and tvOS need their own run.
  - Web: no discovery. Manual entry only. From an https deployment, an `http://<LAN IP>` server works in Chrome and Edge only after the user
    allows the prompt, and never in Firefox; say so on the add-server form and suggest https on the server or a LAN-served http copy of the app.

## Detecting network changes (checked 2026-10-19; documentation only)
- Apple: `NWPathMonitor` (iOS 12, macOS 10.14, tvOS 12): `pathUpdateHandler` receives `NWPath` values with `status`, `availableInterfaces`,
  `gateways`, `usesInterfaceType(_:)`, `supportsIPv4/IPv6`, `isExpensive`, `isConstrained`, `unsatisfiedReason` (including
  `.localNetworkDenied`). It gives no SSID. A Wi-Fi to Wi-Fi switch fires updates on macOS 26.6.2 (see "Apple path monitor and roaming
  on a Mac"); whether `gateways` can tell two networks apart is not verified.
  https://developer.apple.com/documentation/network/nwpathmonitor
- Android: `registerDefaultNetworkCallback`, see above; `onLinkPropertiesChanged` and `onCapabilitiesChanged` report address and capability changes on the same network.
- Browsers: `online`/`offline` on `window` (Baseline since July 2015) but "inherently unreliable": a LAN counts as online even without
  Internet, and on Windows the status depends on reaching a Microsoft server. `navigator.connection` and its `change` event
  (Network Information API) are Chromium only (Chrome and Edge 61+, not Firefox or Safari, per MDN browser-compat-data on 2026-10-19), and describe
  the connection type, not the network identity. https://developer.mozilla.org/en-US/docs/Web/API/Navigator/onLine
- Desktop JVM: nothing in the JDK, see above. What the JDK can do is poll `NetworkInterface` (JDK 21 reference,
  https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/net/NetworkInterface.html, checked 2026-10-20):
  `getNetworkInterfaces()` returns "all the interfaces on this machine" (at least one, possibly loopback; `SocketException` "if an I/O
  error occurs, or if the platform does not have at least one configured network interface"), with `isUp()` ("up and running"),
  `isLoopback()` and `getInterfaceAddresses()`. The reference says nothing about how often the list changes; there is no gateway or
  network identity, so two networks that give the machine the same address look the same.
- Apple, C API of the same monitor (checked 2026-10-20 in Apple's documentation JSON): `nw_path_monitor_set_update_handler` "Sets a
  handler to receive network path updates" (iOS 12, macOS 10.14, tvOS 12); `NWPathMonitor.start(queue:)` "Starts monitoring path changes,
  and sets a queue on which to deliver path events"; `cancel()` "Stops receiving network path updates". The reference does not say that
  the handler is called with the current path when monitoring starts. On macOS 26.6.2 it is, and a Wi-Fi to Wi-Fi switch fires updates
  (see "Apple path monitor and roaming on a Mac"); so on iOS 27.0.1 (see "Roaming on iOS"); tvOS not verified.
  https://developer.apple.com/documentation/network/nw_path_monitor_set_update_handler(_:_:),
  https://developer.apple.com/documentation/network/nwpathmonitor
- Implication (inference): treat every platform signal as "something may have changed" and re-probe the server addresses; none of them
  says which network the device is on.

## Apple path monitor and roaming on a Mac (checked 2026-10-20 on macOS 26.6.2, Apple silicon; ADR 0019)
- What ran: a throwaway `macosArm64Test` binary (not committed) built `networkModule`'s real `RoamingAddressSources` with the real
  `JellyfinServerProbe` and the Apple `NetworkChanges`, for one Jellyfin 12.2 server with a local address (`http://192.168.1.10:8096`,
  first) and an internet address (`https://media.example.com`). It logged, with timestamps, every probe, every `NetworkChanges` element, the
  address in use, `reachable`, and once a second where a request would go and how long `awaitCurrent` held it. A second, independent
  `nw_path_monitor` logged every raw update (status, interface types and names, `isExpensive`; gateways and IPv4/IPv6 support were not
  logged). The owner toggled the network by hand. A command-line test binary, not the app bundle: ATS and the app's `Info.plist` did not
  apply (plain `http` to an IP literal worked), so this says nothing about ATS.
- **The monitor reports the current path at start**: in all four launches the first update came 1-2 ms after `nw_path_monitor_start`,
  with the current path (satisfied, Wi-Fi). So skipping the first update is right: each launch made exactly one check (both addresses at
  once, the local one proved in 10-17 ms, requests held 9-17 ms) and no second one. The first real change after launch was not lost:
  Wi-Fi off 5.5 s after launch, and an update 13.7 s after launch, both cleared the address and started a check.
- **A Wi-Fi to Wi-Fi switch produces updates**, in both directions, when the network is picked in the Wi-Fi menu: the path goes
  unsatisfied for about 0.3 s between the two networks. A move between access points of one network (same SSID) was not tried.
- **Every transition comes as a burst of updates**, 7-11 within 3-5 s, most of them alike in what was logged (satisfied, Wi-Fi, `en0`).
  Opening the Wi-Fi menu alone also produced such updates, with nothing changed. Each update is a `NetworkChanges` element, so each
  one cleared the address in use and restarted the check (ADR 0019's rule).
- Per case, times from the first update of the transition:
  - Wi-Fi off: unsatisfied 0.25 s after the first update; the address was cleared at once, both probes failed (`Unreachable`) within
    0.1 s, requests were held 0.34 s and then failed as unreachable, `reachable` false; the next check came 15 s later, as specified.
  - Wi-Fi on: the first update was still unsatisfied but listed `en0`; the local address was proved 0.13 s later. Eight more updates
    followed in 3 s, each clearing and re-proving it (5-310 ms each); the longest hold the once-a-second sampler saw was 81 ms.
  - Home Wi-Fi to a phone hotspot (`isExpensive` true there): the internet address answered after 0.55 s, but the local address on the
    hotspot timed out (5 s) instead of being refused, and a lower address is used only once the higher ones have failed. The internet
    address was in use 9.2 s after the first update; a request was held 7.5 s. A later update on the hotspot (Wi-Fi menu opened) cleared
    it again and held a request 4.2 s, for the same local timeout.
  - Hotspot back to home Wi-Fi: the local address was in use 0.8 s after the first update (one update in between listed only a wired
    interface); 11 updates in 3.5 s, each re-proved in 8-136 ms.
  - A VPN on and off: a `utun` interface appeared and disappeared in `availableInterfaces`; interface types stayed Wi-Fi (one update
    was "satisfiable" with Wi-Fi and cellular). About 9 updates going on and 7 going off, each re-proving the local address in 5-150 ms.
  - tvOS and iOS: not run (no Apple TV at hand; the simulators share the Mac's network).
- Local network privacy, seen on the side: the binary's very first request to the local address failed as `Unreachable` within 53 ms
  while macOS showed its Local Network prompt; after Allow, the next run reached it. This matches TN3179's "first operation may be refused
  at once" (section "Local network, discovery and plain HTTP: Apple"). Inference: on an app's first launch at home, the local address can
  miss the first check and wait for the 15 s retry.
- Consequence for ADR 0019 (not changed here): counting every update as a change holds up, but bursts and no-change updates cost a check
  each, and on a network where a higher address times out, each such update holds requests for up to the probe timeout.

