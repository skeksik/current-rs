# 0018: Jellyfin sign-in

Status: accepted (2026-10-20). Logic and ViewModels; the screens come with the shells.

## Context
M1 ends when you can sign in and out of Jellyfin on Windows and in the browser, and the app restarts signed in. ADR 0016 saves servers
with the id they reported; ADR 0017 keeps tokens and the per-install device id. Jellyfin accepts only its `MediaBrowser` header since
12.0, revokes a token when the same device id signs in again, and offers Quick Connect (a code approved in another signed-in app).

## Decision
- **`JellyfinAccounts`** (`data:jellyfin`) owns Jellyfin sessions: password sign-in (`POST /Users/AuthenticateByName`), Quick Connect
  (`GET /QuickConnect/Enabled` to offer it, `POST /QuickConnect/Initiate`, whose 401 means it is off, polling
  `GET /QuickConnect/Connect?secret=` every 5 s for up to 10 minutes, where no answer or a 5xx means "not yet" and a 404 means expired,
  then `POST /Users/AuthenticateWithQuickConnect`), sign-out, and `authorizedClient` for the data layer to come.
- **The header** is `MediaBrowser Client="Current", Device="…", DeviceId="…", Version="…"[, Token="…"]` with URL-encoded values, built
  from `DeviceIdentity` and `AppInfo`, sent without `Token` until signed in.
- **Credentials reach only the saved server.** Requests go to the saved server's address (`ServerAddressSources`, the address roaming
  has proved, ADR 0019). The `ServerId` in a sign-in answer must equal the id saved at setup; otherwise the new session is ended on that server
  and nothing is kept.
- **What is kept:** the token in the `SecretStore` (`jellyfin/<server key>/token`); the user's id, name and admin flag in settings
  (`jellyfin.<server key>.user`), which are not secret and let the app show who is signed in while the server is offline.
- **At start** saved sessions go straight into the capabilities state, then `GET /Users/Me` checks each in the background: it refreshes
  the user, a 401 signs out, and an unreachable server stays signed in but offline (ADR 0014).
- **A 401** from an authorized client signs that server out (`ServerClients`' `onUnauthorized`) while the token that got it is still the
  current one: a revoked token, or the same device id signed in elsewhere. A 401 for an older token (a request sent before a new sign-in)
  changes nothing.
- **Sign-out** forgets the token, user and signed-in state here at once, then ends the session on the server in the app scope, best effort
  (`POST /Sessions/Logout`, 5 s), with the token and the address in use captured. Removing a server signs out first, so nothing waits on
  the network and leaving the page cannot stop it halfway.
- **Permissions:** Jellyfin's `Policy.IsAdministrator` maps to `Permission.admin`; nothing else yet.
- **ViewModels** (`feature:settings`): `SignInViewModel` (form, Quick Connect code, problems: wrong credentials, unreachable, timed out,
  refused, different server, Quick Connect off or expired; one sign-in job at a time) and `ServersViewModel` (saved servers with who is signed in, sign out, remove,
  where tokens are kept, and the opt-in file when there is no keyring).

## Consequences
- The web app has the same flow, through the `web` facade (the React sign-in page shipped in PR 37); CORS is open on Jellyfin (platform notes).
- The shapes come from the 12.2 OpenAPI spec. The opt-in live smoke tests (`LiveJellyfinSmokeTest`) confirmed password and Quick Connect
  sign-in, the restored-session check and sign-out against a live 12.2 server on 2026-10-20.
- The user's avatar is not loaded yet (`PrimaryImageTag`), and Quick Connect approval of another device's code (`/QuickConnect/Authorize`)
  is left for Seerr sign-in (M3).

## Addendum (2026-10-21): sign-out sends the token only to a proved address
- The address in use is captured only if it is proved at that moment (`ServerAddressSource.provedNow`). Right after a network change or
  a return from the background it may not be (ADR 0019): the logout then waits up to 5 s for the source to prove one, and is skipped if
  none is proved by then or the server was removed meanwhile. The sign-out here is done at once either way.
