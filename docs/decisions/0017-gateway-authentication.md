# 0017: Authentication gateways in front of a server

Status: proposed (2026-10-22), from the gateway spike; accepted when its PR merges. The owner decided the six open choices on 2026-10-22
(see "Decided by the owner").
Implementation comes after M1. Research, quotes and sources: `docs/notes/gateway-auth.md`.

## Context
Some users put an authentication gateway between the internet and their servers: an HTTP Basic password in a reverse proxy (an
Nginx Proxy Manager access list), a portal that signs the user in and sets a cookie (Authelia, authentik, oauth2-proxy, Pomerium,
Cloudflare Access, often with Google as the identity provider behind it), a secret header (Cloudflare service tokens, a proxy rule), or
client certificates. Today Current fails behind every one of them: setup reports `Redirected`, `NotJellyfin` or a status (ADR 0006,
ADR 0012), and a gateway's 401 mid-session would wipe the Jellyfin session (ADR 0015's known risk).

The owner's goal (2026-10-22): "something adaptive so the app doesn't handle each possible way of authorization, but instead goes to
the authorization page or another better way". So the app should recognise a few **shapes** of answer and offer one generic mechanism
per shape, with no code per gateway product.

What the research found (details in the notes):
- **Jellyfin and `Authorization` collide.** Jellyfin 12.2 reads its client fields and token from `Authorization: MediaBrowser ...`;
  the alternative `X-Emby-Authorization` header works only with `EnableLegacyAuthorization`, which a 2026 migration turns off and the
  web dashboard does not show. After sign-in the token also works as the `ApiKey` query value, but sign-in itself needs the header.
  A gateway that needs `Authorization` for itself (Basic, or a bearer token) therefore blocks sign-in unless the server or the gateway
  is configured for it. Authelia's default endpoints also refuse a request whose `Authorization` is `MediaBrowser`, even with a valid
  cookie.
- **Jellyfin's own 401 carries no `WWW-Authenticate`** (ASP.NET Core's default challenge, unchanged by Jellyfin), and its public probe
  endpoint never answers 401 or 403. Verified from source, not run.
- **Gateways answer in three shapes:** 401 with `WWW-Authenticate: Basic`; a redirect, an HTML page, or a 401/403 without a Basic
  challenge (a web sign-in); a 403 that needs a fixed credential. Which of 302 or 401 depends on `Accept` and configuration.
- **Every portal gateway keeps its session in cookies** (HttpOnly, Secure, often on a parent domain, lifetimes from an hour to a
  week), and all three platform web views let the app read them, HttpOnly included.
- **Google blocks its sign-in in embedded web views** (named for WKWebView; the policy's wording covers WebView2 and WebKitGTK). So a
  gateway with Google behind it cannot be signed in to in a web view, whichever gateway it is.
- **The system browser cannot hand cookies back.** Only a gateway that redirects to the app with a token works that way: Pomerium does
  (documented, loopback); Authelia can with an OIDC client the administrator sets up; Cloudflare only through cloudflared's
  undocumented transfer service.
- **AirPlay and Cast receivers fetch the stream themselves** and get only what is in the URL.

## Decision

### Classifying the answer
The unauthenticated probe (ADR 0012's `GET {base}/System/Info/Public`) and, later, any request, is classified:

| Answer | Shape | What the app offers |
|---|---|---|
| Jellyfin's JSON | no gateway, or this address bypasses it | nothing |
| 401 with `WWW-Authenticate: Basic` | **password gateway** (realm kept for the prompt) | a native password prompt |
| A redirect that ADR 0006 does not follow (another host or port), or a followed one that ends anywhere but Jellyfin's JSON; an HTML page where JSON was expected; a 401 or 403 without a Basic challenge | **web sign-in gateway** | a sign-in window on the server's address |
| 403 after a completed web sign-in, or 403 with no page | **gateway that wants something else** | "advanced: extra headers" (and later client certificates), with an explanation |
| 401 with another scheme only (Digest, NTLM, Negotiate, Bearer) | unsupported | "the server asks for a sign-in method Current does not support" |

Product names are never needed to decide. A redirect to another host stays ambiguous (a moved server or a portal), so setup offers
both: "Sign in" (the web sign-in below) and ADR 0012's "use the address it points to". The probe stays without credentials the first
time; once a gateway credential exists for the address, the probe carries it (see "Order of credentials").

### Mechanism 1: password gateways (HTTP Basic)
- A native prompt shows the origin (scheme, host and port), the realm as "The site says: ..." only when present (it is the server's
  text, never trusted as a title), user name and password. Windows: a `ContentDialog`; macOS: a sheet; GNOME: an `AdwAlertDialog`,
  like WebKitGTK's own.
- The credentials go in the keyring (ADR 0013) under `gateway/<server key>/<address>` (address as its position-independent id, not
  the URL), and are sent pre-emptively as `Authorization: Basic` on every request to that address's origin, including the probe and
  redirects that ADR 0006 follows (same host).
- **Jellyfin's credentials then move to `X-Emby-Authorization`.** This works only when the server has `EnableLegacyAuthorization` on
  **and** the gateway does not pass its `Authorization` header on (NPM: "Pass Auth to Upstream" off). When sign-in answers 400 (the
  four fields did not arrive), the app explains both settings. See open choice 3.
- A wrong or expired gateway password answers 401 with a Basic challenge again: the prompt reappears; nothing else changes.

### Mechanism 2: web sign-in (the default for portals)
- **A sign-in window** (Windows: its own window with WebView2; macOS: a sheet with WKWebView; Linux: a dialog with WebKitGTK) opens the
  server's address, the same URL the probe used minus the API path, and lets the user complete whatever the gateway shows: its own
  login form, a one-time PIN, an identity provider, Basic (the web view's own dialog).
- **It shows where the page is**, as a browser's address bar does: the current origin, with a lock for verified https and a warning
  otherwise, updated on every navigation. The window title names the server's origin: "Sign in to media.example.com".
- **Its own profile:** a non-persistent data store (`WKWebsiteDataStore.nonPersistent()`, WebKitGTK's ephemeral session; for WebView2
  an InPrivate controller or a throw-away user data folder, which one not verified), so nothing leaks between servers or into the
  user's browser, and every sign-in starts clean. No script bridge to the app, no downloads; pop-ups open in the same window.
- **Completion:** after each navigation that lands back on the server's origin, the core probes with the cookies the web view would
  send to the server's URL. When the probe returns Jellyfin's JSON with the expected identity (ADR 0012, ADR 0014), the window closes.
  The user can also close it (cancel).
- **What is taken:** every cookie that applies to the server's base URL by RFC 6265 (domain, path, Secure), with its attributes and
  expiry, not one cookie by name: that covers split cookies (oauth2-proxy's `_oauth2_proxy_0`, `_1`), binding cookies (Cloudflare's
  `CF_Binding`) and unknown gateways. Cookies for other hosts (the identity provider, the gateway's portal host) stay in the web view
  and are discarded with it.
- **Google:** when a navigation goes to Google's sign-in (`accounts.google.com`), the window explains instead of letting the user hit
  `disallowed_useragent`: "Google doesn't allow signing in inside apps. Use another sign-in method of your gateway (for example a
  one-time code), or a service token". This is the one host name the app knows, and only for a message. The web view is never disguised
  as a browser (Google's policy).
- **Expiry:** a later request that gets the web sign-in shape (see the 401 rule below) marks the server "Sign in to the gateway
  again"; the Jellyfin session is kept and resumes after the user signs in to the gateway.

### Mechanism 3: extra headers (advanced, per address)
In the server's settings, "Advanced: extra headers" takes header names and values (Cloudflare service tokens'
`CF-Access-Client-Id` and `CF-Access-Client-Secret`, a pasted `cf-access-token`, a proxy's secret header). Values go in the keyring and
are sent on every request to that address. Names that Current sets itself (`Authorization` when Jellyfin uses it, `Cookie`, `Host`,
`Content-Length` and the other framing headers) are refused. Shown only on request (advanced), with a short explanation.

### Out of scope (for now)
- **System browser with an app callback** (Pomerium's programmatic login; Authelia's OIDC bearer tokens): only Pomerium supports it
  without administrator work, so it waits for a user who needs it (open choice 2). Cloudflare's cloudflared transfer service is
  undocumented and is not used.
- **Client certificates** (mutual TLS): a later spike (OS certificate stores, rustls client authentication).
- **Digest, NTLM, Negotiate**; gateways in front of Seerr and Stash until M4 (the per-address layer below applies unchanged).

### Where the credentials go
- **Per address, not per server.** A gateway usually sits only on the public address; the LAN address often bypasses it. Gateway
  credentials (Basic, cookies, extra headers) belong to the address they were obtained on. Cookies are additionally matched per
  request by RFC 6265 (domain, path, Secure, expiry), so a cookie for `media.example.com` never goes to `192.168.1.10`, and roaming
  (ADR 0014) never sends a gateway credential to another address. A second address under the same cookie domain gets the cookie only if
  RFC 6265 says it applies.
- **Every request through `current-net`:** a gateway layer next to `RequestAuth`, applied by `ServerClient` for the selected address on
  every request and every followed redirect, after the log entry is built (ADR 0006), so neither values nor names of cookies reach the
  log.
- **Set-Cookie updates:** gateways extend or rotate their cookies (Authelia's inactivity window, oauth2-proxy's refresh). Whether the
  core reads `Set-Cookie` back is open choice 4.
- **Images:** the core fetches image bytes with the same credentials and hands the UI the bytes (UI image loaders cannot add headers).
  M2 decides the image path; this ADR only requires that it carries gateway credentials.
- **The player (M2, M3):** libmpv gets gateway cookies through FFmpeg's own `cookies` option, passed through `stream-lavf-o` as a
  per-file option of `loadfile`, never a cookie file (mpv's own `cookies` only switches its cookie file on), and Basic or extra headers
  through FFmpeg's `headers` the same way. FFmpeg keeps headers on a redirect to another host, so every stream gets `max_redirects=0`
  and a redirect fails; every stream is also opened with `tls-verify=yes` (mpv's default is off). FFmpeg 9.0.2 matches a cookie's
  domain against the request's Host value, port included, and ignores Secure (`libavformat/http.c`, `get_cookies`), so the engine
  gives each cookie the stream URL's host and port as its domain and sends Secure cookies only over https. External subtitles are
  fetched by the core, not by mpv. Amended by ADR 0019 (owner, 2026-10-25); written in `current-mpv` (docs/notes/mpv-engine.md). AVPlayer takes cookies
  through `AVURLAssetHTTPCookiesKey`; headers have no supported API, so header and Basic gateways on Apple need a resource loader or a
  loopback relay in the core, decided in M3.
- **AirPlay and casting:** a receiver gets only the URL. When the address in use has gateway credentials, the cast and AirPlay buttons
  stay visible but say "Not available while connected through <origin>'s sign-in; connect on the local network to use it" (the
  platform's buttons stay where they are; only the action is refused). With roaming on a LAN address without a gateway, they work.

### Order of credentials and security
1. **Probe:** carries only the gateway credentials of the address being probed, never the Jellyfin token. Sending them before the
   identity check is what a browser does: they were obtained from (cookies: issued for) that origin.
2. **Identity check** (ADR 0012, ADR 0014) runs on the probe's answer.
3. **Jellyfin token** only after the identity check passed, as today.
- Gateway credentials follow ADR 0014's transport rule for tokens: verified https, or plain http the user accepted under ADR 0012.
  Cookies marked Secure never go over http (RFC 6265). See open choice 5.
- Redirects keep ADR 0006's rule (same host only), so no gateway credential leaves its origin; cookie matching is recomputed per hop.
- **No value in logs, errors or Debug:** gateway credential types have redacted Debug and no Display; `NetError` keeps carrying no
  header; the request log already omits headers and `Set-Cookie` (tested in `current-net`). Cookie names are not logged either (a
  name can carry an app id).
- **Keyring size:** a Windows credential holds at most 2560 bytes (ADR 0013), and gateway cookies can be larger (oauth2-proxy splits
  at 4000). Gateway secrets are stored as numbered chunks under one key (`.../0`, `.../1`), written and deleted together. Proposed
  amendment to ADR 0013.
- **Phishing:** the sign-in window only ever starts at the server's own address, shows the origin of every page, and never fills in
  anything itself.

### The 401 rule (proposed amendment to ADR 0015)
A 401 ends the Jellyfin session only if it is Jellyfin's:
1. A 401 with any `WWW-Authenticate` challenge is a gateway's: never a session end. With a Basic challenge the password prompt
   reappears.
2. Any other 401 (and any redirect or HTML page where JSON was expected) on a server: the core re-runs the identity probe with the
   address's gateway credentials before acting. If the probe answers Jellyfin's JSON with the right identity, the 401 was Jellyfin's:
   ADR 0015's rule applies. If the probe gets a gateway shape, the server shows "Sign in to the gateway again" and the session and
   token are kept. If the probe fails to connect, nothing changes (roaming handles it).
This also covers gateways that answer 401 without a challenge (Pomerium, oauth2-proxy's JSON errors, Authelia's 401 with a `Location`).
`NetError::Auth` gains the challenge schemes (`Basic`, others by name; no realm or parameters in Debug), which is a proposed amendment
to ADR 0006.

### Jellyfin's header placement (`current-jellyfin`)
A per-server setting, `Authorization` (default) or `X-Emby-Authorization`. It changes to `X-Emby-Authorization` automatically when the
address has Basic gateway credentials, and when a request carrying `Authorization: MediaBrowser` gets a gateway answer with a challenge
while the probe (without that header) passes, as Authelia's default endpoints do. In the second case the request provably did not
reach Jellyfin, so a sign-in is retried once with the other header. A sign-in that then answers 400 means the server ignores
`X-Emby-Authorization`: the error explains the two server-side fixes from the notes (legacy authorization on, and the gateway not
passing its `Authorization` on; or a gateway rule that lets Jellyfin's API through).

## Decided by the owner (2026-10-22)
1. Web sign-in: the **embedded sign-in window** by default; for Google-backed gateways, **extra headers** (a service token) as the
   fallback.
2. Pomerium's programmatic login: **later**, when a user needs it.
3. Basic gateways in front of Jellyfin: **option A**, supported, with clear errors naming the server and proxy settings to change.
4. Cookie updates: **option B**, our own small `Set-Cookie` reader in `current-net`, no new dependency.
5. Gateway credentials over plain HTTP: **option A**, the same rule as the Jellyfin token (ADR 0012).
6. The 401 rule amendment to ADR 0015: **approved**.

The options as they were put to the owner follow.

## Open choices (as proposed)

1. **Web sign-in: embedded web view (recommended) or the system browser.** With the web view, the app gets the gateway's cookies and
   works with Authelia, authentik, oauth2-proxy, Pomerium and Cloudflare Access when they use their own login form, a one-time PIN or
   an identity provider other than Google (other providers not verified). It cannot work when Google is the identity provider: Google
   refuses web views, and the user sees our explanation. With the system browser, every identity provider works, Google included, but
   the cookie stays in the browser, so it works only with gateways that send a token back to the app (Pomerium; Authelia with
   administrator setup). **Recommendation:** the web view as the default; for Google-backed gateways, extra headers (a Cloudflare
   service token, which the user creates in their own Cloudflare account) as the fallback; the system browser only through choice 2.
   Consequence: a user whose gateway signs in only with Google must create a service token once.

2. **Pomerium's programmatic login, later.** It is a documented system-browser flow with a loopback callback (RFC 8252): the app opens
   Pomerium's sign-in in the user's browser and receives a token. It would make Pomerium with Google work, but it is code for one
   product. **Recommendation:** not now; add it when a user needs it. Consequence: Pomerium with Google needs a service account
   (Pomerium's paid editions) until then.

3. **Support for Basic gateways in front of Jellyfin.** Because Jellyfin needs `Authorization`, a Basic gateway works only if the
   server's administrator turns on Jellyfin's "legacy authorization" (a hidden setting Jellyfin turned off in 2026 and may remove) and
   the gateway does not pass the password on. Option A: support it as described, with clear errors. Option B: do not use Basic
   credentials for Jellyfin at all and tell the user to let Jellyfin's API through the gateway (an IP rule or a bypass). **Recommendation:
   A**, since Basic matters more for Seerr and Stash later (no collision there). Consequence: users with default Jellyfin settings
   still need a server change, and the app says which.

4. **Updating cookies from `Set-Cookie`.** Option A: reqwest's `cookies` feature (a new dependency, `cookie_store` and its own
   dependencies; licences to check under ADR 0003). Option B: our own small `Set-Cookie` reader in `current-net`, only for cookies that
   already apply to the address. Option C: no updates; the cookies from the sign-in are used until the gateway asks again. With C,
   gateways with short idle windows (Authelia's default is 5 minutes of inactivity, 1 hour in all, or a month with "remember me") send
   the user back to the sign-in window often. **Recommendation: B**, no dependency and the narrowest behaviour. Consequence: about a
   hundred lines and tests in `current-net`.

5. **Gateway credentials over plain HTTP.** Option A (recommended): the same rule as the Jellyfin token (local addresses silently,
   other addresses after the one-time confirmation of ADR 0012). Option B: gateway credentials only over https. A is what users with a
   LAN proxy expect; B is stricter but breaks a Basic password on a LAN proxy without TLS. Consequence of A: on a confirmed public http
   address, the gateway password crosses the network in clear, as the Jellyfin token already does.

6. **The 401 rule amendment** (above): approve or not. Without it, a gateway session that expires signs the user out of Jellyfin.

## Proposed amendments to merged ADRs (for the owner)
- **ADR 0006:** `NetError::Auth` carries the challenge schemes; `ServerClient` gains the per-address gateway layer; `Redirected` and
  HTML answers feed the classification.
- **ADR 0012:** `ProbeError` gains `GatewayPassword { realm }`, `GatewaySignIn`, `GatewayRefused` and `UnsupportedChallenge`; a
  cross-host redirect offers "Sign in" next to "use this address".
- **ADR 0013:** gateway secrets under `gateway/<server key>/<address id>`, stored in chunks when larger than a Windows credential.
- **ADR 0014:** gateway credentials are per address and follow the same transport rule; the identity probe carries that address's
  gateway credentials.
- **ADR 0015:** the 401 rule above; the Jellyfin header placement.

## Implementation plan (after M1)
1. `current-net`: `GatewayCredentials` per address (Basic, extra headers, a cookie list with RFC 6265 matching) applied by
   `ServerClient`; `NetError::Auth { challenge }`; a response classifier (`GatewayShape`); open choice 4's cookie updates.
2. `current-jellyfin`: probe classification and the new `ProbeError` variants; header placement; sign-in's 400 diagnosis.
3. `current-store`: per-address gateway secrets, chunked; an address field saying which kind it has (never a value).
4. `current-app`: `AddServer` states `GatewayPassword { origin, realm }` and `GatewaySignIn { url }`; a server status "gateway
   sign-in needed"; the 401 rule; roaming probes with per-address gateway credentials.
5. `current-ffi`: the UI hands over cookies from the web view (name, value, domain, path, Secure, HttpOnly, expiry) and Basic
   credentials; values never come back to the UI.
6. Windows UI first: the password dialog, the sign-in window with the origin bar, the advanced headers editor. macOS and Linux with
   their milestones (M3, M5).
7. The player and images per "Where the credentials go", in M2 and M3.

## Test plan
Hand-written stand-ins on several ports of 127.0.0.1, as in `crates/net/tests/client.rs`, in front of a Jellyfin stand-in:
- **Basic proxy:** 401 with `WWW-Authenticate: Basic realm="..."` without credentials; passes with them; a mode that forwards
  `Authorization` and one that clears it; the Jellyfin stand-in answers 400 to a sign-in without the four fields. Checks: the probe
  classifies `GatewayPassword` with the realm; Jellyfin's header moves to `X-Emby-Authorization`; the 400 diagnosis; a wrong password
  re-prompts without touching the session.
- **Login portal:** a "portal" on another port that the gateway redirects to (302), and a JSON mode answering 401 with a `Location`
  (Authelia-like) and an HTML mode answering 403 (oauth2-proxy-like); the portal sets a cookie for the gateway's host and redirects
  back. Checks: classification `GatewaySignIn` in each mode; with the cookie the probe passes; an expired cookie gives "sign in to the
  gateway again" and keeps the Jellyfin token; a Jellyfin 401 through the gateway still ends the session (the confirming probe passes).
- **Header gate:** 403 unless a fixed header is present. Checks: extra headers pass; refused header names are rejected.
- **Scope:** a second address (another port, standing in for the LAN address) receives no gateway cookie, Basic or extra header
  (checked on the stand-in's received requests); a Secure cookie is not sent over http; a cookie for another domain is not sent; a
  redirect to another host carries nothing.
- **Order:** the probe carries gateway credentials but never the Jellyfin token; the token goes only after the identity check.
- **Redaction:** no log line, error or Debug output contains a cookie name or value, a Basic password or an extra header value.
- **Keyring:** chunked secrets round-trip and are deleted together (on the Windows and macOS CI runners, as ADR 0013's tests).
- **By hand, not in CI:** the sign-in window on Windows against the portal stand-in, and against a real gateway on the owner's own
  server (never in a cloud session). The web view's cookie APIs (HttpOnly cookies listed) are checked there.

## Consequences
- One mechanism per answer shape, no code per gateway product, except one host name (Google's sign-in) used for a message.
- Users behind a Google-only gateway need a service token or a gateway login method other than Google.
- Basic gateways in front of Jellyfin need a server-side change, which the app names.
- Casting and AirPlay do not work through a gateway.
- The web view is the first web content in Current: its window, profile and origin display are new UI on each platform.

## Sources (read 2026-10-22)
Full list with quotes in `docs/notes/gateway-auth.md`. Main ones:
- Jellyfin server `v12.2` (commit ca0f16eb): `Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`,
  `MediaBrowser.Model/Configuration/ServerConfiguration.cs`, `Jellyfin.Server/Migrations/Routines/20260531160000_DisableLegacyAuthorization.cs`,
  `Jellyfin.Api/Auth/CustomAuthenticationHandler.cs`, `Jellyfin.Api/Controllers/UserController.cs`; ASP.NET Core `v10.0.11`
  `src/Security/Authentication/Core/src/AuthenticationHandler.cs`.
- Authelia v4.39.28, authentik 2026.8.5, oauth2-proxy v7.15.5, Pomerium v0.33.4, Nginx Proxy Manager v2.16.0 (source and bundled docs).
- Cloudflare One documentation (authorization cookie, session management, service tokens, one-time PIN, CLI tutorial); cloudflared
  source at fa04bbf8.
- Google: OAuth 2.0 policies ("Use secure browsers", modified 2026-08-05), native-app guide (updated 2026-10-16), the 2016 and 2021
  blog posts on embedded web views. RFC 8252; RFC 6265; RFC 9110 §11.6.2.
- Apple: `ASWebAuthenticationSession`, `AVURLAssetHTTPCookiesKey`, `WKHTTPCookieStore`, `WKWebsiteDataStore`; Microsoft: WebView2
  `CoreWebView2CookieManager`, `BasicAuthenticationRequested`, `OAuth2Manager`, WinUI dialogs; WebKitGTK 2.54 reference.
- mpv v0.41.0 `DOCS/man/options.rst`, `stream/stream_lavf.c`; FFmpeg `libavformat/http.c`, `hls.c`, `aviobuf.c` (master 179d0834).
