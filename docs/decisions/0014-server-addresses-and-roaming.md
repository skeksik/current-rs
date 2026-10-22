# 0014: Several addresses per server, and roaming between them

Status: proposed (2026-10-22), from the M1 roaming spike; implemented in `current-net`, `current-jellyfin`, `current-model` and
`current-app` (2026-10-22), with the changes listed under "Implementation" at the end. Research, sources and measurements: `docs/notes/roaming.md`. The owner decided
the two open choices on 2026-10-22 (marked **Decided by the owner**).

## Context
A server is often reachable at more than one address: a LAN address at home, a public name elsewhere, sometimes a VPN address. ADR 0006
gave `current-net` an `AddressProvider` seam, asked before every request, with `FixedAddress` as the only implementation, and requires
credentials to go only to verified addresses. ADR 0012 (server setup, in parallel) settles URL parsing, the identity probe and the
warning for plain HTTP; this ADR builds on it and does not redefine it. ADR 0013 (storage) stores the address list.

What the research found (details in the notes):
- Jellyfin's unauthenticated `GET /System/Info/Public` returns an `Id` per installation, the same on every address. Its `LocalAddress`
  is the server's guess for the asking client, often a LAN IP or a proxy-side address; no endpoint lists all addresses.
- Seerr's only stable instance id is a UUID in its public settings named `plexClientIdentifier`; Stash has none, and answers nothing but
  401 without credentials when they are configured.
- The `Id` is public, so anyone on the path can replay it: it catches mistakes (another server, a portal page), not attackers. Only TLS
  proves who answers. Publicly trusted certificates cannot name private IPs.
- A staggered race (Happy Eyeballs, RFC 8305: 250 ms between starts) found the right address in 10-20 ms when the first is up or
  refuses, and about 260 ms when it hangs, on a local prototype. Reusing a probe client matters: 1.3 ms per probe versus 156 ms when a
  client is built each time.
- Every platform reports network changes, connectivity (with captive portals), metered or expensive, the interface and (except GIO) a
  VPN, without a permission prompt. The Wi-Fi name costs a location prompt everywhere. On iOS and macOS 15+ any connection to a LAN
  address needs the user's Local Network permission.

## Decision

### Addresses
- A server has an **ordered list of addresses; the order is the user's preference.** The core does not reorder by guessed locality.
  Setup may suggest LAN addresses first and may offer the server's `LocalAddress` as a suggestion, never add it by itself.
- Every address of a server must pass the **identity check** (ADR 0012's probe: for Jellyfin, the same `Id` from `/System/Info/Public`)
  before it is used. An address that answers a different identity is marked "different server" and skipped until the user edits it;
  the UI shows it in the server's settings. The check runs on every race, not only at setup.
- **Credentials go to an address only after its identity check passed on this connection's race**, and only if the address is HTTPS
  verified by the OS verifier (ADR 0006) or the user accepted it under ADR 0012's rule for plain HTTP. Probes carry no credentials.
- Link-local IPv6 addresses (which need a zone id) are not supported; ADR 0012 decides what setup rejects.

### Choosing an address: the race
1. Candidates in the user's order, skipping "different server" addresses. If a winner is cached for the current network, it goes
   first.
2. Start a probe on candidate 1. Start the next **250 ms** later (RFC 8305's default), or at once when every running probe has failed.
3. The first probe that passes the identity check wins; the others are cancelled.
4. Each probe has a **3 s** timeout (to be tuned on real networks); the race fails when all have failed.
5. One race at a time per server. Requests arriving during a race wait for it, within their own deadline.
6. One probe client (a `ServerClient` without credentials) per address, built once and reused.

### When to race again
- **A request fails** on the selected address with `Unreachable`, `Timeout` or `Tls`. Any other error is the server answering: no race.
- **The network changed** (UI report, below): race at once, the new network's cached winner first. The address in use stops serving
  until it passes its identity check again on the new network (see "Implementation").
- **Upgrade check:** while the selected address is not the first, re-probe only the better ones every 5 minutes while the server is in
  use, and switch when one passes.
- **Unreachable:** retry with backoff from 5 s doubling to 5 minutes, and at once on a network change or when the app returns to the
  foreground.
- The winner is cached per network key (below), in memory only for M1, at most 16 networks per server. Persisting it is an option for
  later (ADR 0013).

### Network facts from the UI
The UI reports facts and the core decides, as with display facts (ADR 0007). Only facts that need no permission prompt.
A new `NetworkFacts` record in `current-model`, passed in through `App::network_changed(facts)` on start and on every platform change
event:

| Field | Values | Windows | Apple | Linux (GIO) | Android |
|---|---|---|---|---|---|
| `connectivity` | none, local only, captive portal, full, unknown | `GetNetworkConnectivityLevel` | `NWPath.status` (no portal) | `connectivity` | `VALIDATED`, `CAPTIVE_PORTAL` |
| `metered` | yes, no, unknown | `GetConnectionCost` | `isExpensive` | `network-metered` | `NOT_METERED` |
| `constrained` | yes, no, unknown | unknown | `isConstrained` | unknown | unknown |
| `interface` | Wi-Fi, Ethernet, cellular, other, unknown | `IanaInterfaceType` | `usesInterfaceType` | unknown (NetworkManager if used) | transports |
| `vpn` | yes, no, unknown | tunnel type (not verified) | `.other` only: unknown | unknown | `TRANSPORT_VPN` |
| `network_key` | opaque string or none | NLM network GUID | interface + gateways | NetworkManager connection, else interface + gateway | interface + default route |

The network key only has to be stable for one network and differ between networks. It can hold addresses, so it is never logged or
shown (its Debug output is redacted) and is kept in memory.

### `current-net`: the provider
`AddressProvider` changes (the sketch as decided; the implemented shape is under "Implementation"):

```rust
pub struct Selected { pub url: Url, pub position: u8 /* 1-based place in the user's list */, /* race generation, private */ }

pub enum AddressOutcome { Ok, Unreachable, Timeout, Tls }

pub trait AddressProvider: Send + Sync {
    /// The address to use now; `None` while none is known. Called before every request: cheap, at most a short lock and an Arc clone.
    fn current(&self) -> Option<Selected>;
    /// Waits until an address is selected or the race fails, up to `deadline`.
    fn wait(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>>;
    /// What happened to a request on `selected`. Never blocks; may start a race. Reports for an older generation are ignored.
    fn report(&self, selected: &Selected, outcome: AddressOutcome);
}
```

`FixedAddress` keeps working: always selected, `report` does nothing. `RoamingAddresses` holds the list, one probe client per address,
an `IdentityProbe` from the service crate (Jellyfin's is ADR 0012's probe), the state and the per-network cache, and runs races on a
tokio task.

States, and what the capabilities state shows (ADR 0005's `Reachability`, unchanged):

| State | Meaning | `Reachability` |
|---|---|---|
| `Idle` | Nothing tried yet | `Unknown` |
| `Racing { serving: None }` | First race, the selected address failed, or the network changed | `Unknown` the first time, else stays as it was until the race ends |
| `Racing { serving: Some(s) }` | Upgrade check; `s` still serves | `Reachable` |
| `Selected(s)` | `s` passed its identity check | `Reachable` |
| `Unreachable { retry_at }` | The last race failed | `Unreachable` |

`ServerStatus` gains the selected address's position (`Option<u8>`) for the server's settings screen. No address reaches the
capabilities state.

### Requests in flight
- On a failure that starts a race, `ServerClient` retries **once**, on the newly selected address, within the request's original
  deadline, and only if the race picked a different address:
  - failed before anything was sent (DNS, refused, connect timeout, TLS handshake: reqwest's `is_connect()`): any method;
  - failed after sending: only GET, HEAD, PUT, DELETE, OPTIONS (RFC 9110 §9.2.2). **Never a POST or PATCH.**
- Redirects stay on the selected address's host (ADR 0006, unchanged).
- **Playback streams** hold the address that was selected when they started; after a switch they fail when the old address does.
  M2 decides how the player re-opens at the current position on the new address. Stream URLs with credentials follow the same rule:
  only verified addresses.
- **The request log** (ADR 0006) gains the address's position: `jellyfin/1 address 2 GET /Items -> 200`. Never the address. Probes are
  logged like requests. Errors stay without addresses.

### Decided by the owner: TLS for servers without a publicly trusted certificate
ADR 0006 checks certificates with the OS verifier only. For a self-signed or private-CA server on the LAN:

| Option | What the user does | Risk |
|---|---|---|
| A. OS trust store only (today's behaviour) | Installs their CA in the OS, or uses a public name over the LAN (split DNS or hosts), or uses plain HTTP under ADR 0012 | None new; harder for users, and a router's DNS rebinding protection can block a public name resolving to a LAN IP |
| B. Extra trusted roots per server (`Verifier::new_with_extra_roots`, rustls-platform-verifier 0.7.1) | Accepts the server's CA certificate once, in setup | A CA the user accepts can sign for any name, but it is used only for that server's client; not available on Android in 0.7.1 |
| C. Pin the certificate's key per address, trust on first use | Accepts the fingerprint once, in setup | An attacker present at setup is pinned instead; a server that renews with a new key breaks until the user accepts again; a custom `ServerCertVerifier` of our own |
| D. Refuse | Nothing | Some self-hosted setups cannot use HTTPS with Current at all |

**Decided (owner, 2026-10-22): A for M1**, revisit B or C if users need it. A needs no new code.

### Decided by the owner: tokio in `current-net`
The provider needs tokio as a normal dependency of `current-net` (today a dev-dependency) with the `rt`, `time` and `sync` features.
tokio is already in `[workspace.dependencies]` and runs the core (ADR 0002), so this adds features, not a crate. The tests would like
`test-util` (paused time) as a dev feature. **Approved (owner, 2026-10-22)**: `rt`, `time` and `sync` as a normal dependency,
`test-util` for tests.

### Metered networks
Later question, not decided: whether a metered or constrained network should lower quality, pause downloads or warn (M2 and
downloads). The facts are already reported.

### Per service
- **Jellyfin:** as above.
- **Seerr and Stash (M4):** M4 decides their identity check. Seerr could compare `plexClientIdentifier` from `/api/v1/settings/public`
  (a per-installation UUID despite its name); Stash has no identifier, so its addresses are tied together only by TLS and the user's
  choice. Seerr signs in through the app's Jellyfin session (roadmap M4), Stash with an API key.

## Test plan (implementation task)
Stand-in servers on **several ports of 127.0.0.1** (not 127.0.0.2+, which macOS may not have; not verified), hand-written as in
`crates/net/tests/client.rs`:
- Race: first up wins; first refused, second wins at once; first silent, second wins after one stagger; first slow but within the
  stagger wins; a wrong-`Id` server is skipped and marked "different server"; all down ends `Unreachable` within the probe timeout.
- Cache: a network key's winner goes first; a different key does not use it; a race on a known network still checks identity.
- Triggers: a failed request starts one race (not one per request); `Auth` and `Status` do not; a network change starts a race; the
  upgrade check moves back to address 1 when it returns and stops while idle; backoff grows and resets on a network change.
- Credentials: no probe carries `RequestAuth`'s headers or query; no request goes to a wrong-`Id` address (checked on the stand-in's
  received requests).
- Retries: a GET retried once on the new address after a connect failure; a POST after a dropped response is not retried; no second
  retry.
- Log and errors: entries carry `address N`; no log line, error or Debug output contains a stand-in's address or the network key.
- Reachability: the states map as in the table.
- Timing tests use tokio's paused time (`test-util`, approved).
- Apple's Local Network permission cannot be tested in CI (Terminal-run tools are exempt): checked by hand on a Mac in the macOS app task.

## Implementation (2026-10-22)
What the implementation fixed, and where it departs from the decision above (agreed with the manager, 2026-10-22; the owner is told at
merge):

- **A network change stops serving the address in use until it passes its identity check again** (`Racing { serving: None }`, not
  `Some`). Otherwise credentials could go to the old address on the new network (for example `192.168.1.10` on another network,
  someone else's host) before any check, against this ADR's own rule that credentials go only to an address that passed in this
  race. Requests wait for the race within their deadline; the cached winner goes first, so this usually costs one probe. Reachability
  stays as it was during that race.
- **The API as built** (`current-net`):
  - `Selected` has `url()` and `position() -> Option<u8>`: `None` for `FixedAddress`, whose log lines therefore carry no position.
    Debug shows the position only.
  - `AddressProvider::wait` takes a `tokio::time::Instant`, so that tests can use tokio's paused time.
  - `IdentityProbe::check(&self, client: &ServerClient) -> IdentityCheck` (`Passed`, `DifferentServer`, `Failed`); `current-net`
    builds the probe client per address and passes it in. Why a probe failed is in the request log.
  - `RoamingAddresses::new(server, addresses, probe, log, RoamingConfig, &RuntimeHandle)`. `RuntimeHandle` is tokio's
    `runtime::Handle`, re-exported so that callers need no tokio features. `network_changed(&facts)`, `retry_now()` and `status()`
    (reachability, the position in use, and the positions of "different server" addresses).
  - `LogEntry` gains `address: Option<u8>`.
- **`current-app`:** `App::network_changed(facts)` fans out to every provider made by `App::jellyfin_addresses(server, id,
  addresses, runtime)`, and providers made later start from the last facts. `App::returned_to_foreground()` makes unreachable servers
  race at once. Wiring providers into signed-in servers and the capabilities state is left to sign-in.
- **While `Unreachable`, `wait` returns at once** instead of holding requests until the next retry: a request fails fast, and the
  backoff races go on.
- **Connectivity `None` skips probing:** the provider goes to `Unreachable` with no retry timer until the next report.
- **Facts before the first request only record the facts** (no race until something uses the server), and a report equal to the
  last one is not a change.
- **An address that needs the user's confirmation for plain HTTP (ADR 0012) and lacks it is never probed or used.** It keeps its
  position, so the log matches the user's list.
- **Jellyfin's identity check** (`JellyfinIdentity`) runs ADR 0012's info request on the reused probe client and compares the `Id`.
  Unlike setup, it does not move to a base path when the server answers with its web client: a stored address is already the base.
- **Retries and "nothing was sent":** a failure counts as before sending only when reqwest's `Error::is_connect()` is true. Read on
  2026-10-22: hyper-util 0.1.21 `src/client/legacy/client.rs` wraps the connector's error as `ErrorKind::Connect`
  (`.map_err(|src| e!(Connect, src))`), which reqwest 0.13.5 `src/error.rs` `is_connect()` looks for, and reqwest's rustls connector
  (`src/connect.rs`, `hyper_rustls::HttpsConnector`) does the TLS handshake inside that connector. So DNS, refused connections,
  connect timeouts and failed handshakes are "not sent"; a timeout from the request's own deadline is not (it counts as sent), and
  after a followed redirect everything counts as sent.

## Consequences
- A dead preferred address costs one stagger (250 ms) after a network change, not a timeout.
- The `Id` check keeps credentials away from wrong servers and portal pages, but on plain HTTP nothing stops an attacker on the path;
  that remains ADR 0012's warning.
- Each platform UI implements one small network observer that maps its API to `NetworkFacts`; the macOS and iOS apps must also explain
  the Local Network prompt, and the core re-races when the app returns to the foreground because the first race may fail while the
  prompt is shown.
- M2 must handle a playback stream whose address went away.

## Sources (read 2026-10-22)
Full list with quotes in `docs/notes/roaming.md`. Main ones:
- Jellyfin v12.2 source (commit ca0f16eb7c19): `Jellyfin.Api/Controllers/SystemController.cs`, `Emby.Server.Implementations/SystemManager.cs`,
  `ApplicationHost.cs` (`GetSmartApiUrl`), `Devices/DeviceId.cs`, `src/Jellyfin.Networking/Manager/NetworkManager.cs`:
  https://github.com/jellyfin/jellyfin/tree/v12.2
- Seerr `develop` at 6f63b08d5c61 (`seerr-api.yml`, `server/lib/settings/index.ts`); Stash `develop` at bdee3c83d33d
  (`graphql/schema/`, `internal/api/authentication.go`)
- RFC 8305 (Happy Eyeballs v2); draft-ietf-happy-happyeyeballs-v3-04 (2026-07-02); RFC 9110 §9.2.2; RFC 8952; RFC 9525; RFC 9844
- CA/Browser Forum Baseline Requirements v2.3.1 §4.2.2; Let's Encrypt, IP address certificates generally available (2026-01-15)
- rustls-platform-verifier 0.7.1: https://docs.rs/rustls-platform-verifier/0.7.1/
- Windows `Windows.Networking.Connectivity` and "Changes to API behavior for Wi-Fi access and location" (updated 2025-06-06); Apple
  `NWPath` and TN3179 (revised 2026-10-06); GIO `GNetworkMonitor`; Android `ConnectivityManager` and `NetworkCapabilities`
