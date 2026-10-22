# 0019: Roaming between server addresses

Status: accepted (2026-10-20), with several addresses per server.

## Context
A saved server keeps several addresses in the user's priority order (ADR 0016): local ones (`http://192.168.1.10:8096`,
`http://media.lan:8096`) and internet ones (`https://media.example.com`). ADR 0015 made every client read its address from a
`ServerAddressSource` for each request, so roaming needs a smarter source, not new clients. Credentials must reach only an address that
proved to be the same server. docs/platform-notes.md ("Detecting network changes") found no platform that says which network the device
is on, so a signal can only mean "something may have changed".

## Decision
- **One event loop per server.** `RoamingServerAddress` handles every input in order from a single queue: an edit, a network change
  and its settle, a resumed network, a request (no address, or the address in use failing), a timer, and each probe's result. Only the
  loop changes the state (the address in use, whether requests are held, the check running, the retry wait); requests read what it
  publishes and wait on it. The settle and the timers are scheduled events, and probes run as child jobs that post their results;
  each carries a token of the settle, timer or check it belongs to, and the loop ignores one that has been replaced. Requests,
  resumed networks and failures (one per address) are queued only when one is not already waiting, so the queue stays bounded however fast they come. The rules
  below are the loop's.
- **One `RoamingServerAddress` per saved server**, made by `RoamingAddressSources` (bound in Koin in place of the first address),
  started for every saved server and shared by every client of that server. It runs in the app scope until the server is removed,
  follows edits to its addresses, and has no address once stopped.
- **Choosing: the best address proved so far, upgraded as higher ones answer.** Every address is asked at once with the service's
  `ServerProbe`, without credentials, and only one that reports the saved server id is used. When no address is in use, the first
  that proves itself is used at once and the check moves up to each higher one that answers, so requests never wait for a higher
  address that hangs until its probe times out (5 s for Jellyfin). An address already in use is only replaced mid-check by a higher
  one; a lower one takes over only when the check ends without it answering. The exception is an address in use that a request
  just found failing (it reported the address unreachable): the check that answers that report, the one running or the one it
  starts, counts it as unproved for choosing (requests still go to it until another address proves itself), the way a suspension
  does. So the first lower address that proves itself is used at once, one already proved in that check included, and the check
  still moves back up if the failing address answers after all. That check is always full; a report deferred by the request limit
  keeps its mark for the check that comes when the window ends; the mark carries over to the check an edit starts and to the full
  check after a partial miss; a network change, or an edit that removes the address, clears it. A full miss keeps the failing
  address in use and drops the mark: the next failure marks it again. Credentials still go only to proved addresses. The owner's
  browser run (docs/platform-notes.md, "Roaming in browsers, final") found a lower address answering within 0.13-0.45 s after a
  hotspot switch while the check waited about 5 s more for the failing one's probe to time out.
  The check ends when the highest proved address has no
  higher one still being asked, or once it has run for twice the probe's timeout (10 s for Jellyfin), with the best
  address it proved by then, since a probe can fail to answer at all. An address that answers with another id, a redirect or
  anything else is never used. In a browser, a local `http` address from an `https` page fails as mixed content and simply does
  not answer.
- **What the check proves:** the id comes from a public endpoint (`/System/Info/Public`), so it stops credentials going to *another*
  server that happens to answer at an address (a different network, a reused IP or name). It does not stop impersonation: anyone who can
  answer on a plain `http` local address can copy the id. HTTPS addresses are protected by TLS as usual.
- **Requests wait for the first proved address** at start and after a network change: a request made while that check runs waits
  until some address proves itself (at most the 1 s settle window plus twice the probe's timeout, 11 s for Jellyfin) instead of going
  to an unproved address, and fails as unreachable if nothing is proved. A network change holds requests from the moment it is
  reported, before the loop has handled it. On a new network the old address is dropped at once, since
  whatever answers there may be another server. For the other triggers the address in use stays until a check finds another.
- **A miss keeps the last proved address.** When nothing answers and the network has not changed, the address in use stays in use and
  the server is marked unreachable; the check is retried with the waits below. Only a network change, or an edit that removes it,
  clears the address.
- **One wait per request:** `ServerClient.request` asks the source once, sends every request in its block to that address, and reports
  that same address when one cannot connect or times out.
- **When it asks again:**
  - at start, and when the user adds, removes or reorders addresses;
  - on a network change (`NetworkChanges`): Android's default network callback; the browser's `online` and `navigator.connection`
    `change` events; on desktop, the network interfaces polled every 5 s (a change in which ones are up or in their addresses); on
    Apple, Network framework's path monitor (`NWPathMonitor`), when the path's status, interfaces (names and types) or gateways differ
    from the last update's; the first update is the path at start, and unchanged repeats do not count. A network change also starts the retry
    waits over at 15 s. The address is dropped and requests wait from the first update, but the check starts only once updates have
    stopped for 1 s, since one transition comes as a burst;
  - when the same network is usable again (`NetworkChange.Resumed`): on Android when the system unblocks the default network (it
    blocks a background app's network, and no check runs while it does), and on iOS and tvOS when the app comes back to the
    foreground. The check starts at once, the address in use stays, and the retry waits start over. With no address in use (a switch
    while blocked leaves none), requests wait for that check instead of failing at once, on every platform. A request that was waiting
    across it starts its wait again instead of failing, once per resumed network and never beyond twice the bound in all. The macOS
    wake is not a signal yet (a follow-up);
  - on Android when the system blocks the app's network (about 5 s after it goes to the background, and while it is frozen), and
    on iOS and tvOS when the app goes to the background (`NetworkChange.Suspended`): every check and timer stops and requests are
    held from that moment, since nothing runs while the app is suspended and the network may change meanwhile. The address in use is kept but no
    longer counts as proved: at the return (`Resumed`) a check starts at once, requests are released by the first address it proves,
    the kept one included, and the kept one is dropped if nothing answers. While suspended, a waiting request, or one made then, waits
    for the return without a time limit; time spent suspended does not count against the bound or the twice-the-bound limit,
    whether or not the process is actually frozen (Android blocks the network about 5 s after the app leaves, but freezes it only a
    minute or so later). A wait that ran out during a freeze, whose timeout fires at the return before the loop has seen it, waits for
    the return too. The owner's runs (docs/platform-notes.md, "Roaming on iOS, final" and the Android runs after it) found requests
    failing at the return or while suspended, and requests reaching the old address in the few ms before the path update;
  - when a request to the address in use cannot connect or times out (reports for an address no longer in use are ignored), and
    when a request finds no address at all. Never while a check runs, which answers it, and at most once per 15 s after the last
    check ended; a request that comes sooner is kept and the check comes when that window ends. So a caller that retries on every
    failure cannot drive checks back to back, and recovery takes at most 15 s plus a probe;
  - every 5 minutes, asking the address in use and those above it: to move back to a higher (local) address, or to notice that the
    one in use stopped answering, in which case every address is asked; every address is asked at once when a failure of the one in
    use was reported;
  - while nothing answers, after 15 s, doubling up to 5 minutes after each miss on the timer (not after one a request or a change
    woke), back to 15 s after a success.
  Edits and resumed networks that arrive during a check make exactly one more check; requests make none. A check whose addresses are edited, or whose network changes,
  while it runs is cancelled at once; a new one starts at once after an edit, and once the network has settled after a change.
- Whether the server answered goes into the capabilities state (`reachable`). A request that succeeds marks it reachable too, for the
  address in use only (it never proves an address), so a server is not shown offline while its requests succeed. After a missed check
  the two writers can alternate (the check writes false, a request true); the last write wins, and each is true when made. Stash, with
  no probe, is reachable after a request succeeds and unreachable once each address failed in turn since then or since the last
  network change; a saved server with no address source at all is unreachable.
- **While local network access is denied** (ADR 0020), checks skip local addresses. Denied means `LocalNetworkAccess.status()` is
  `CanAsk` or `Blocked`: Android 17 never shows the dialog by itself, so until the app asks and the user allows, local traffic is
  blocked as after a denial. PR 27's runs found that a denied request to a local address doesn't fail, it times out
  (docs/platform-notes.md). So:
  - Every local address (scope `Local` by host, ADR 0016) is left out of every check, except an overlay VPN address (Tailscale's
    `100.64.0.0/10`, `fd7a:115c:a1e0::/48` and `.ts.net`): the permission most likely does not cover VPN interfaces (not verified), so those are probed like
    internet ones and fall under the next rule if they do need it. A skipped address is never probed, so it never proves and never gets
    credentials. Skipping is preferred over ranking these addresses last: ranking last still waits for each one's timeout, and a grant
    brings them back within one probe anyway. A check left with no address ends at once. A server with only local addresses then has
    none, is marked unreachable, and requests fail as unreachable at once instead of after the 11 s bound.
  - An internet name can still resolve to a local address (split DNS at home), and that can't be known in advance. Such a name is
    probed. If its probe gets no answer for its whole timeout while access is denied, it is marked, unless it is in use or has proved
    itself since the last network change (a name that answered here does not resolve locally). A server that is down or busy and answers
    slower than the probe's timeout looks the same, so a mark only lowers how often the name is asked: checks leave marked names out
    only while another address is left to ask, and the periodic check (every 5 minutes), the check after a resumed network and every
    check while no address is in use (the retry waits bound how often) ask them again, so a server that comes back is used again, a
    higher one moved back to; while denied, that costs one probe timeout per periodic check. The mark clears on a network change, since
    another network may resolve the name publicly, on a grant, and when the address is removed. A refusal, or any failure that comes
    sooner, doesn't mark it.
  - `LocalNetworkAccess.changes` reports when the status may have changed: after each answer to the dialog, and each time the Android
    activity resumes, since a grant in the settings comes no other way. Only a flip between allowed and denied counts. It starts a full
    check at once, which replaces a running one; during a settle or while suspended, the check that follows uses the new status. A flip
    to denied drops a local address in use. With no address left in use, requests wait for that check, as after a resumed network. Each
    check also reads the status first, so a change no signal reported is seen by the next one, with the same effects.
  - A service that can't identify itself (Stash) takes turns only among its addresses that don't need local access while access is
    denied, unless every address needs it.
  - Where the platform doesn't ask (`NotNeeded`: desktop, web, Apple for now, Android before 17), nothing changes.
- **Connection failures reported outside `IOException`** are request failures (`Unreachable`), not bugs, so they cannot end the roaming
  loop: Android's `SecurityException` for a missing `INTERNET` permission (logged once, so a shell without the permission does not pass
  for a server that is down), and Ktor's browser `Error("Fail to fetch")` for any failed fetch. Only those exact shapes count; any other
  `SecurityException` or `Error` stays a bug.
- **A source whose run a bug ends is replaced**, a stray `CancellationException` included. Clients hold a stable wrapper; behind it
  a fresh source, with its own queue, ids and state, starts cold (holding requests and checking) from the saved server as it is then,
  so the dead run's queued events and probe results are never handled. It starts after 15 s, doubling while the bug recurs up to
  5 minutes, back to 15 s after a run of 5 minutes. A dying run drops its address at once, and `reachable` goes back to unknown
  until the fresh source has checked. The bug reaches the app scope's exception handler, which logs its class name only (ADR 0015),
  so it does not end the app on Android and Apple; a stray cancellation, which that handler never sees, is logged the same way.
- **A network signal that fails starts again** after 15 s, doubling up to 5 minutes, and counts as a change, since one may have been
  missed meanwhile. A source whose signal ends is treated as a bug, never left running blind.
- **A service that cannot identify itself without credentials** (Stash, later; an explicit list) gets its addresses in priority order,
  moves to the next on a reported failure and back to the first on a network change. Every saved address is one the user typed or
  confirmed (ADR 0016). Any other service without a probe gets no address.
- **Editing addresses:** `withAddress`, `withoutAddress` (never the last) and `withAddressMoved` on `SavedServer` (`core:network`);
  adding still goes through `ServerSetupViewModel(addTo = …)`, which accepts only the same server id. `ServerAddressesViewModel`
  (`feature:settings`) lists the addresses with their scope and the one in use, and removes and reorders them.
- **The address screen's status** (`ServerAddressesViewModel`): the server's status (checking, reachable, offline), the address in use
  with its network kind, and Refresh. Status comes from `reachable` in the capabilities state. It reads "checking" only while
  `reachable` is unknown or a check runs with no address in use, so the periodic check never makes it flicker. While offline, no
  address is shown in use, though the source keeps the last proved one. A source publishes whether a check runs or a network settles
  (`ServerAddressSource.checking`). Refresh (`refresh()`) stays enabled during a check, so it keeps keyboard focus, and is ignored
  while one runs:
  - a full check, a pending settle or a suspension already answers it;
  - a periodic (partial) check is replaced by a full one;
  - it asks marked names too and starts the retry waits over, keeping the address in use;
  - Stash goes back to its first address.
- **Switch notice:** `ServerAddressSource.switches` reports each address roaming moved to from another one it used before. Not a switch:
  the first address after start, the addresses the first check moves through, or an address an edit of the addresses moved to.
  `RoamingNoticesViewModel` (`feature:settings`) turns them into a brief notice. It names the server and the network kind, never the
  address, and clears itself after 4 s. The Compose shell shows it over the page; the web can render the same state later.

## Consequences
- Android declares `INTERNET` and `ACCESS_NETWORK_STATE`; without the latter there is no signal. Reaching a local server on Android
  still needs a network security config for plain `http` local addresses, and `ACCESS_LOCAL_NETWORK` at target 37
  (docs/platform-notes.md, "Roaming on Android"); both came with ADR 0020.
- On Android a DNS lookup runs outside every engine timeout: over a VPN the owner measured 6.0 s for a failed lookup of the internet
  name (docs/platform-notes.md, "Request timeouts on blocking engines"). `RequestDeadline` (`core:network`) bounds every request,
  probes included, so a probe ends at its 5 s timeout while the abandoned lookup finishes on its own. A failed lookup is cached for
  only 1-3 s on that phone, so a later check usually waits the full 5 s again rather than failing fast.
- A network change does not cancel requests already sent: one sent just before it can go to the old address and hang until its own
  timeout (17.8 s over a VPN on Android).
- Every probe of every address goes into the request log under the server's key, so roaming adds log lines but no hosts.
- The first request after start or a network change can wait up to the settle window plus twice the probe timeout (11 s for
  Jellyfin), longer while a burst of updates goes on. On a Mac (docs/platform-notes.md, "Apple path monitor and roaming on a Mac") a
  transition came as 7-11 path updates within 3-5 s, and opening the Wi-Fi menu alone sent some. Before the settle window each
  update restarted the check: mostly a few milliseconds, but on a phone hotspot, where the home address timed out instead of being
  refused, requests were held 4-7.5 s per update. With the window, a burst costs one check after it ends; an update with nothing
  changed still costs one check on platforms that report them; Apple's now compare the path first.
- A network change no signal reports leaves requests on the old address until one fails or times out, or until the next periodic
  check (up to 5 minutes): if another device answers there on the new network, it receives them, credentials included. Known cases: on
  desktop, a move to another network that keeps the same interface addresses (same subnet and lease); on Apple, whatever the path
  monitor does not report, such as a move between two Wi-Fi networks whose gateways have the same address, made without a moment in
  which the path is unsatisfied (the path's status, interfaces and gateways then look the same); on the web, most real switches:
  the owner's run in Chrome, Edge and Firefox fired no `online` or `offline`
  for switches between networks, and `navigator.connection` `change` follows the speed estimate, not the network
  (docs/platform-notes.md, "Roaming in browsers"). There the periodic check and failed requests, now mapped to `Unreachable`, are what
  notices a new network.
- Desktop polling reads the interfaces every 5 s while the app runs. Link-local addresses are ignored, and an interface left with no
  other address does not count, so Docker's `veth*` pairs and idle Hyper-V or WSL adapters do not trigger a check. Each of these still
  does, and makes requests wait for one check: a virtual adapter with a routable address appearing, disappearing or changing (Hyper-V's
  `vEthernet (Default Switch)` or WSL's adapter getting a new private address when the host or WSL restarts, Docker's `docker0` and
  `br-*` bridges, a VPN's tunnel), and an IPv6 temporary address that rotates.
- A server answering at several addresses is asked at all of them on each full check; with a handful of addresses per server that is cheap.

## Addendum 2026-10-21: the address screen
The Addresses page lists the addresses in priority order with their scope and the one in use, adds one through
`ServerSetupViewModel`, removes one, and reorders with Move up and Move down. The candidates, and what became of them:
- **Built:** a "Currently using" block at the top: the server's status, the address in use, its network kind (local or internet), and
  a Refresh button that starts a full check (Decision, "The address screen's status").
- **Built:** a brief notice when roaming switches to another address (Decision, "Switch notice"), in the Compose shell; the web can
  render the same state later.
- **Left out:** reordering internet addresses by dragging. Compose has no reorderable list, and a hand-written long-press drag with
  auto-scroll for touch and mouse is not small. Move up and Move down stay, and `ServerAddressesViewModel.move(from, to)` is there for
  a later drag.
- **Still candidates:** local addresses added by hand or found by discovery (the spike in docs/platform-notes.md, "Local server
  discovery, run on devices"), which stops by itself after a short time.
- **Still candidates:** custom HTTP headers per address, for reverse proxies. They touch credentials and redaction (ADR 0015), so
  header values are secrets: never logged, never copied into diagnostics, and stored like other secrets (ADR 0017). Needs an owner
  decision on how they are stored.
