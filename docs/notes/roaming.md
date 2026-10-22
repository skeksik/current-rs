# Several addresses per server, and roaming between them (M1 spike)

Research behind ADR 0014: what the servers say about themselves, what proves an address is the right server, how to pick an address,
what each platform reports about the network, and what happens to requests in flight. All sources were read on 2026-10-22 unless a date
says otherwise. Anything not checked against a source is marked "not verified".

Out of scope here and settled elsewhere: URL parsing and the identity probe at setup (ADR 0012, `feat/jellyfin-server-setup`), storing
addresses and caches (ADR 0013, `feat/store`), local server discovery, sign-in, and the UI.

## 1. What the servers offer

### Jellyfin 12.2
Read from the source at tag `v12.2` (commit `ca0f16eb7c19`), https://github.com/jellyfin/jellyfin/tree/v12.2. The hosted OpenAPI
(https://api.jellyfin.org/openapi/) was not compared: not verified that it matches.

- **`GET /System/Info/Public` needs no credentials.** `Jellyfin.Api/Controllers/SystemController.cs`: `[HttpGet("Info/Public")] ...
  GetPublicSystemInfo()` has no `[Authorize]`, nor does the class; `/System/Endpoint` next to it does. There is no fallback
  authorization policy (`Jellyfin.Server/Extensions/ApiServiceCollectionExtensions.cs` sets only `DefaultPolicy`).
- **Fields** (`MediaBrowser.Model/System/PublicSystemInfo.cs`): `LocalAddress`, `ServerName`, `Version`, `ProductName`, `Id`,
  `StartupWizardCompleted`, and `OperatingSystem` (obsolete, always empty). `Emby.Server.Implementations/SystemManager.cs` fills them:
  `Id = _applicationHost.SystemId`, `LocalAddress = _applicationHost.GetSmartApiUrl(request)`.
- **The Id is per installation, not per address.** `Emby.Server.Implementations/Devices/DeviceId.cs`: "Provides the persistent unique
  identifier of this server installation", stored in `device.txt` in the data folder and created as `Guid.NewGuid().ToString("N")` when
  missing. So every address of one server answers the same Id, and a reinstall that loses the data folder gets a new one (read from the
  code; not tested against a server).
- **`LocalAddress` is the server's guess at an address for the asking client, not a list.** `ApplicationHost.GetSmartApiUrl`:
  1. With `EnablePublishedServerUriByRequest` (network setting, default `false`) it echoes the request's own `Host` and scheme.
  2. Otherwise a `PublishedServerUrl` startup setting (command line or environment, `AddressOverrideKey = "PublishedServerUrl"`) wins
     for every client.
  3. Otherwise `NetworkManager.GetBindAddress(remote address)` (`src/Jellyfin.Networking/Manager/NetworkManager.cs`) picks one: a
     `PublishedServerUriBySubnet` entry matching the client (keys `all`, `external`, `internal`, a subnet or an interface; the most
     specific wins), then a bind interface, then the first non-loopback interface, preferring LAN ones. "LAN" is `LocalNetworkSubnets`,
     or all private ranges and loopback when that is empty.
  4. `BaseUrl` is appended as the path.
- **Reverse proxies:** forwarded headers are honoured only when `KnownProxies` is set
  (`ApiServiceCollectionExtensions.cs`: `if (config.KnownProxies.Length == 0) { options.ForwardedHeaders = ForwardedHeaders.None; ...`).
  Without it the server sees the proxy's address and usually answers its own LAN address; a client on the internet is never told a
  public URL unless the admin published one. Read from the code, not run.
- **No endpoint lists all of a server's addresses.** None found in `Jellyfin.Api`; the network configuration is only readable by an
  admin.
- **Auto-discovery** (out of scope, for reference): UDP 7359, request text "who is JellyfinServer?", reply JSON `Address` (the same
  `GetSmartApiUrl` guess), `Id`, `Name` (`src/Jellyfin.Networking/AutoDiscoveryHost.cs`).

What this means for us: `LocalAddress` can seed a suggestion ("this server also answers at ...") but is often a LAN IP the client
cannot reach, or a proxy-side address. The user's list is the source of truth. The `Id` is what ties addresses together.

### Seerr
Read from `develop` at `6f63b08d5c61` (https://github.com/seerr-team/seerr).
- `GET /api/v1/status` (no credentials, `seerr-api.yml` `security: []`): `version`, `commitTag`, `restartRequired`, update fields.
- `GET /api/v1/settings/public` (no credentials): the spec lists `initialized` and `plexClientIdentifier`; the code
  (`server/routes/index.ts`, `server/lib/settings/index.ts`) returns more, including `applicationUrl` (admin-entered, empty by default)
  and `plexClientIdentifier = this.data.clientId`, a `randomUUID()` stored in `settings.json`. That UUID is per installation and is the
  only stable instance identifier found; its name says Plex, so relying on it is a choice M4 must make.
- API key header `X-API-Key` (`server/middleware/auth.ts`). Current signs in to Seerr through the app's Jellyfin session (roadmap M4).

### Stash
Read from `develop` at `bdee3c83d33d` (https://github.com/stashapp/stash).
- No instance identifier in the GraphQL schema (`graphql/schema/`). `version { version hash build_time }` and `systemStatus` exist, but
  when credentials are configured every `/graphql` request without them gets 401 (`internal/api/authentication.go`); only `/login`,
  `/logout`, static assets and `/healthz` (no identity) answer. Without credentials configured, Stash refuses public IPs.
- Credentials: `ApiKey` header or `apikey` query parameter, or a `session` cookie (`pkg/session/session.go`).
- So Stash cannot prove which server answers before the key is sent, other than by TLS.

### What proves an address is the same server
ADR 0006 sends credentials only to verified addresses. Three different things get called "verified":

| Check | Proves | An attacker on the LAN or a captive portal can |
|---|---|---|
| Unauthenticated `Id` matches (Jellyfin) | The answering server says it is the one we know. Catches the honest mistakes: another server, an old reinstall, a router or portal page, a proxy pointing elsewhere | Fake it trivially: `/System/Info/Public` is public, so anyone who has seen it once can replay the `Id` |
| TLS certificate valid for the address's name, checked by the OS verifier (ADR 0006) | The holder of the name's private key answers | Not fake it without a CA's mistake or a root the user installed |
| TLS certificate pinned at setup (self-signed or private CA) | The same key as the one the user accepted | Not fake it after setup; can fake it at setup (trust on first use) |
| Plain HTTP | Nothing | Read and change everything, credentials included |

So the `Id` is a consistency check, not a proof. Proof comes only from TLS. For plain HTTP the only protection is the user's choice,
which ADR 0012 decides (its warning rule for plain HTTP except locally).

Publicly trusted certificates cannot name private IPs: the CA/Browser Forum Baseline Requirements (v2.3.1,
https://github.com/cabforum/servercert/blob/main/docs/BR.md) §4.2.2 "CAs SHALL NOT issue Certificates containing Internal Names or
Reserved IP Addresses", and the IANA special-purpose registries list 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7 and fe80::/10.
Let's Encrypt issues certificates for public IP addresses since general availability on 2026-01-15, as six-day certificates only
(https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability); that does not help a LAN address.

So a LAN address with TLS is one of:
1. **A public name used over the LAN** (split DNS or a hosts entry pointing the public name at the LAN IP): the OS verifier accepts the
   certificate. Split DNS is defined in RFC 9499 §6 (https://www.rfc-editor.org/rfc/rfc9499.txt). Routers with DNS rebinding protection
   drop public answers that point at private addresses: dnsmasq `--stop-dns-rebind`
   (https://thekelleys.org.uk/dnsmasq/docs/dnsmasq-man.html), on by default in OpenWrt
   (https://openwrt.org/docs/guide-user/base-system/dhcp); other routers' defaults not verified.
2. **A private CA the user installed in the OS store**: rustls-platform-verifier (ADR 0006) accepts it like any root.
3. **A self-signed or private-CA certificate the app accepts on its own**: needs either extra roots
   (rustls-platform-verifier 0.7.1 `Verifier::new_with_extra_roots`, "augment[s]" the platform roots; not compiled on Android,
   https://docs.rs/rustls-platform-verifier/0.7.1/) or a pin, which rustls-platform-verifier does not offer (no pinning API in 0.7.1),
   so it would be our own `ServerCertVerifier` around it. Browsers dropped public key pinning for the web (Chrome removed HPKP in
   version 72, https://chromestatus.com/feature/5903385005916160, citing "risks of denial of service and hostile pinning"); pinning one
   self-hosted server the user chose is a different case, but it is still a departure from the OS verifier. ADR 0014 lists it as an
   option for the owner.
4. **Plain HTTP** on the LAN, under ADR 0012's rule.

TLS identity for an IP address is an `iPAddress` subjectAltName, matched octet for octet; SNI cannot carry an IP, so such a server
presents it without SNI (RFC 9525 §6.4 and §7.4, https://www.rfc-editor.org/rfc/rfc9525.txt; RFC 9525 obsoletes RFC 6125).

## 2. Choosing an address

### Order
- **The user's order, not "local first".** The app cannot tell reliably which address is local: a public name may resolve to a LAN IP
  (split DNS), a LAN IP may be reachable over a VPN from far away, and a "local" IP on a hotel network may be someone else's host. The
  user knows which address they prefer. Server setup may suggest an order (LAN addresses first), but the list's order is the preference.
- **Racing removes most of the cost of a wrong guess**: a preferred address that is down costs one stagger delay, not a timeout.

### The race
Happy Eyeballs v2 (RFC 8305, https://www.rfc-editor.org/rfc/rfc8305.txt) races connection attempts with a delay between starts: §5
"One recommended value for a default delay is 250 milliseconds", minimum 100 ms ("MUST NOT be started within 10 milliseconds"),
"upper bound ... 2 seconds". Its v3 draft (draft-ietf-happy-happyeyeballs-v3-04, 2026-07-02, not an RFC,
https://datatracker.ietf.org/doc/draft-ietf-happy-happyeyeballs-v3/) keeps those numbers and adds SVCB/HTTPS records and QUIC, which
do not apply here. RFC 8305 §4 on history: it "MUST NOT be used across different network interfaces and SHOULD be flushed whenever a
device changes the network."

Our race is the same idea one level up: candidates are whole addresses (each of which reqwest resolves and connects to itself), and an
attempt succeeds only when the identity probe passes, not when TCP connects.

1. Candidates in the user's order. If a winner is cached for the current network, it goes first.
2. Start candidate 1. Start the next one 250 ms later, or at once when every running attempt has failed.
3. The first attempt whose identity probe passes wins; the others are cancelled.
4. Each attempt has its own timeout (the probe request's), and the race ends when all have failed.

Whether a higher-preference address that answers a little after a lower one should still win (a "grace" window) was considered and
left out: the upgrade re-check (below) brings the client back to it within minutes, and a grace window delays every start.

### Measurements (throwaway prototype, removed)
A prototype in `crates/net/tests` raced `ServerClient` probes of `GET /System/Info/Public` against stand-in servers on several ports of
127.0.0.1, stagger 250 ms, probe timeout 3 s, 5 runs each, release build, Linux cloud container (kernel 6.18, Rust 1.99.1):

| Candidates (in order) | Winner and time to win |
|---|---|
| ok (5 ms) | 1st, 9-10 ms |
| refused, ok | 2nd, 10-13 ms (a refusal starts the next at once) |
| answers with another `Id`, ok | 2nd, 18-19 ms |
| silent (accepts, never answers), ok | 2nd, 259-262 ms (one stagger) |
| slow (400 ms), ok | 2nd, 260-261 ms |
| slow (150 ms), ok | 1st, 153-155 ms (answers before the stagger ends) |
| unreachable, silent, refused, ok | 4th, 265-274 ms |
| refused, other `Id`, silent | none, after 3.0 s (the probe timeout) |

- **Build one probe client per server and reuse it.** A probe on a reused `ServerClient` took 1.3 ms; building a new client for each
  probe took 156 ms on average (the builder sets up TLS with the platform verifier; cause not profiled).
- A true black hole (SYN never answered) could not be reproduced: the container refuses connections to 192.0.2.1 (TEST-NET-1) at once.
  On a real network an unreachable LAN IP costs the stagger (250 ms) before the next address starts, and the connect timeout before
  that attempt is given up.

### Timeouts
- Probe timeout: a few seconds (3 s in the prototype). It bounds "all addresses down" and is far below the 10 s connect timeout of
  ordinary requests (ADR 0006). The number is to be tuned on real networks.
- The race never runs twice at once for one server; requests arriving during a race wait for it within their own deadline.

### When to re-check
- **On failure:** a request that fails with `Unreachable`, `Timeout` or `Tls` on the selected address starts a race (the selected
  address keeps serving until a winner is found, unless it is the one that failed). `Auth`, `Status`, `Decode` are the server
  answering: no race.
- **On a network change** reported by the UI (section 3): the cached choice for the old network is kept for that network, and a race
  starts at once, with the cached winner for the new network first if there is one.
- **Upgrade check:** while the selected address is not the user's first, re-probe the better ones (only them) periodically, for example
  every 5 minutes, and switch when one passes. Not while nothing is using the server (no requests for a while), so an idle app does not
  poll.
- **When unreachable:** retry with backoff (for example 5 s, doubling to 5 minutes), and at once on a network change or when the app
  comes to the foreground.

### Network conditions
- **Captive portals** (RFC 8952 architecture, https://www.rfc-editor.org/rfc/rfc8952.txt: portals "typically implement some variations
  of forging DNS or HTTP responses. Some attempt man-in-the-middle (MITM) proxy of HTTPS"). A plain-HTTP address gets the portal's page
  or a redirect: the `Id` check fails or the redirect is refused (ADR 0006), so no credentials go there. An HTTPS address gets a TLS
  failure. Platforms report portals (section 3); the core then shows "sign in to the network" rather than "server down".
- **VPNs:** a LAN address may be reachable through a VPN from anywhere, and a VPN may hide the LAN. The race handles both; the VPN fact
  (section 3) is only a re-check trigger.
- **Same hostname, different answers (split DNS):** one URL then serves both places and needs no roaming. Fine: the `Id` check still
  runs.
- **Hairpin NAT:** RFC 4787 REQ-9 and RFC 5382 REQ-8 say NATs MUST support hairpinning; whether consumer routers do is not verified.
  When they do not, the public address fails from inside the LAN, which the race absorbs if a LAN address is also listed.
- **IPv6:** global and ULA (RFC 4193) addresses work like IPv4 ones. Link-local addresses need a zone id, and RFC 9844 (2025-08,
  https://www.rfc-editor.org/rfc/rfc9844.txt) obsoleted the URI syntax for zone ids from RFC 6874; `url` parsing of zone ids not
  verified. Link-local addresses are not supported (ADR 0012 decides what setup rejects).
- **Apple local network privacy** (TN3179, https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy,
  revised 2026-10-06): on iOS 14+ and macOS 15+, an outgoing TCP connection to an address on a Wi-Fi or Ethernet network needs the
  user's "Local Network" permission, from any API ("This includes Network framework, BSD Sockets, URLSession"), and `NSLocalNetworkUsageDescription`.
  The system "may deny the operation immediately, before the user has responded to the alert"; "There's no general API that returns
  whether the current process has local network access." The errno a BSD socket gets on denial is not stated (not verified). Command-line
  tools run from Terminal are exempt, so tests from a terminal will not see it. Consequences: the first race on a Mac may fail every LAN
  address while the alert is up, so the core must re-race when the app regains focus and on the UI's report that the permission
  changed if it can tell; and the macOS app must explain the prompt. Detail for the macOS app task.

### Loopback for tests
Linux routes all of 127.0.0.0/8 to `lo` (RFC 1122 §3.2.1.3; Linux v6.12 `net/ipv4/fib_frontend.c` adds the prefix as `RTN_LOCAL`),
and binding 127.0.0.2 worked in the container. On macOS only third-party reports say 127.0.0.2 needs `ifconfig lo0 alias` first
(for example https://jira.apache.org/jira/browse/CASSANDRA-15323); on Windows nothing was found. Both not verified. The test plan
therefore uses several ports on 127.0.0.1, which every CI runner has.

## 3. Network changes per platform
The UI reports facts; the core decides (as for display facts in ADR 0007). Only facts that need no permission prompt are used: the
Wi-Fi name (SSID or BSSID) costs a location prompt on every platform.

| Platform | Change event | Connectivity | Metered / expensive | Interface | VPN | Network identity without a prompt |
|---|---|---|---|---|---|---|
| Windows | `NetworkInformation.NetworkStatusChanged`; NLM `INetworkEvents` | `ConnectionProfile.GetNetworkConnectivityLevel`: None, LocalAccess, ConstrainedInternetAccess ("possibly due to a captive portal"), InternetAccess | `GetConnectionCost`: `NetworkCostType`, `Roaming`, `OverDataLimit` | `NetworkAdapter.IanaInterfaceType` (6 Ethernet, 71 Wi-Fi, 131 tunnel), `IsWlanConnectionProfile`, `IsWwanConnectionProfile` | tunnel interface type; whether a VPN is its own profile not verified | NLM network GUID ("The GUID of the new network is provided"); `ProfileName`, `GetNetworkNames` not listed as gated |
| macOS, iOS | `NWPathMonitor.pathUpdateHandler` | `NWPath.status`: satisfied, unsatisfied, requiresConnection | `isExpensive` (cellular, hotspot), `isConstrained` (Low Data Mode) | `usesInterfaceType`, `availableInterfaces` | only as interface type `.other` ("virtual networks or networks of unknown types") | `gateways`, interface; SSID needs Location (macOS `CWInterface.ssid`, iOS `NEHotspotNetwork.fetchCurrent`) |
| Linux | GIO `GNetworkMonitor::network-changed` (netlink, NetworkManager, or the Flatpak portal, which needs no user interaction) | `connectivity`: LOCAL, LIMITED, PORTAL, FULL | `network-metered` ("If this information is not available then no networks will be marked as metered") | not in GIO; NetworkManager D-Bus `PrimaryConnectionType` | not in GIO; NetworkManager's "vpn" type not verified | not in GIO; NetworkManager's `PrimaryConnection` (not in the Flatpak portal) |
| Android | `ConnectivityManager.registerDefaultNetworkCallback` (ACCESS_NETWORK_STATE, normal permission) | `NET_CAPABILITY_VALIDATED`, `NET_CAPABILITY_CAPTIVE_PORTAL` | `NET_CAPABILITY_NOT_METERED` | `TRANSPORT_WIFI`, `CELLULAR`, `ETHERNET` | `TRANSPORT_VPN` | `LinkProperties` routes and interface name; SSID needs location (redacted by default since Android 12) |

Sources:
- Windows: `NetworkInformation.NetworkStatusChanged`, `ConnectionProfile`, `NetworkConnectivityLevel`, `NetworkAdapter.IanaInterfaceType`
  (learn.microsoft.com/uwp/api/windows.networking.connectivity.*, pages updated 2025-11-21 and 2025-11-25). `NetworkConnectivityLevel`
  remarks: "The network status reported by Windows APIs is only a hint ... Apps should attempt to connect to their services whenever
  `LocalAccess` or higher connectivity is reported." NLM: https://learn.microsoft.com/en-us/windows/win32/api/netlistmgr/ (updated
  2024-02-22). Windows 11 24H2 location gating: "Changes to API behavior for Wi-Fi access and location"
  (https://learn.microsoft.com/en-us/windows/win32/nativewifi/wi-fi-access-location-changes, updated 2025-06-06) gates BSSID APIs in
  `wlanapi.h`, `Windows.Devices.WiFi` and `NetworkInformation.GetLanIdentifiers`, behind a "one-time-per-app system prompt"; it names
  `WlanConnectionProfileDetails.GetConnectedSsid` as the alternative and does not list `ConnectionProfile` itself. Whether SSID reads
  are gated in practice: not verified.
- Apple: https://developer.apple.com/documentation/network/nwpath, `NWPath.Status`, `NWInterface.InterfaceType`;
  `NEHotspotNetwork.fetchCurrent` (precise location or another listed condition, plus an entitlement); macOS SSID needs Location per an
  Apple engineer quoting `CWInterface.h` (https://developer.apple.com/forums/thread/732431, July 2023; the first macOS version not
  verified).
- Linux: https://docs.gtk.org/gio/iface.NetworkMonitor.html, https://docs.gtk.org/gio/enum.NetworkConnectivity.html,
  https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.NetworkMonitor.html ("not a portal in the strict sense,
  since it does not involve user interaction"), https://networkmanager.dev/docs/api/latest/gdbus-org.freedesktop.NetworkManager.html.
- Android: https://developer.android.com/reference/android/net/ConnectivityManager, `NetworkCapabilities`, `LinkProperties`, `WifiInfo`
  (pages updated 2026-08-03); Wi-Fi permissions, https://developer.android.com/develop/connectivity/wifi/wifi-permissions (2026-10-16).

The common record every platform can fill without a prompt: a "changed" event, connectivity (none, local only, captive portal, full,
unknown), metered (yes, no, unknown), constrained or low-data mode where it exists, interface kind (Wi-Fi, Ethernet, cellular, other,
unknown), VPN (yes, no, unknown), and an opaque network key built by the UI from what it has (Windows: the NLM network GUID; Apple:
interface plus gateways; Linux: the NetworkManager connection, else interface plus gateway; Android: interface plus default route). The
key only has to be stable for one network and different between networks; the core never shows or logs it.

## 4. Behaviour in flight

### Retries
RFC 9110 §9.2.2 (https://www.rfc-editor.org/rfc/rfc9110.txt): idempotent methods (PUT, DELETE and the safe GET, HEAD, OPTIONS, TRACE)
"can be repeated automatically if a communication failure occurs before the client is able to read the server's response"; "A client
SHOULD NOT automatically retry a request with a non-idempotent method unless it has some means to know that the request semantics are
actually idempotent ... or some means to detect that the original request was never applied", and "SHOULD NOT automatically retry a
failed automatic retry".

So, when a request fails on the selected address and a race finds another one:
- **Failed before anything was sent** (DNS, connection refused, connect timeout, TLS handshake; reqwest's `Error::is_connect()`,
  reqwest 0.13.5 `src/error.rs`): retry once on the new address, any method. The request never reached a server.
- **Failed after sending** (timeout or a dropped connection while waiting for the answer): retry once only for GET, HEAD, PUT, DELETE,
  OPTIONS. Never a POST or PATCH: the server may have applied it; the error goes to the caller.
- One retry at most, within the request's original deadline, and only after the race picked a different address.
- Whether Jellyfin's POSTs that are effectively idempotent (marking played, reporting progress) may be retried is for the service crate
  to say per request later; the default is no.

### Playback streams
A stream URL (direct play, transcode, subtitles, images) is built from the address selected when playback starts, and the player holds
it. After a switch the player keeps the old address until that connection fails, and then cannot reconnect by itself. Options for M2:
re-open the stream on the new address at the current position when the player reports a network error; or have the player fetch
through a URL the core can redirect. Jellyfin's transcode sessions are keyed by play session, not by address, so re-opening probably
keeps them (not verified). Credentials in stream URLs (query `api_key`) must also only go to verified addresses.

### The request log
ADR 0006 keeps addresses out of errors and the log. With several addresses, which one served a request matters for debugging, so a log
entry carries the address's position in the user's list ("address 2"), never the address. Probes are logged like any request, with
their position.

## 5. Metered networks (later question)
Every platform reports metered or expensive networks without a prompt (section 3). Whether that should lower streaming quality, pause
downloads or prefetching, or warn before playing, is a later question for M2 (playback) and the downloads work; not decided here.

## Options considered
- **Local first, then the rest** (sort by guessed locality): rejected; the app cannot judge locality reliably (section 2) and the user
  can.
- **Strictly sequential, each address with a timeout**: simplest, but a dead first address costs a full timeout on every network
  change. The race costs 250 ms instead.
- **Probe all addresses at once, prefer the best that answers within a window**: fastest, but hits every address on every check
  (including public ones from inside the LAN) and still needs a window. Not chosen; the stagger gives the preferred address a head
  start with the same worst case.
- **Use `LocalAddress` from the server to add addresses automatically**: rejected as automatic (often unreachable or a proxy-side
  address, and credentials would go to an address the user never chose); fine as a suggestion in setup.
- **Let the UI choose the address** (it knows the network): rejected; logic lives in the core (ADR 0001). The UI reports facts.
