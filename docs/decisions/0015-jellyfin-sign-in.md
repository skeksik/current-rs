# 0015: Jellyfin sign-in, sessions and saved servers

Status: proposed (2026-10-22), from M1; accepted when its PR merges. The device name rule and the tokio features were decided by the
owner (2026-10-22, marked **Decided by the owner**). Amended 2026-10-23: removing a saved server (owner's request; manager's answers
on the design), below under "Remove a server" and in "Saved servers". Amended 2026-10-24: the 401 rule and Jellyfin's header
placement behind an authentication gateway (ADR 0017, approved by the owner), below under "The 401 rule" and "Header placement".
Amended 2026-10-25: the token check after a 401 that the identity probe confirms as Jellyfin's (approved by the owner 2026-10-25),
below under "The 401 rule".

## Context
After setup finds a server (ADR 0012), the user signs in with a password or Quick Connect. The app must remember the server and user,
keep the token only in the OS keyring (ADR 0013), restore the session at the next start, notice when the server ends it, and sign out.
Facts below were read on 2026-10-22 from the sources listed at the end; "Jellyfin" means the server at tag `v12.2`.

## What the server does
- **The authorization header.** Jellyfin reads `Authorization: MediaBrowser Client="…", Device="…", DeviceId="…", Version="…",
  Token="…"` (`AuthorizationContext.cs`; scheme name case-insensitive). `GetParts` splits on commas outside quotes, trims the quotes and
  **URL-decodes every value** (`WebUtility.UrlDecode`). Jellyfin's Kotlin SDK percent-encodes all but unreserved characters (space as
  `+`); its TypeScript SDK uses `encodeURIComponent`. A sign-in without Client, Device, DeviceId or Version fails with 400
  (`ArgumentException.ThrowIfNullOrEmpty` in `SessionManager.AuthenticateNewSessionInternal`). On every authenticated request the
  server copies a changed Device or Version from the header into the token's device record.
- **What the server shows.** Each token is a device record (app name, app version, device name, device id). The dashboard's device
  list (`DeviceInfoDto`: `Name`, `AppName`, `AppVersion`, `LastUserName`) and its active sessions show them; an administrator can set a
  `CustomName` per device.
- **Password sign-in:** `POST /Users/AuthenticateByName` with `{"Username", "Pw"}` returns `AuthenticationResult { User (UserDto),
  SessionInfo, AccessToken, ServerId }`. Errors (`UserManager.AuthenticateUser`, `ExceptionMiddleware`):
  - unknown user or wrong password: `AuthenticationException`, **401**;
  - disabled user, remote access turned off for the user and the request is not local, a parental time limit, a device the user may
    not use, the session limit: `SecurityException`, **403**. Outside a development environment the body is always "Error processing
    request.", so the 403 causes cannot be told apart;
  - no rate limiting. **Lockout** disables the user after `LoginAttemptsBeforeLockout` failures (policy default -1, none; 0 means 3),
    so the next attempt answers 403.
- **Quick Connect** (`QuickConnectController`, `QuickConnectManager`):
  - `GET /QuickConnect/Enabled` answers a bool;
  - `POST /QuickConnect/Initiate` (needs the four header values) answers `QuickConnectResult { Authenticated, Secret (64 hex digits),
    Code (6 digits), DeviceId, DeviceName, AppName, AppVersion, DateAdded }`, or 401 when Quick Connect is off;
  - `GET /QuickConnect/Connect?secret=` answers the same, 404 for a request it does not know (expired), 401 when off;
  - another signed-in client approves the code (`POST /QuickConnect/Authorize`); the server then creates the token at once through
    `AuthenticateDirect`, which also logs out older tokens of the same device and user;
  - `POST /Users/AuthenticateWithQuickConnect {"Secret"}` returns the `AuthenticationResult`, 404 if not approved or expired;
  - a request lives **10 minutes** (`Timeout = 10`); an approved secret stays collectable for 10 minutes after approval;
  - jellyfin-web polls `Connect` **every 5 s** (`setInterval(…, 5000)` in its login controller); neither SDK sets an interval.
- **The token later:** an unknown token makes `AuthService` throw `SecurityException("Invalid token.")`, which
  `CustomAuthenticationHandler` turns into a failed authentication: **401**. A disabled user with a valid token also gets 401. A valid
  token that `DefaultAuthorizationHandler` refuses (remote access off from outside, a parental time limit) gets **403**.
- **Current user:** `GET /Users/Me` answers the `UserDto`; 401 as above, 400 if the token has no user.
- **Sign out:** `POST /Sessions/Logout` answers 204 and deletes the token's device record (`SessionManager.Logout`). The server writes
  the token to its own log at Info level while doing so ("Logging out access token {0}"); nothing on our side can change that.
- **DeviceId:** a new sign-in logs out every token for the same device id and user, then creates one (`GetAuthorizationToken`,
  as ADR 0013 found). Two users on one server from one install get one token each.

## Decision

### `current-jellyfin`
- `ClientIdentity` (client `Current`, the core's version, the device name, the device id) and `authorization_header`, which
  percent-encodes every value except RFC 3986 unreserved characters (so a comma, quote or non-ASCII name cannot break the header;
  tested against a port of `GetParts`). `JellyfinAuth` (the `RequestAuth` hook, ADR 0006) adds the header to every request, with the
  token once one is set; the header value is marked sensitive. It runs after the request log entry is built, so the token never
  reaches the log.
- `AccessToken` and `QuickConnectSecret`: redacted `Debug`, no `Display`. The Quick Connect secret travels as a query value, which
  the request log redacts (ADR 0006), and in a JSON body.
- `JellyfinClient` (one `ServerClient`, a token slot shared with the hook): `sign_in_by_name`, `quick_connect_enabled`,
  `quick_connect_initiate`, `quick_connect_approved`, `quick_connect_sign_in`, `quick_connect_wait` (polls every interval until
  approved, 404 or the lifetime ends; a poll that fails for a transient reason, no connection, a timeout or a 5xx while the server
  restarts, is retried at the next interval; dropping the future stops it), `current_user`, `sign_out`, `send_authenticated`.
- `ClientIdentity`'s `Debug` leaves out the device name.
- The password is borrowed into the request body and nowhere else: no state, error or log holds it (tested).
- `AuthError`, by what the UI does: `WrongCredentials` (401 at sign-in), `Refused` (403), `QuickConnectDisabled`,
  `QuickConnectExpired`, `SessionRevoked` (401 with a token), `NotSignedIn`, `Network(NetError)`; amended 2026-10-24 (ADR 0017):
  `Gateway(GatewayAnswer)` (a gateway's challenge) and `EmbyHeaderIgnored` (the 400 diagnosis).
- **Users:** `UserDto` maps to the model's `User`. Policy flags that are `true` become permissions: `IsAdministrator` → `admin`,
  `EnableContentDownloading` → `download`, `EnableContentDeletion` → `delete`; every other true flag passes through under Jellyfin's
  name (open values, ADR 0005), except `IsDisabled` and `IsHidden`, which are states of the account. The policy is read as a JSON
  map so that flags a newer server adds pass through. The avatar is left out for now: its URL holds the server's address, which the
  capabilities state must not (ADR 0014).

### The 401 rule
A 401 to any request that carried the token ends the session: the client forgets the token, then `current-app` deletes it from the
keyring, clears the server's active user in the settings and shows the server signed out. A 403 never does.

Why: Jellyfin answers 401 to a token only when the token no longer exists (signed out elsewhere, replaced by a newer sign-in from this
device id, removed by an administrator) or the user is disabled. In every case it can never work again: keeping it would retry a dead
token at every start and show "signed in" when the user is not. A 403 means the token is valid and this one request was refused (an
admin-only endpoint, remote access, a time limit), so the session stays. A client forgets only the token that failed, so a newer
sign-in in between is kept. In `current-app` each live session has an id and its user; the hook acts only if its session is still
the server's current one, and then clears the active user and deletes the token only if they are still that session's user. A late
401 on an old client (the server logged its token out when the same user signed in again, or another user signed in since) changes
nothing (tested).

**Known risk:** a reverse proxy with its own authentication (HTTP Basic, say) in front of Jellyfin also answers 401, and would wipe
the token. Jellyfin's own 401 carries no `WWW-Authenticate: Basic` (ASP.NET Core's default challenge; not verified against a live
server), so a later change could ignore 401s that ask for Basic. That needs `current-net`'s `NetError::Auth` to carry that fact; not
built now (manager, 2026-10-22).

**Amended 2026-10-24 (ADR 0017):** a 401 ends the Jellyfin session only if it is Jellyfin's.
1. A 401 with any `WWW-Authenticate` challenge is a gateway's: never a session end. With a Basic challenge the password prompt
   reappears. `send_authenticated` returns `AuthError::Gateway` for a 401 or 403 with a challenge, keeps the token and reports
   `SessionEvent::Gateway` to the session hook (`JellyfinClient::on_event`, which replaced the revoked hook); a sign-in's 401 with
   a challenge is `AuthError::Gateway`, not `WrongCredentials`.
2. Any other 401 (and any redirect or HTML page where JSON was expected) on a server: the core re-runs the identity probe with the
   address's gateway credentials before acting (`JellyfinIdentity::probe_client`). If the probe answers Jellyfin's JSON with the
   right identity, the 401 was Jellyfin's: the rule above applies. If the probe gets a gateway shape, the server shows "Sign in to
   the gateway again" and the session and token are kept. If the probe fails to connect, nothing changes (roaming handles it).
   Built in `current-app`: the client keeps the token, reports `SessionEvent::Unconfirmed` to the session hook once, and answers
   `AuthError::Unconfirmed` without sending until the app calls `revoke` (Jellyfin's 401) or `resume`.
This also covers gateways that answer 401 without a challenge.

**Amended 2026-10-25 (approved by the owner 2026-10-25, "check the token"):** a 401 that the identity probe confirms as Jellyfin's
no longer ends the session by itself: the core first asks Jellyfin who the token belongs to.

Why: the probe proves only that the 401 came from Jellyfin, not that the token is invalid. On the owner's server (2026-10-25),
clicking the navigation's entry of a library removed on the server signed the user out. Jellyfin 12.2 answers 401 with a valid token
for one resource (sources below, read 2026-10-25):
- `GET Items` with a `parentId` the server still knows but the user cannot see: `ItemsController.GetItems` returns
  `Unauthorized($"{user.Username} is not permitted to access Library {item.Name}.")` when `!item.IsVisible(user)` (lines 342-350).
  `IsVisible` is false for a library not among the user's enabled folders or among the blocked ones (`Folder.IsVisible`, lines
  252-275), a disabled library (`CollectionFolder.IsVisible`, lines 117-125), and an item the user's parental rating or tags hide
  (`BaseItem.IsParentalAllowed`). A `parentId` the server does not know answers **400**, not 401: `LibraryManager.GetParentItem`
  throws `ArgumentException("Invalid parent id: ...")` (lines 3276-3289), which `ExceptionMiddleware` maps to 400 (line 127). Which
  of the two a removed library gets depends on whether the server still holds its folder item, which was not checked on a live server
  (**not verified**).
- `DELETE Items/{itemId}` and `DELETE Items` when the user may not delete the item (`LibraryController`, lines 388, 421, 434), and
  `GET Items/{itemId}/Collections` without a user (line 753). Current sends none of these.
- Quick Connect turned off (`QuickConnectController`, lines 66, 92, 118): sign-in only, already handled.
- Not 401: a single item (`GET Items/{itemId}`, `UserLibraryController.GetItem`), `GET Items/Latest`, images (`ImageController`) and
  streams answer 404 for an item that is gone or hidden from the user (`GetItemById(id, user)` returns null, then `NotFound()`); a
  missing media source or live stream is `ResourceNotFoundException`, 404 (`MediaInfoHelper`, `StreamingHelpers`). Searched: every
  controller in `Jellyfin.Api/Controllers` for `return Unauthorized` and `Status401Unauthorized`.

The rule's second part, after the probe answered Jellyfin's JSON with the right identity:
- The client asks `GET /Users/Me` with the token (`JellyfinClient::check_token`), while it still holds every other request back.
- **The token works** (Jellyfin's `UserDto`, or a 403 without a page, which Jellyfin sends only to a valid token): the 401 was about
  that one resource. The session and token stay, held requests go out again, and the request that got the 401 fails with
  `AuthError::NotAvailable` (`JellyfinClient::keep`). Browse shows it as `BrowseProblem::NotAvailable`, playback as
  `PlaybackProblem::NotFound`.
- **The token fails** (401 without a challenge): the session ends as before (`revoke`, then the app signs out).
- **No answer** (a network error, a 5xx, a gateway's answer, a page, or a newer token in the meantime): nothing changes (`resume`).
- The request that got the 401 now waits for the decision (at most 60 s) and answers `SessionRevoked`, `NotAvailable` or
  `Unconfirmed`. Requests made while the check runs still answer `Unconfirmed` without sending, so at most one probe and one token
  check run per session.
- A library grid whose request fails with `NotAvailable`, or with 400 or 404 (a server that has forgotten the parent), reads the
  user's libraries again; if the library left them, the grid's state says `NotAvailable` (ADR 0018, amended 2026-10-25).
- Tests: `crates/jellyfin/tests/sign_in.rs` (the client's three outcomes, the token check's answers), `crates/app/tests/
  library_changes.rs` against the stand-in's `remove_library` (401 to a valid token) and `forget_library` (400): the session survives
  and the grid shows `NotAvailable`; a revoked token still signs out; a gateway's answer still asks for the gateway sign-in; one probe
  while the token is checked, other requests held back.

Sources (read 2026-10-25): Jellyfin server `v12.2` (ca0f16eb), https://github.com/jellyfin/jellyfin:
`Jellyfin.Api/Controllers/{ItemsController, LibraryController, UserLibraryController, ImageController, QuickConnectController,
VideosController, MediaInfoController, DynamicHlsController, TvShowsController, UserViewsController, UserController}.cs` and the rest
of `Jellyfin.Api/Controllers` (searched), `Jellyfin.Api/Helpers/{MediaInfoHelper, StreamingHelpers}.cs`,
`Jellyfin.Api/Middleware/ExceptionMiddleware.cs`, `Jellyfin.Api/Auth/CustomAuthenticationHandler.cs`,
`Emby.Server.Implementations/Library/LibraryManager.cs`, `MediaBrowser.Controller/Entities/{BaseItem, Folder, CollectionFolder}.cs`,
`MediaBrowser.Model/Configuration/LibraryOptions.cs`.

### Header placement (amended 2026-10-24, ADR 0017)
A per-server setting, `HeaderPlacement`: `Authorization` (default) or `X-Emby-Authorization`, which Jellyfin reads only with
`EnableLegacyAuthorization` on and `Authorization` empty (`docs/notes/gateway-auth.md`). The app keeps it with the server and sets
it on the client (`JellyfinClient::set_header_placement`). It changes to `X-Emby-Authorization` automatically when the address has
Basic gateway credentials (`HeaderPlacement::for_gateway`), and when a request carrying `Authorization: MediaBrowser` gets a gateway
answer with a challenge while the probe (without that header, through the same address) passes, as some portal gateways' default
endpoints do (`HeaderPlacement::after_gateway_challenge`). In the second case the request provably did not reach Jellyfin, so a
sign-in (and Quick Connect's `Initiate`, which needs the same four values) is retried once with the other header; the app reads
`header_placement()` afterwards and saves it if it changed. A sign-in that then answers 400 means the server ignores
`X-Emby-Authorization`: `AuthError::EmbyHeaderIgnored` explains the two server-side fixes (legacy authorization on, and the gateway
not passing its `Authorization` on; or a gateway rule that lets Jellyfin's API through), and a placement changed by the retry goes
back.

### Saved servers (`current-model`, `current-store`)
- `SavedServer { server: ServerRef, id: ServerId, name, addresses: Vec<ServerAddress>, users: Vec<SavedUser { id, name }>,
  active_user: Option<UserId> }`, in `settings.json` as a defaulted `servers` field: format 1, no migration (ADR 0013). The file keeps
  ids, names and addresses, never a token. `server_id` is defaulted, since a service without one (Stash, ADR 0014) may store none.
  Unknown fields inside a server record are not kept on save (only top-level ones are); a
  newer version that adds one must raise the format if an older one dropping it matters.
- The token lives only in the keyring under `SecretKey { server, user }` (ADR 0013).
- A sign-in to a server whose id is already saved reuses its key, adds the address if new and adds or updates the user. Otherwise
  the server gets the next numeric key: one more than the highest ever given out, kept as `last_server_key` in `settings.json`
  (added 2026-10-23, defaulted, no migration; ADR 0013). A removed server's key is never given to another server, so a keyring
  entry that outlived its server can never be read for a different one. An older file without the field starts from the highest
  key in its list. A downgrade keeps the field (an older build writes unknown top-level fields back, ADR 0013) but ignores it
  when it picks a key, so an older build may give a removed server's key again; accepted, since those builds cannot remove a
  server and the removed server's keyring entries were deleted, or reported as left. Signing in as another user makes that user
  active; the earlier user's token stays in the keyring for user switching later (not built).

### `current-app`
- **`SignIn`** flow, from `App::sign_in(NewServer)` (setup's `Ready`) or `App::sign_in_again(&ServerRef)`: `Ready { quick_connect }`,
  `SigningIn`, `QuickConnectWaiting { code }`, `Failed { error }`, `SignedIn { server, user, remembered }`. No state holds a password,
  token or secret (tested). A new attempt or `cancel()` supersedes a running one through a generation counter in a `watch` channel;
  the running future returns at once (`tokio::select!`), so Quick Connect stops polling; dropping the future does the same. Only the
  current attempt writes the state. A sign-in the server already completed is saved in full (settings, keyring, session, status) on
  a spawned task, even if the attempt is cancelled or its future dropped during the save. `remembered` is false without a keyring:
  the session lasts for the run (ADR 0013).
- **Before anything is sent:** an attempt fails at once with `Save(NewerVersion)` when the settings belong to a newer version (the
  sign-in could not be saved, and it would log out this device's earlier token on the server). For a saved server, each attempt
  first waits (in `SigningIn`, up to 10 s, cancellable) until the server's address provider has selected an address that passed the
  identity check (ADR 0014). It fails with `Probe(DifferentServer)` when none did but one answered as another server, else with
  `Auth(Network(Unreachable))`. No password and no Quick Connect request can reach an unchecked address even without that wait: the
  attempt's client goes through the same provider, which serves nothing before a passed check (see "Address"). Setup has just
  probed a new server's one address. A sign-in whose `ServerId` differs from the expected server's is not saved
  (`Probe(DifferentServer)`).
- **Restore:** `App::open` loads the saved servers into the capabilities state, signed in where a user is active, reachability
  unknown. The shell then runs `App::restore_sessions()` in the background. It gives every saved Jellyfin server its address
  provider and checks every server at once (`JoinSet`): the token from the keyring (none: signed out and the active user cleared;
  keyring unavailable: signed out for this run, the user kept); then the session is installed over the provider with the token;
  then `GET /Users/Me`. There is no separate identity probe: `/Users/Me` is the first request with the token, and it waits for the
  provider's race, whose identity check must pass first. Results: signed in with user and permissions; signed out on 401 (the 401
  rule); still signed in and unreachable when no address answers as the server (no token is sent then). Such a session stays
  installed, and its user is checked once its provider reaches the server. Running restore again checks sessions whose user is not
  yet confirmed. Servers of other services are skipped. (Changed in wave 3: before, an unreachable server got no session.)
- **Sign out:** `App::sign_out` first signs out on this device, under the session lock: the session, the active user and the stored
  token are gone, and the status shows it, before anything is sent. It then calls `POST /Sessions/Logout` (10 s timeout) with the
  token the old client still holds in memory, and returns the server's answer for the UI to mention. The user stays saved. A restore
  while the server answers finds nothing to restore, and a sign-in that completes meanwhile is a new session and is kept (both
  tested).
- **Remove a server** (amended 2026-10-23): `App::remove_server(&ServerRef)` returns `Removal::{NotSaved, Removed {
  credentials_left }}`. Local first, as sign-out:
  - Under the session lock, kept short (no keyring I/O under it): the list without the server is saved (the atomic save; the
    observable list and the settings file change together), then its session, its address provider (the follow task stops) and
    its status in the capabilities state are dropped.
  - After the lock: each saved user's token for the server (all of `users`, not only the active one) is read from the keyring
    and deleted. Reading after the commit is safe: from then on nothing writes a token under that key (a sign-in to it finds no
    saved server, `ServerRemoved`; one saved earlier has already stored its token, which is read and deleted here; the key is
    never given out again).
  - Then, in the background, best effort and never awaited, the server is told: `POST /Sessions/Logout` (10 s timeout) with the
    live session's client and with each other token read, all through the server's `RoamingAddresses`, so only to an address that
    passed the identity check (a server not reached in this run gets a provider over its last saved addresses for this). Its
    answer changes nothing. A sign-in whose save finds the server removed logs its new token out the same way, so no device
    record lingers on the server.
  - Removing a server that is not saved is a no-op (`NotSaved`), not an error. Settings from a newer version (or a failed write)
    fail the removal before anything changes: the server stays saved and signed in.
  - **A keyring that fails** does not stop the removal: the settings entry goes, and `credentials_left: true` lets the UI say
    that a saved sign-in may still be in Credential Manager or the keychain. That is a delete that fails (`Failed`, `TooLong`),
    or a read or delete answering `Unavailable` on a platform with a keyring backend, where it means locked or refused and an
    entry stored while it worked may still be there (manager, 2026-10-23). Only without a backend (`Secrets::has_backend`
    false: Linux until its app task) does `Unavailable` mean nothing was ever stored. The key is never reused, so a leftover
    entry is dead weight, not a risk; a token that was read is also logged out on the server.
  - **Races:** only a saved server has a status: `update_status` writes nothing for a server that is not in the saved list, checked
    under the capabilities state's lock, which the removal takes after its commit. So a restore that ends after the removal
    (its `signed_out`, `Unreachable` or user check) cannot bring the status back, and its install is refused because the server
    has no active user. A provider being built for it is dropped (the existing "saved list changed" check). The 401 hook and a
    user check on the old client find no session. Tested: a removal with two users signed in (both tokens deleted and logged
    out), unknown and repeated removals, a removal that does not wait for the server, a restore waiting on `/Users/Me` while
    the server is removed, a sign-in again racing the removal and one saved just before it, a keyring that cannot delete, and
    settings from a newer version.
- **Ordering:** saving a sign-in, signing out, removing a server, the 401 rule and restoring each change a server's session under
  one async lock and check under it that the session or active user they act on is still current. Restore installs its client
  only if no sign-in or sign-out happened during its probe, and writes the status only while its session is current.
- **Address** (wave 3, ADR 0014): each saved Jellyfin server has one `RoamingAddresses` over all its addresses with
  `JellyfinIdentity` (its `ServerId`). It is kept in the shared state with the addresses it was built from, and it hears network
  facts (`App::network_changed`, `App::returned_to_foreground`). Its session and its sign-ins use it. It is made when first needed
  (restore, a sign-in, a sign-in-again attempt), because it needs the tokio runtime and `App::open` runs off it. It is made again
  when the saved addresses or id change, for example a sign-in through setup that adds an address. A new server's sign-in itself
  uses `FixedAddress` on the address setup just probed; the session saved from it uses the provider.
  - **Credentials only to checked addresses:** the provider serves an address only once it passed the identity check in the race
    that selected it, and during a race after a failure or a network change it serves none (ADR 0014, "Implementation"). Every
    request with a token or a password goes through it and waits for that check. Tested: an address answering with another
    server's id never gets the token, at restore or after a network change mid-session. No explicit probe is left in restore or
    sign-in-again. **Known window:** a request that took its address just before the platform reported a network change still
    goes to that address, with the token, after the device has moved to the new network. The identity check cannot close it, and
    it is no protection against an attacker anyway (it catches mistakes; only TLS proves who answers, ADR 0014).
  - The provider is always built from the saved list as it is at that moment, never from a caller's older copy. It is built on
    the blocking pool, because each address sets up TLS. A replaced provider's status no longer reaches the capabilities state:
    each provider has a generation, checked under the lock that guards the replacement.
  - A user check that does not finish (a restore dropped mid-request, say) goes back to "unchecked", so the next time the server
    is reached, or the next restore, checks again.
  - **The capabilities state follows the provider:** a task per provider copies its status into the server's `ServerStatus`:
    reachability and the position of the address in use (`address`), never the address itself. A provider that has not tried
    yet leaves the status as it is, so a sign-in that just reached the server shows it reachable. The version comes from the
    identity check's answer (`JellyfinIdentity::version()`, the last passed check's `Version`), and from setup for a new server.
  - `App::jellyfin(server)` returns a restored session's client also while the server is unreachable; its requests fail fast
    until the provider reaches the server.
  - `App::saved_servers()` publishes the saved list on every change, for the bridge.
- **Blocking work:** the settings file and the keyring are blocking (ADR 0013). `current-app`'s async methods run them on tokio's
  blocking pool (`spawn_blocking`); `App::open` stays a sync call the shell makes off the runtime, as before. The saved-server list
  and the settings file change together (`Shared::commit`, serialized by its own lock, which writes first and keeps the old list if
  the write fails). The list's lock is held only to read or swap it, never during the write, so async code may read it.

### Decided by the owner: the device name
"Use the same names that Jellyfin uses" for the device, and `Current` with the core's version for the app (2026-10-22). What
Jellyfin's own clients send as `Device`:

| Client | Device name | Source |
|---|---|---|
| jellyfin-web (browser) | The browser's name (`Chrome`, `Firefox`, …, else `Web Browser`), plus ` iPad`, ` iPhone` or ` Android` | `src/components/apphost.js` `getDeviceName` |
| Jellyfin Desktop (Windows, Linux) | The host name (`QHostInfo::localHostName()`) | `src/utils/Utils.cpp` `ComputerName`, `SettingsComponent::getClientName` |
| Jellyfin Desktop (macOS) | The computer's name as the user set it (`[[NSHost currentHost] localizedName]`) | `src/utils/osx/OSXUtils.mm` |
| Jellyfin Android, Android TV | The device name from the system settings (`Settings.Global.DEVICE_NAME`), else manufacturer and model | Kotlin SDK `jellyfin-core/src/androidMain/.../AndroidDevice.kt`, used by both apps |
| Swiftfin (iOS, tvOS) | `UIDevice.current.name`, diacritics folded, characters outside the URL query set removed | `Shared/Extensions/JellyfinAPI/JellyfinClient.swift` |

So Current sends the platform's name for the computer, which only the UI can read: `App::open` takes `device_name`, the shells pass

| Platform | `device_name` |
|---|---|
| Windows | The host name (what `QHostInfo::localHostName()` returns there; the Windows app task picks the API) |
| macOS | `Host.current().localizedName`, Foundation's `NSHost.localizedName` in Swift (the computer's name the user set; which settings screen shows it not verified) |
| Linux | The host name |
| Android, Android TV | `Settings.Global.DEVICE_NAME`, else manufacturer and model |
| iOS, tvOS | `UIDevice.current.name` |

The core trims it, turns control characters into spaces, cuts it at 64 characters and percent-encodes it in the header; an empty
name becomes the platform's name (`Windows`, `macOS`, `Linux`, …), since Jellyfin refuses an empty one. The name shows in the
server's dashboard to its administrators, as every Jellyfin client's does; it never goes into our logs, errors or `App`'s Debug.

### Decided by the owner: tokio features
`time` in `current-jellyfin` (Quick Connect's wait); `rt` (`spawn_blocking`, `JoinSet`), `time` and `macros` (`select!`) in
`current-app` (2026-10-22). No new crates; `current-jellyfin` also uses `serde_json`, already a workspace dependency.

## Consequences
- A second sign-in from the same install as the same user logs out the first token on the server: one session per user and device.
- Wrong password and refused user are told apart; the reasons for a refusal are not, so the UI says "the server refused this user"
  and suggests asking the administrator.
- After a 401 the user signs in again; a disabled user who is enabled again must sign in again too.
- The 403 cases keep the session, so a user outside a parental time limit stays signed in and sees requests refused.
- Tests run against hand-written stand-ins only: password ok, wrong and refused; Quick Connect approved, off, expired, cancelled,
  dropped and riding out a 503; restore ok, revoked, unreachable and another server at the address; a 401 and a 403 mid-session; a
  late 401 on an old client of the same or another user; signing in again to an address that answers as another server; a sign-in
  reporting another server's id; settings from a newer version; sign out with the server up and gone, racing a new sign-in and racing a restore; no
  keyring; settings round trip. Roaming (wave 3): a two-address server restoring on address 2 when address 1 is down; an address
  with another server's id never getting the token, at restore and after a network change mid-session; a network change racing
  again with the session going on; the status following reachability and the user checked once the server is reached;
  signing in again waiting in `SigningIn` and cancelled. None ran against a real Jellyfin server.

## Sources (read 2026-10-22)
- Jellyfin server, tag `v12.2` (commit ca0f16eb, 2026-10-05), https://github.com/jellyfin/jellyfin:
  `Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`, `Jellyfin.Api/Controllers/UserController.cs`,
  `Jellyfin.Api/Controllers/QuickConnectController.cs`, `Jellyfin.Api/Controllers/SessionController.cs`,
  `Emby.Server.Implementations/QuickConnect/QuickConnectManager.cs`, `Emby.Server.Implementations/Session/SessionManager.cs`,
  `Jellyfin.Server.Implementations/Users/UserManager.cs`, `Jellyfin.Api/Middleware/ExceptionMiddleware.cs`,
  `Jellyfin.Api/Auth/CustomAuthenticationHandler.cs`, `Emby.Server.Implementations/HttpServer/Security/AuthService.cs`,
  `Jellyfin.Api/Auth/DefaultAuthorizationPolicy/DefaultAuthorizationHandler.cs`, `MediaBrowser.Model/Dto/UserDto.cs`,
  `MediaBrowser.Model/Users/UserPolicy.cs`, `MediaBrowser.Model/Dto/DeviceInfoDto.cs`, `MediaBrowser.Model/QuickConnect/QuickConnectResult.cs`.
- Jellyfin OpenAPI, https://api.jellyfin.org/openapi/jellyfin-openapi-stable.json (version 12.2.0): `AuthenticateUserByName`,
  `AuthenticateWithQuickConnect`, `InitiateQuickConnect`, `GetQuickConnectState`, `GetQuickConnectEnabled`, `GetCurrentUser`,
  `ReportSessionEnded`, `AuthenticationResult`, `QuickConnectResult`.
- Jellyfin Kotlin SDK v1.9.2 (commit 88f916be): `jellyfin-api/.../util/AuthorizationHeaderBuilder.kt`, `util/url.kt`,
  `jellyfin-core/src/androidMain/.../AndroidDevice.kt`. Jellyfin TypeScript SDK v1.0.1 (commit 26191928): `src/utils/authentication.ts`.
- jellyfin-web v12.2 (commit 61b1f890): `src/components/apphost.js`, `src/apps/legacy/controllers/session/login/index.js`.
- Jellyfin Desktop v2.0.0 (commit 1634d3d): `src/utils/Utils.cpp`, `src/utils/osx/OSXUtils.mm`,
  `src/settings/SettingsComponent.cpp`, `resources/settings/settings_description.json`.
- Jellyfin Kotlin SDK v1.9.2 `jellyfin-core/src/androidMain/.../JellyfinOptions.kt` (`deviceInfo ?: androidDevice(context)`).
  Jellyfin Android v2.7.3 (commit 9a5f9be): `app/src/main/java/org/jellyfin/mobile/app/ApiModule.kt` (`createJellyfin`),
  `ApiClientController.kt`, `bridge/NativeInterface.kt`. Jellyfin Android TV
  v0.20.0-beta.4 (commit 344d26e): `app/src/main/java/org/jellyfin/androidtv/di/AppModule.kt`.
- Swiftfin `main` at 04e0aa1 (2026-10-21): `Shared/Extensions/JellyfinAPI/JellyfinClient.swift`.
