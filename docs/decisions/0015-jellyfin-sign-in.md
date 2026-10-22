# 0015: Jellyfin sign-in, sessions and saved servers

Status: proposed (2026-10-22), from M1; accepted when its PR merges. The device name rule and the tokio features were decided by the
owner (2026-10-22, marked **Decided by the owner**).

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
  `QuickConnectExpired`, `SessionRevoked` (401 with a token), `NotSignedIn`, `Network(NetError)`.
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

### Saved servers (`current-model`, `current-store`)
- `SavedServer { server: ServerRef, id: ServerId, name, addresses: Vec<ServerAddress>, users: Vec<SavedUser { id, name }>,
  active_user: Option<UserId> }`, in `settings.json` as a defaulted `servers` field: format 1, no migration (ADR 0013). The file keeps
  ids, names and addresses, never a token. `server_id` is defaulted, since a service without one (Stash, ADR 0014) may store none.
  Unknown fields inside a server record are not kept on save (only top-level ones are); a
  newer version that adds one must raise the format if an older one dropping it matters.
- The token lives only in the keyring under `SecretKey { server, user }` (ADR 0013).
- A sign-in to a server whose id is already saved reuses its key, adds the address if new and adds or updates the user. Otherwise
  the server gets the next numeric key. Signing in as another user makes that user active; the earlier user's token stays in the
  keyring for user switching later (not built).

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
  first probes its address with `probe_expecting` (ADR 0012), so no password and no Quick Connect request goes to an address that
  does not answer as that server (ADR 0014; setup has just probed a new one). A sign-in whose `ServerId` differs from the expected
  server's is not saved (`Probe(DifferentServer)`).
- **Restore:** `App::open` loads the saved servers into the capabilities state, signed in where a user is active, reachability
  unknown. The shell then runs `App::restore_sessions()` in the background; it checks every server at once (`JoinSet`): the token from
  the keyring (none: signed out and the active user cleared; keyring unavailable: signed out for this run, the user kept); then the
  identity probe on the first address (`probe_expecting`, ADR 0012), because the token goes only to an address that answers as the
  same server (ADR 0014); then `GET /Users/Me`. Results: signed in with user, permissions and version; signed out on 401 (the 401
  rule); unreachable but still signed in when the server does not answer or another server answers there (no token is sent then).
  It may run again later for servers without a live session. Servers of other services are skipped.
- **Sign out:** `App::sign_out` calls `POST /Sessions/Logout` (10 s timeout), then deletes the token and the active user whatever the
  server answered, and returns the server's answer for the UI to mention. The user stays saved. It signs out the session current when
  it started: a sign-in that completes while the server is being told is kept (tested).
- **Ordering:** saving a sign-in, signing out, the 401 rule and restoring each change a server's session under one async lock and
  check under it that the session or active user they act on is still current. Restore installs its client only if no sign-in or
  sign-out happened during its probe, and writes the status only while its session is current.
- **Address:** each server uses `FixedAddress` with its first address, built in one place (`Shared::client`), so roaming (ADR 0014)
  replaces it there.
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
  reporting another server's id; settings from a newer version; sign out with the server up and gone, and racing a new sign-in; no
  keyring; settings round trip. None ran against a real Jellyfin server.

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
