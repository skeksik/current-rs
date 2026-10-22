# 0012: Jellyfin server setup

Status: accepted (2026-10-22).

## Context
M1 starts with adding a Jellyfin server by its address. Users type anything from `192.168.1.10` to a pasted web client URL; servers
run on Jellyfin's ports or behind a reverse proxy, under a base path or not, over https or plain http. Current must find the server,
tell whether two addresses lead to the same server (several addresses and roaming come later, ADR 0014), refuse servers it cannot
work with, and decide when plain HTTP needs the user's agreement. Facts below were read on 2026-10-22 from the sources listed at the
end; "Jellyfin" means the server at tag `v12.2`.

## What the server offers
- **`GET /System/Info/Public`**, unauthenticated (`SystemController.GetPublicSystemInfo`, built in `SystemManager.cs`), answers
  `PublicSystemInfo` in PascalCase JSON, every field nullable in the OpenAPI document:
  - `Id`: the server's `SystemId`, a GUID (32 hex digits) kept in `<data>/device.txt` and generated on first start when missing or
    invalid (`Devices/DeviceId.cs`). It is the same on every address. A fresh install with a new data folder gets a new id; moving
    or restoring the data folder keeps it; two servers started from one copied data folder share it.
  - `ServerName`: the configured name, else the machine name. It changes freely: shown, never used as identity.
  - `Version`: `ApplicationVersion.ToString(3)`, e.g. `12.2.0`.
  - `ProductName`: the assembly product, `Jellyfin Server`.
  - `LocalAddress`: the server's own idea of its URL (the published server URL, else its bind address). A hint only.
  - `StartupWizardCompleted`: whether first-run setup is done; nullable "only for backwards compatibility in API clients".
  - `OperatingSystem`: deprecated, never set.
- While the server starts, a temporary setup server answers this endpoint with `StartupWizardCompleted: false` hard-coded
  (`ServerSetupApp/SetupServer.cs`), or 503 with `Retry-After` (documented in the OpenAPI document). When remote access is off or an
  IP filter blocks the client, `IpBasedAccessValidationMiddleware` answers 503 without a body or `Retry-After`.
- **Ports:** 8096 for http and 8920 for https by default (`NetworkConfiguration.DefaultHttpPort`, `DefaultHttpsPort`).
- **Base path:** `BaseUrl` is normalised to `/x` without a trailing slash; nothing forbids `/web`. `BaseUrlRedirectionMiddleware`
  runs first and sends any path outside the base path to `{BaseUrl}/web/` with a relative redirect on the same scheme, host and port.
  The web client is served at `{BaseUrl}/web/`.
- **https:** with `RequireHttps`, ASP.NET Core's `UseHttpsRedirection` sends http requests to https on the https port (8096 to 8920).
- **Jellyfin's own clients:** both SDKs prefix `http://` when no scheme is typed, add https and Jellyfin-port candidates and prefer
  https; an explicit scheme and port is used as is. The Kotlin SDK 1.9.2 requires server 12.0.0 or newer and flags a product name
  other than `Jellyfin Server`.

## Decision
- **Parsing** (`current_jellyfin::parse_address`): trim; only http and https; no user name or password in an address; a bare IPv6
  literal gets brackets. Names become punycode through `url` (the typed text stays what the UI shows). IPv6 zone ids (`fe80::1%eth0`)
  are not part of a URL in the WHATWG URL Standard that `url` follows, so they are refused. Query and fragment are dropped, the
  path is the base path without trailing slashes, and a pasted web client URL is cut before its last `web` segment (a server whose
  base path is `/web` itself keeps it when its own web client URL, `/web/web/...`, is pasted).
- **Candidates**, tried in order until a Jellyfin server answers:

| Typed | Candidates |
|---|---|
| scheme and port | exactly that |
| scheme, no port | the scheme's default port, then 8096 (http) or 8920 (https) |
| port, no scheme | both schemes on that port, http first for 80 and 8096, else https first |
| neither | `https://h`, `https://h:8920`, `http://h:8096`, `http://h` |

  A name that does not resolve on this device gets only its first candidate tried: every candidate shares the name. (A proxy may
  still resolve it, so one attempt is made.) Probes run one after another with short timeouts (5 s to connect, 10 s per request);
  a dead host that drops packets can take four connect timeouts. Racing candidates in parallel comes with roaming (ADR 0014).
- **Probe** (`Prober::probe`, `probe_url`): `GET {base}/System/Info/Public` through `current-net`, which follows same-host redirects
  only (ADR 0006). `current-net`'s `Response` now carries the URL it came from (a `Location`, hidden from Debug and never logged), so
  the probe adopts an http to https upgrade as the server's address, and when it lands on the web client it retries once under the
  base path before that `web` segment. A redirect to another host or port stays unfollowed and is reported with its target for the
  UI to offer.
- **Accepting a server:** `Id` and `Version` must be present and non-empty. The version, read leniently (its leading dotted numbers,
  so a fork's suffix passes), must be at least **12.0.0** (ADR 0002: latest releases only). The product name is kept and shown, not
  checked: forks may name themselves differently. `StartupWizardCompleted: false` refuses the server; a missing value counts as done.
- **Errors** (`ProbeError`), one per thing the UI says: `InvalidAddress` (with the reason), `Unreachable`, `Timeout`, `Tls`,
  `NotJellyfin` (an answer without Jellyfin's fields, a web page, 404), `Unavailable` (503: starting, or not accepting this network;
  Jellyfin answers both alike), `Redirected` (with its target), `Status`, `TooOld` (with the version), `SetupIncomplete` (the text
  says to finish setup in a browser, or wait if the server is still starting), and `DifferentServer` for `probe_expecting`. When
  every candidate fails, the most telling error wins (Unavailable, then Redirected, Tls, NotJellyfin or Status, Timeout,
  Unreachable); `TooOld` and `SetupIncomplete` end the search, since a Jellyfin server answered. No error holds an address.
- **Identity:** `ServerId` (model) is the server's `Id`, compared byte for byte; `same_server` compares two answers and
  `probe_expecting` checks an address leads to a known server. The app's own `ServerKey` (ADR 0005) stays the identity in the app,
  since a reinstall changes the server's id.
- **https that fails:** when an https candidate fails its TLS handshake and plain http then answers, the result says so
  (`https_failed_tls`), so the UI can tell the user the server offers https with a certificate this device does not trust instead of
  silently settling for http. Whether Current may trust a self-signed certificate is ADR 0014's question, for the owner.
- **Plain HTTP** (`current_net::transport`, service-agnostic so that roaming and the other services share it): https is silent.
  Plain http is silent to this device (127.0.0.0/8, `::1`, `localhost`, `*.localhost`) and the local network: private IPv4 (RFC
  1918), link-local (169.254.0.0/16, `fe80::/10`), IPv6 unique local (`fc00::/7`), IPv4-mapped forms of these, single-label names,
  and names under `.local` (mDNS, RFC 6762), `.home.arpa` (RFC 8375) and `.internal` (reserved by ICANN for private use). Anywhere
  else the user confirms it **once per address**; the confirmation is kept on the address (`ServerAddress::plain_http_confirmed`), so
  `current-store` persists it and no connection asks again. The address is classified as written, not by what a name resolves to,
  so a public name that resolves to a LAN address is confirmed once. The rule is applied to the address the server answered on, so a
  server that upgrades http to https needs no confirmation.
- **Shared address space** (100.64.0.0/10, RFC 6598) is **not** local: an ISP's carrier-grade NAT puts other subscribers in it, and
  Jellyfin's own default local networks leave it out. Mesh VPNs also hand out addresses there; their users confirm once.
- **Flow** (`current_app::AddServer`, in memory): `Idle`, `Probing`, `Failed`, `ConfirmPlainHttp`, `Ready`. A newer probe or a reset
  supersedes one still running. Storing the result is `current-store`'s; signing in comes later. Probes are logged as
  `jellyfin/setup`.

### Why this plain-HTTP rule (ADR 0001: follow the platforms)
- Apple's App Transport Security exempts "unqualified domains, .local domains, and IP addresses" with `NSAllowsLocalNetworking`;
  everything else must be https unless the app declares an exception.
- Android 9 (API level 28) and later disable cleartext traffic by default; an app opts in, per domain or globally.
- Chrome 154 (October 2026) turns on "Always Use Secure Connections" for public sites only: private sites (local IP addresses,
  single-label hostnames) are exempt, and a public http site is warned about once, not on every visit.
- No Windows rule for apps was found (not verified further).
Current follows Chrome's split: silent for private addresses, one confirmation per public address. The macOS app will need
`NSAllowsLocalNetworking` and an ATS exception path for confirmed public http addresses (not built yet).

## Consequences
- Typing `192.168.1.10` probes two https candidates before http on 8096; on a LAN the refused connections fail fast.
- `LocalAddress` is not used; roaming may use it as a hint (ADR 0014).
- A server behind a login portal on another host fails with `Redirected` until the user enters the address it points to (ADR 0006).
- The https upgrade path is tested at the level of URL handling only: there is no TLS stand-in server in the tests.

## Sources (read 2026-10-22)
- Jellyfin server, tag `v12.2` (commit ca0f16eb, 2026-10-05), https://github.com/jellyfin/jellyfin: `Jellyfin.Api/Controllers/SystemController.cs`,
  `Emby.Server.Implementations/SystemManager.cs`, `MediaBrowser.Model/System/PublicSystemInfo.cs`,
  `Emby.Server.Implementations/Devices/DeviceId.cs`, `Emby.Server.Implementations/ApplicationHost.cs` (`SystemId`, `FriendlyName`,
  `GetSmartApiUrl`), `MediaBrowser.Common/Net/NetworkConfiguration.cs`, `Jellyfin.Api/Middleware/BaseUrlRedirectionMiddleware.cs`,
  `Jellyfin.Api/Middleware/IpBasedAccessValidationMiddleware.cs`, `Jellyfin.Server/Startup.cs`,
  `Jellyfin.Server/ServerSetupApp/SetupServer.cs`, `MediaBrowser.Common/Net/NetworkConstants.cs`,
  `src/Jellyfin.Networking/Manager/NetworkManager.cs`.
- Jellyfin OpenAPI, https://api.jellyfin.org/openapi/jellyfin-openapi-stable.json (version 12.2.0): `GetPublicSystemInfo`,
  `PublicSystemInfo`.
- Jellyfin Kotlin SDK v1.9.2 (commit 88f916be): `discovery/AddressCandidateHelper.kt`, `RecommendedServerDiscovery.kt`,
  `RecommendedServerIssue.kt`, `Jellyfin.kt` (`minimumVersion = 12.0.0`). Jellyfin TypeScript SDK v1.0.1 (commit 26191928):
  `src/utils/address-candidates.ts`, `src/utils/url/`, `src/versions.ts`.
- Apple, `NSAllowsLocalNetworking`: https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking
- Android, network security configuration: https://developer.android.com/privacy-and-security/security-config
- Chrome Security, "HTTPS by default" (October 2025): https://security.googleblog.com/2025/10/https-by-default.html
- RFC 1918, RFC 3927, RFC 4193, RFC 4291, RFC 6598, RFC 6761, RFC 6762, RFC 8375; WHATWG URL Standard (host parsing).
