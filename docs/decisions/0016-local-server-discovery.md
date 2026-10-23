# 0016: Local server discovery

Status: proposed (2026-10-22); accepted when its PR merges. The owner decides the open choices marked **Owner decides**.

## Context
The "add a server" screen (ADR 0012) should list Jellyfin servers on the local network next to the field for a typed address.
Jellyfin has its own UDP discovery. This ADR covers how Current uses it, what the platforms require, and what is trusted. Facts
were read on 2026-10-22 from the sources at the end. "Jellyfin" means the server at tag `v12.2`.

## What the server does
- `AutoDiscoveryHost` binds `new UdpClient(new IPEndPoint(IPAddress.Any, 7359))`. That is **IPv4 only**, port **7359**, and no
  multicast group is joined, so clients reach it by broadcast (or unicast).
- A datagram whose UTF-8 text contains `who is JellyfinServer?` (compared without case) gets a **unicast reply to the sender's
  endpoint**. The reply is `ServerDiscoveryInfo`, serialized by System.Text.Json with default names:
  `{"Address": ..., "Id": ..., "Name": ..., "EndpointAddress": null}`.
- `Address` is `GetSmartApiUrl(remoteIp)`:
  - the published server URL from the startup configuration, if one is set;
  - otherwise the bind address chosen for that client, with scheme, port and base path (`GetLocalApiUrl`).
  It can name an address this device cannot reach, for example one inside a container. When the server cannot work out an
  address it sends no reply.
- `Id` is the `SystemId` that `/System/Info/Public` also returns (ADR 0012). `Name` is the friendly name.
- Discovery is turned off by the server setting `AutoDiscovery` (`NetworkConfiguration`, default `true`). It runs only once the
  main server has started, not during the startup setup server.
- Jellyfin's Kotlin SDK 1.9.2 sends the request once, to `255.255.255.255:7359` only. It reads with a 1024-byte buffer and a
  500 ms timeout per receive, takes at most 15 servers, merges by `Id`, and replaces `EndpointAddress` with the packet's source
  IP. The TypeScript SDK 1.0.1 has no discovery (a browser cannot send UDP).

## Decision
- **Request:** `who is JellyfinServer?`, with `SO_BROADCAST`, from several sockets (amended 2026-10-23, below):
  - one bound to `0.0.0.0:0`, sending to `255.255.255.255:7359`, as before;
  - one bound to each local interface address the platform lists (`<address>:0`), sending to that interface's subnet broadcast
    address and to `255.255.255.255`, both on port 7359.

  Every socket sends once more after 500 ms in case a datagram was lost. Replies are read from all sockets, in turn so a busy
  one cannot starve the others, in the same window and under the same limits.
- **Collection window: 2 s.** Windows Firewall accepts unicast replies to a broadcast for about 3 s (below), so the window and
  the resend stay inside that.
- **Limits** (constants in `current-jellyfin`'s `discovery.rs`, next to the Windows note):
  - a reply of more than 2048 bytes is dropped whole;
  - at most 32 servers and 256 datagrams per search, across all sockets;
  - at most 16 interfaces, counted after the unusable and repeated ones are dropped.
- **Strict parsing:**
  - the reply must be one JSON object (hence UTF-8);
  - `Id` must be 1 to 64 ASCII letters, digits or hyphens;
  - `Address` must pass ADR 0012's `parse_address` (http or https only, no credentials, a host), and its first candidate is used;
  - unknown fields are ignored, so a newer server's additions pass;
  - `Name` loses control characters and is cut to 256 characters.
  Replies claiming the same `Id` are merged. The first reply's name is kept, and every reply's addresses are added in arrival
  order, deduplicated, at most 4 per `Id`. So a spoofed reply that arrives first cannot hide the real server's address: the
  addresses are probed in order until one answers with the claimed `Id`. This does not make the `Id` a proof (see below).
- **The reply's source:** when `Address` names an IP other than the one the reply came from, an address with the source IP (same
  scheme, port and base path) is probed first, then the claimed one. A reply that names a host (a published URL) is probed as
  given.
- **Replies are untrusted.** Any host on the network can answer, with any `Address`, including a public host. A candidate is shown
  only after ADR 0012's prober (`probe_expecting`) reaches a usable Jellyfin server at one of its addresses and that server answers
  with the `Id` the reply claimed. The UI shows the probed name and version, not the reply's.
  - So a forged reply can make Current send one unauthenticated `GET /System/Info/Public` to a host of the forger's choosing. The
    probe carries no credentials, and the address goes through ADR 0012's plain-HTTP rule when picked. The risk is small and
    accepted.
  - Picking a result hands it to "add a server" as a probed server, so it ends `Ready`, or `ConfirmPlainHttp` for plain http
    outside the local network.
- **Probes:** at most **8 at once**, so 32 bogus replies cannot fan out into dozens of connections. Each candidate's addresses are
  tried one after another. Discovery probes use shorter timeouts than setup (2 s to connect, 5 s per request) and are logged as
  `jellyfin/discovery`.
- **Servers that answer but cannot be used are not listed.** That covers too old, setup unfinished and unavailable; the user can
  still type the address and get ADR 0012's error.
- **State** (`current_app::Discovery`): `Idle`, `Searching { found }` (growing as probes confirm), `Done { found }`, and
  `Failed { reason }`. The reason is one of:
  - `NotPermitted`: the OS refused;
  - `NoNetwork`: unreachable or no address;
  - `Failed`: anything else.
  These are sorted by the I/O error's kind only. A socket that cannot be bound, or whose every send fails, is left out; the search
  fails only when every socket failed, with `NotPermitted` if any of them was refused that way (the reason the user can act on),
  otherwise the first failure. A new search or a reset supersedes one still running. Nothing in the state, its Debug output or an
  error holds an address, including the interfaces' own.
- **No IPv6:** the server does not listen on it.
- **Interfaces** (amended 2026-10-23):
  - Rust's standard library (and tokio) cannot list interfaces or their netmasks. Listing them would take `unsafe` calls to
    `getifaddrs` (forbidden here) or a new dependency. So each shell lists its up IPv4 interfaces, address and prefix length, as
    a platform fact (`LocalInterface`, like ADR 0014's `NetworkFacts`) and passes them to each search, read just before it. The
    core decides what to do with them.
  - Not part of `NetworkFacts`: those feed the roaming "same network" comparison, and the list must be fresh at each search.
  - Dropped: loopback (127/8), unspecified, multicast and broadcast addresses, prefixes over 32, then repeated addresses, then any
    after the first 16.
  - Kept: link-local 169.254/16. Jellyfin listens on every address, and on a link without DHCP (a direct cable) it is the only
    address there is. A shell passes only up interfaces, so the cost is two datagrams per such adapter.
  - Targets: the subnet broadcast address (none for /31, /32 or /0) and `255.255.255.255`, without repeats. Both, because the
    platforms differ in where a limited broadcast from a bound socket goes:
    - on Windows it leaves on the interface the socket is bound to: in the owner's run (below), a socket bound to the wired or
      the Wi-Fi address got a reply every time;
    - on Linux, the route for `255.255.255.255` may pick the interface whatever the bound address (**not verified**), so the
      subnet broadcast is the one that reaches that network there.
  - The unbound socket stays next to them: it costs one socket and covers an adapter the shell filtered out or one that came up
    between listing and sending. Replies to it and to an interface's socket merge by `Id`.
  - Apple's TN3179 advises running a broadcast discovery "on *all* broadcast-capable interfaces".
  - The Windows app lists them with .NET's `NetworkInterface`; the macOS app (`getifaddrs` in Swift) is a later task and passes an
    empty list until then, which is the unbound socket alone.
- **No mDNS or DNS-SD:** Jellyfin does not advertise itself that way. A search of the v12.2 source for mDNS, DNS-SD, Bonjour and
  zeroconf finds nothing.

### Owner decides: no firewall rule in the Windows installer
- Windows Defender Firewall's profile setting "allow unicast response to multicast or broadcast" defaults to on: "The computer
  can receive unicast responses to outgoing multi-cast or broadcast messages… The default setting when managing a computer is
  True." The archived Windows Server 2008 page says it "waits as long as three seconds for unicast responses from the other
  computers and then blocks all later responses". That figure is not verified on current Windows.
- So the replies need no inbound rule. An inbound rule would also need admin rights, which the per-user installer (ADR 0009)
  does not have.
- Microsoft says the first-run prompt follows a *listen call*, and a non-admin user's answer creates block rules whatever they
  click. Whether a UDP socket bound to `0.0.0.0:0`, used only to send and read replies, raises that prompt is **not verified**.
- **Proposed:** no rule, and no installer change. If the owner's local check shows a prompt, try binding to a specific local
  address first. Revisit a rule (in a machine-wide install only) if that fails.
- 2026-10-23: the owner's run (Amendment, below) showed no prompt and created no rule, from unbound sockets or sockets bound
  to an interface address.

### Platforms
- **macOS 15+ and iOS 14+:** sending a UDP broadcast is a local network operation, and the first one shows the Local Network
  prompt. The system "may deny the operation immediately, before the user has responded to the alert". So:
  - the apps need `NSLocalNetworkUsageDescription` in Info.plist;
  - the screen offers "search again" (or searches again when the app returns to the foreground);
  - `NotPermitted` explains where to grant the permission.
  - On macOS a denial may surface as "no route to host", which would be reported as `NoNetwork` (not verified; the macOS app task
    checks it).
  - Tools run from Terminal are exempt, so tests and CI never see the prompt.
- **iOS:** sending or receiving broadcast also needs the `com.apple.developer.networking.multicast` entitlement, which Apple grants
  on request (the request process is not verified). TN3179: "The multicast entitlement isn't required on macOS". Receiving the
  unicast reply needs nothing more.
- **Linux, including Flatpak:** nothing beyond `SO_BROADCAST`. Flatpak's `--share=network` grants "Access the network", and the
  documentation mentions no separate permission for broadcast or the local network.
- **Android (later):** `ACCESS_LOCAL_NETWORK` is a runtime permission enforced for apps targeting SDK 37 (Android 17). Whether a
  plain broadcast is covered is not verified.

## Consequences
- A search takes about 2 s plus the probes, which run while results already show.
- A search sends from up to 17 sockets, two datagrams each (plus the resend), on a device with many adapters. Small, and bounded.
- A server answers every socket that reaches it, up to four times each (two targets, two sends). With several sockets on one LAN
  the shared 256-datagram limit can end a search after about 25 servers instead of 32. Acceptable for a home network.
- A shell that passes no interfaces (the macOS app until its task) still misses servers on interfaces other than the one the OS
  picks for the limited broadcast.
- A server that publishes an address unreachable from this device is still found through the reply's source IP when that IP is
  reachable.
- Not tested in CI: a real LAN, the Windows firewall's behaviour and the Apple prompt. The tests use UDP and HTTP stand-ins on
  127.0.0.1. The owner's local check (wave 3) verifies these on a real network:
  - whether Windows shows a firewall prompt;
  - whether replies arrive with the default profile;
  - macOS's Local Network prompt.

## Amendment (2026-10-23)
The owner's run of the Windows app on a PC with Wi-Fi and wired on the same LAN, plus virtual (WSL/Hyper-V) and VPN adapters,
found nothing on the first search, and never while wired: the lowest-metric route for `255.255.255.255` was a virtual adapter. From
a shell with the same request, an unbound socket sending to `255.255.255.255:7359` got 1 reply in 9 tries; a socket bound to the
wired or the Wi-Fi address got a reply every time. No firewall prompt appeared and no rule was created. The owner approved the
per-interface rule above (2026-10-23), as the Interfaces bullets of the first version anticipated.

## Sources (read 2026-10-22)
- Jellyfin server, tag `v12.2` (commit ca0f16eb), https://github.com/jellyfin/jellyfin:
  - `src/Jellyfin.Networking/AutoDiscoveryHost.cs`
  - `MediaBrowser.Model/ApiClient/ServerDiscoveryInfo.cs`
  - `MediaBrowser.Common/Net/NetworkConfiguration.cs` (`AutoDiscovery`)
  - `Emby.Server.Implementations/ApplicationHost.cs` (`GetSmartApiUrl`, `GetLocalApiUrl`)
  - `src/Jellyfin.Networking/Manager/NetworkManager.cs` (`GetBindAddress`)
  - `Jellyfin.Server/Startup.cs`
- Jellyfin Kotlin SDK v1.9.2 (commit 88f916be): `jellyfin-core/src/jvmCommonMain/kotlin/org/jellyfin/sdk/discovery/LocalServerDiscovery.kt`,
  `jellyfin-model/.../api/ServerDiscoveryInfo.kt`. Jellyfin TypeScript SDK v1.0.1 (commit 26191928): `src/`.
- Microsoft:
  - `Set-NetFirewallProfile` (`-AllowUnicastResponseToMulticast`, `-NotifyOnListen`):
    https://learn.microsoft.com/powershell/module/netsecurity/set-netfirewallprofile
  - "Windows Firewall rules" (updated 2025-06-06):
    https://learn.microsoft.com/windows/security/operating-system-security/network-security/windows-firewall/rules
  - Archived "Customize Settings for a Firewall Profile" (Windows Server 2008):
    https://learn.microsoft.com/previous-versions/windows/it-pro/windows-server-2008-r2-and-2008/cc742429(v=ws.10)
- Apple, TN3179 "Understanding local network privacy" (revised 2026-10-06):
  https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy
- Flatpak, "Sandbox Permissions": https://docs.flatpak.org/en/latest/sandbox-permissions.html
- Android, "Local network permission": https://developer.android.com/privacy-and-security/local-network-permission
