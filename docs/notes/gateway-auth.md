# Authentication gateways in front of a server (spike)

Research behind ADR 0017: what an authentication gateway in front of Jellyfin answers to Current's requests, how a native app can get
through it, where the credentials must go, and what that means for security and for each platform's UI. All sources were read on
2026-10-22 unless a date says otherwise. Nothing was run against a live gateway or a real server; everything below is from
documentation and source code. Anything not checked against a source is marked "not verified".

Source trees read (shallow clones):

| Project | Tag or commit |
|---|---|
| Jellyfin server | `v12.2` (commit ca0f16eb, 2026-10-05) |
| jellyfin-web | `v12.2` (commit 61b1f890) |
| ASP.NET Core | `v10.0.11` (`AuthenticationHandler.cs`; Jellyfin 12.2 targets `net10.0` and pins `Microsoft.AspNetCore.*` 10.0.11) |
| Authelia | `v4.39.28` |
| authentik | `version/2026.8.5` |
| oauth2-proxy | `v7.15.5` |
| Pomerium | `v0.33.4` |
| Nginx Proxy Manager (NPM) | `v2.16.0` |
| cloudflared | default branch at fa04bbf8 (after tag 2026.9.3) |
| mpv | `v0.41.0` (41f6a645), and `master` at 01a7420c for one unreleased change |
| FFmpeg | `master` at 179d0834 |
| nginx | `master` (`src/http/ngx_http_request.c`) |
| reqwest | 0.13.5 (crates.io) |

## 1. Jellyfin: where its credentials go

### Which headers carry the token
`Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`:
- The client fields (`Client`, `Device`, `DeviceId`, `Version`) and the token come from **one header**:
  `GetAuthorizationDictionary` reads `Authorization`, and `X-Emby-Authorization` only
  `if (_configurationManager.Configuration.EnableLegacyAuthorization && string.IsNullOrEmpty(auth))`. The scheme must be
  `MediaBrowser` (or `Emby` with the legacy flag); any other scheme gives no fields (`if (!validName) { return null; }`).
- Token fallbacks, in order: `X-Emby-Token` and `X-MediaBrowser-Token` (legacy flag only), then the query `ApiKey` (**always**:
  `token = queryString["ApiKey"];`), then `api_key` (legacy flag only).
- **The legacy flag is off.** `MediaBrowser.Model/Configuration/ServerConfiguration.cs`: `public bool EnableLegacyAuthorization { get;
  set; }` (no initializer, so `false`), and the migration `Jellyfin.Server/Migrations/Routines/20260531160000_DisableLegacyAuthorization.cs`
  sets it to `false` on existing servers ("Migration to disable legacy authorization in the system config"). jellyfin-web v12.2 has no
  setting for it (`src/` searched for `LegacyAuthorization`: no match), so an administrator can turn it on only in `system.xml`
  (`BaseApplicationPaths.SystemConfigurationFilePath => Path.Combine(ConfigurationDirectoryPath, "system.xml")`) or through the
  configuration API. Its name says Jellyfin considers it on the way out (not verified that removal is planned).
- With a token, the device fields fall back to the token's device record (`if (string.IsNullOrWhiteSpace(authInfo.Client)) {
  authInfo.Client = device.AppName; }` and so on), so **requests after sign-in work with `?ApiKey=<token>` alone**.
- **Sign-in needs the header.** `Jellyfin.Api/Controllers/UserController.cs` `AuthenticateUserByName`: `var auth = await
  _authContext.GetAuthorizationInfo(Request)`, then `App = auth.Client, AppVersion = auth.Version, DeviceId = auth.DeviceId, DeviceName =
  auth.Device`, and ADR 0015 found that empty values fail with 400. Quick Connect's `Initiate` needs the same four values (ADR 0015).

**Consequence:** HTTP allows one `Authorization` credential per request (RFC 9110 §11.6.2: `Authorization = credentials`), and nginx
refuses a request with two (`ngx_http_request.c`: `{ ngx_string("Authorization"), ..., ngx_http_process_unique_header_line }`, which
answers 400 "client sent duplicate header line"). So when a gateway needs `Authorization` for itself (HTTP Basic, or a bearer token),
Jellyfin 12.2 with default settings **cannot be signed in to** through it: the four client fields have nowhere else to go. Two ways
out, both on the server side:
1. The administrator turns on `EnableLegacyAuthorization`, **and** the gateway does not forward its own `Authorization` to Jellyfin
   (Jellyfin reads `X-Emby-Authorization` only when `Authorization` is empty). NPM's access list clears it unless "Pass Auth to
   Upstream" is on (section 3.5); authentik clears it after a successful header login (section 3.2).
2. The gateway lets Jellyfin's API through without its own `Authorization` (an IP rule such as NPM's "Satisfy Any", a cookie session,
   a header the gateway accepts instead, or a bypass rule).

Jellyfin's own web client has the same problem behind a Basic gateway: it sets `Authorization: MediaBrowser ...` on its own requests
(ADR 0015, TypeScript SDK), which replaces the browser's Basic credentials (inferred; not tested).

### What Jellyfin's 401 looks like
- `Jellyfin.Api/Auth/CustomAuthenticationHandler.cs` overrides only `HandleAuthenticateAsync`. ASP.NET Core's default
  (`src/Security/Authentication/Core/src/AuthenticationHandler.cs`, v10.0.11):
  `protected virtual Task HandleChallengeAsync(AuthenticationProperties properties) { Response.StatusCode = 401; return
  Task.CompletedTask; }`. **No `WWW-Authenticate` header.**
- No `WWW-Authenticate` anywhere in Jellyfin's C# (`grep -rni "WWW-Authenticate\|HandleChallenge"`: no match), so the sign-in 401
  (`AuthenticationException` through `ExceptionMiddleware`, ADR 0015) carries none either.
- `/System/Info/Public` never answers 401 or 403 (ADR 0012: no `[Authorize]`; `IpBasedAccessValidationMiddleware` answers 503). **A
  401 or 403 to the public probe is never Jellyfin's.**

This settles ADR 0015's "known risk" (it was marked "not verified against a live server"; now verified from source, still not run):
a 401 with any `WWW-Authenticate` challenge did not come from Jellyfin.

## 2. What gateways answer without a session

Current's requests send reqwest's default `Accept: */*` (reqwest 0.13.5 `src/async_impl/client.rs`: `headers.insert(ACCEPT,
HeaderValue::from_static("*/*"))`) and no `X-Requested-With`.

| Gateway | Answer to an API request without a session | Decided by | `WWW-Authenticate` |
|---|---|---|---|
| NPM access list (nginx `auth_basic`) | 401 | always | `Basic realm="Authorization required"` (nginx standard; realm from NPM's template; header not checked in nginx source) |
| Authelia, forward-auth (Traefik, Caddy) | 401 if `X-Requested-With: XMLHttpRequest` or Accept lacks `text/html` (`*/*` first counts as HTML); else 302 (GET, HEAD, OPTIONS) or 303 to the portal | `Accept`, `X-Requested-With` | none on the redirect path; the 401 carries `Location` and a small HTML body |
| Authelia, nginx `auth_request` | 302 to the portal (the documented `error_page 401 =302 $redirection_url;`) | nginx config | none |
| Authelia, a refused `Authorization` header | 401 | the header strategy | `Basic realm="Authorization Required"` |
| authentik, forward auth (Traefik, Caddy, Envoy) | 302 to the IdP sign-in (`http.StatusFound`) | always | none found |
| authentik, nginx | 302 to `/outpost.goauthentik.io/start` (documented `error_page 401`) | nginx config | none found |
| authentik, proxy mode | 302 to `/outpost.goauthentik.io/start`; **401** if the request has an `Authorization` header and "Intercept header authentication" is on (default) | `Authorization` present | none found |
| oauth2-proxy, proxy mode | **403 with an HTML sign-in page** by default; 302 with `--skip-provider-button`; 401 with body `{}` if Accept is exactly `application/json`, the path is an `--api-route`, or `--force-json-errors` | Accept, config | none |
| oauth2-proxy, `/oauth2/auth` (nginx) | 401, which the documented nginx config turns into the sign-in (`error_page 401 = @oauth2_signin;`) | nginx config | none |
| Pomerium | 302 to the authenticate service if `text/html` is the most acceptable type, else **401** ("Unauthenticated") | Accept | only on MCP routes |
| Cloudflare Access | "a redirect or `403` error" (Cloudflare's agents page); OPTIONS preflights get 403 | not documented | only with Managed OAuth ("401 response with a `www-authenticate` header") |

Sources: Authelia `internal/handlers/handler_authz_util.go` (`if ctx.IsXHR() || !ctx.AcceptsMIME("text/html") { return
fasthttp.StatusUnauthorized }`), `internal/middlewares/authelia_context.go` (`(i == 0 && mimeType == "*/*")`),
`docs/content/integration/proxies/nginx.md`, `docs/content/reference/guides/proxy-authorization.md`, `internal/handlers/const.go`;
authentik `internal/outpost/proxyv2/application/oauth.go`, `mode_forward.go`, `website/docs/add-secure-apps/providers/proxy/`;
oauth2-proxy `oauthproxy.go` (`if p.forceJSONErrors || isAjax(req) || p.isAPIPath(req)`), `docs/docs/features/endpoints.md` ("only
returns a 202 Accepted response or a 401 Unauthorized response"); Pomerium `authorize/check_response.go`; NPM
`backend/templates/_access.conf`; Cloudflare https://developers.cloudflare.com/cloudflare-one/access-controls/authenticate-agents/ and
`.../http-apps/authorization-cookie/cors/`. The exact Cloudflare redirect target (the team domain's login) is not verified.

**What this means:** the answers fall into three shapes that need no product knowledge:
- **401 with `WWW-Authenticate: Basic`**: a gateway that takes a user name and password.
- **A gateway page:** a redirect to another host, or to another path on the same host that is not Jellyfin's; an HTML page where
  JSON was expected; a 401 or 403 without a Basic challenge (to the public probe, never Jellyfin's). Each needs a sign-in in a web
  page.
- **403 that stays 403** after a web sign-in or with no sign-in page: a gateway that wants something else (a service token, a
  client certificate, an allowed IP).

Sending `Accept: application/json` on API requests would make Authelia, oauth2-proxy and Pomerium answer 401 instead of a redirect or
page. That helps no classification (both shapes are handled), but it makes the answer cheaper and matches what the API returns.

## 3. Gateways in detail

### 3.1 Authelia (v4.39.28)
- **Session cookie** (`docs/content/configuration/session/introduction.md`): `authelia_session`; `same_site` default `lax`;
  `inactivity` default "5 minutes", `expiration` "1 hour", `remember_me` "1 month"; `cookies[].domain` is required and "Must either be a
  valid suffix of or exactly match" the portal's host, so one cookie covers sibling subdomains. Secure is always set
  (`internal/session/provider_config.go`: `c.Secure = true`); path `/` and HttpOnly from `fasthttp/session` v2.5.9 `cookie.go`.
- **Credentials in a header:** the default authz endpoints try `HeaderAuthorization` then `CookieSession`
  (`proxy-authorization.md` table). The header strategy accepts `Basic` (default) or `Bearer`
  (`server-endpoints-authz.md`: "Options are `Basic`, and `Bearer`"), and on failure "Responds with the [WWW-Authenticate] header and a
  [401 Unauthorized]". A `HeaderProxyAuthorization` strategy reads `Proxy-Authorization` and answers 407 instead.
- **Collision with Jellyfin:** `internal/model/authorization.go` rejects any other scheme ("invalid scheme: scheme with name '%s' is
  unknown"), and `handler_authz.go` returns that error before the cookie strategy runs (`if strategy.CanHandleUnauthorized() { return
  authn, strategy, err }`). So **a request with `Authorization: MediaBrowser ...` is refused with 401 even with a valid session
  cookie**, on the default endpoints (read from the code, not run). An administrator can configure an endpoint with only
  `CookieSession`, or bypass rules for Jellyfin.
- **Native-client tokens:** Authelia's OpenID Connect provider can issue tokens with the `authelia.bearer.authz` scope, usable "for
  the forwarded authentication flow ... in place of the standard session cookie"
  (`docs/content/integration/openid-connect/oauth-2.0-bearer-token-usage.md`), with PAR and PKCE S256 required and the `form_post`
  response mode only. That is a system browser + loopback flow, but it needs a client registered by the administrator and the token
  travels as `Authorization: Bearer`, colliding with Jellyfin again unless the proxy-authorization strategy is configured for Bearer
  (not verified).

### 3.2 authentik (2026.8.5, proxy provider)
- **Cookie** (`internal/outpost/proxyv2/application/application.go`, `session.go`): `authentik_proxy_<8 hex digits>`, HttpOnly,
  Secure when the external host is https, SameSite Lax, path `/`, domain the configured cookie domain (the parent domain in
  domain-level forward auth). `MaxAge` = the access token validity + 1, default `hours=1` (`authentik/providers/oauth2/models.py`).
- **Header login** ("Intercept header authentication", default on, `authentik/providers/proxy/models.py`): Basic with an app password,
  or a Bearer JWT issued for the provider. "If the `Authorization` header value is invalid, authentik returns an HTTP `401`"; on
  success it "removes the `Authorization` header" and the docs say to "persist the cookies returned by the outpost"
  (`header_authentication.mdx`). The session is checked first (`checkAuth`), so a valid cookie plus `Authorization: MediaBrowser`
  passes (read from code).
- No proxy-level system-browser flow found. The OAuth2 provider has a device code flow; using it for the proxy provider is not verified.

### 3.3 oauth2-proxy (v7.15.5)
- **Cookie** (`pkg/apis/options/cookie.go`): `_oauth2_proxy`, path `/`, expire `168h`, refresh off (`0`), Secure and HttpOnly on,
  SameSite unset, domain optional (a parent domain with a leading dot). Sessions larger than 4000 bytes are split into `_oauth2_proxy_0`, `_1`, ...
  (`pkg/sessions/cookie/session_store.go`, `maxCookieLength = 4000`).
- **Header login:** `--skip-jwt-bearer-tokens` (off by default) accepts verified bearer JWTs; `--htpasswd-file` adds Basic. An
  `Authorization: MediaBrowser ...` is logged and falls through to the cookie (`pkg/middleware/jwt_session.go`; 403 only with
  `--bearer-token-login-fallback=false`).
- The client's own `Authorization` reaches the upstream by default (inferred from `pass-authorization-header` off and
  `skip-auth-strip-headers` stripping only headers oauth2-proxy would set).
- No native-client flow found.

### 3.4 Pomerium (v0.33.4)
- **Cookie** (`config/options.go`): `_pomerium`, HttpOnly, expire 14 h, Secure (`config/session.go`); no default domain (host-only,
  inferred).
- **Header forms** (`internal/sessions/header/header.go`): `X-Pomerium-Authorization: <JWT>`, `Authorization: Pomerium <JWT>`,
  `Authorization: Bearer Pomerium-<JWT>`. `internal/httputil/headers.go`: `X-Pomerium-Authorization` "can be used in place of the
  standard authorization header if that header is being used by upstream applications". No collision with Jellyfin.
- **Programmatic login** (`proxy/handlers.go` `ProgrammaticLogin`, `internal/authenticateflow/`; docs
  https://www.pomerium.com/docs/capabilities/programmatic-access):
  1. `GET https://<route>/.pomerium/api/v1/login?pomerium_redirect_uri=http://localhost:<port>/<path>`. The redirect must be in
     `programmatic_redirect_domain_whitelist`, default `localhost` (`config/options.go`); otherwise 400.
  2. The answer is `200 text/plain`, a signed sign-in URL ("returns a cryptographically signed sign-in URL").
  3. The app opens it in the system browser; the user signs in with the IdP.
  4. Pomerium redirects to the loopback URL with `?pomerium_jwt=<token>` (`urlutil.QueryPomeriumJWT = "pomerium_jwt"`), "an opaque
     token".
  5. The app sends it in `X-Pomerium-Authorization`. No lifetime or refresh is documented; the docs warn the underlying refresh token
     "may stop working", so the app starts again on a 401.
- Service accounts (same headers) are an Enterprise and Zero feature
  (https://www.pomerium.com/docs/capabilities/service-accounts).

### 3.5 Nginx Proxy Manager access lists (v2.16.0)
`backend/templates/_access.conf`:
```nginx
auth_basic            "Authorization required";
auth_basic_user_file  /data/access/{{ access_list_id }};
{% if access_list.pass_auth == 0 or access_list.pass_auth == false %}
proxy_set_header Authorization "";
{% endif %}
```
followed by the IP rules, `deny all;` and `satisfy any;` or `satisfy all;`. The UI calls the option "Pass Auth to Upstream"
(`frontend/src/locale/src/en.json`). The database default is on (`backend/migrations/20201014143841_pass_auth.js`:
`.defaultTo(1)`), but a list created in the UI starts off (`frontend/src/hooks/useAccessList.ts`: `passAuth: false`); which one users
get in practice is not verified. With it off, Jellyfin never sees the Basic header, so the legacy `X-Emby-Authorization` route of
section 1 works; with it on, it does not. With "Satisfy Any", a client from an allowed address skips Basic.

### 3.6 Secret-header rules
A gateway that requires a fixed header is configuration, not a product: Traefik routers match ``Header(`key`, `value`)``
(https://doc.traefik.io/traefik/reference/routing-configuration/http/routing/rules-and-priority/, v3.7 docs); in NPM it is hand-written
nginx in the host's "Advanced" field (no documented pattern; not verified). Cloudflare's service tokens (below) are the documented
product form.

## 4. Cloudflare Access
- **Cookies** (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/):
  `CF_Authorization` is set on the team domain (`cloudflareaccess.com`, the global session) and "stored as a cookie on the protected
  domain" (the application token). HttpOnly "is enabled by default"; SameSite "Admin choice (Default: None)"; the Secure flag is not
  stated (not verified). With "Cookie Path" off the cookie "is only scoped to the domain and subdomain".
- **Binding cookie:** "The `CF_Authorization` cookie cannot be used without the associated binding cookie" (`CF_Binding`) when that
  option is on. Taking every cookie the web view would send to the server, not one by name, covers it.
- **Lifetimes** (`.../access-settings/session-management/`): an application session duration per application (default 24 hours) and a
  global session duration (15 minutes to one month) after which Access asks the identity provider again. The JWT has `exp`. "Access
  can immediately revoke a single user session across all applications".
- **The token as a header:** `cf-access-token: <app token>` in place of the cookie (https://developers.cloudflare.com/cloudflare-one/tutorials/cli/,
  `curl -H "cf-access-token: $TOKEN"`; cloudflared `carrier/carrier.go`: `CFAccessTokenHeader = "Cf-Access-Token"`). The CLI page also
  says `cloudflared access curl` passes it "as a query argument named token", but the code sends the header.
- **Service tokens** (`.../service-credentials/service-tokens/`): `CF-Access-Client-Id` and `CF-Access-Client-Secret` headers, on an
  application whose policy action is "Service Auth"; otherwise "Access will prompt for an identity provider login". A token has a
  duration set at creation. A single `Authorization` header holding JSON is also accepted (which would collide with Jellyfin). Without
  strict mode Access answers with a `CF_Authorization` cookie; in strict mode it does not, and "Subsequent requests should continue to
  use service token headers".
- **One-time PIN** (`.../identity-providers/one-time-pin/`): a login method "as an alternative to integrating an identity provider";
  the code is mailed and "expires 10 minutes after the initial request". It is an ordinary web page, so it works in a web view (not
  tested).
- **Bypass** (`.../access-controls/policies/`): "The Bypass action ... disables Access enforcement for specific traffic", per
  application, which can be a path.
- **`cloudflared access login`** (cloudflared `token/token.go`, `token/transfer.go`, `token/encrypt.go`, `token/path.go`): it asks the
  application for its metadata (`cf-access-metadata-request: true`), tries a stored token or swaps the stored team token for an
  application token, else makes a NaCl box key pair, opens the system browser at
  `https://<app>/cdn-cgi/access/cli?token=<public key>&aud=<AUD>&redirect_url=...&send_org_token=true&edge_token_transfer=true`, and
  long-polls `https://login.cloudflareaccess.org/transfer/<public key>` (up to 10 tries) until an encrypted `{"app_token",
  "org_token"}` arrives. It stores both as plain files (mode 0600) under `~/.cloudflared`. **These endpoints appear only in
  cloudflared's source; no public documentation was found**, so Current must not rely on them. The documented user-side route is
  running `cloudflared access login <url>` and `cloudflared access token -app=<url>` (the CLI tutorial) and pasting the token.

## 5. Signing in with Google inside an app
- Google, 2016 (https://developers.googleblog.com/2016/08/modernizing-oauth-interactions-in-native-apps.html): "we will no longer allow
  OAuth requests to Google in embedded browsers known as 'web-views'", naming "the WebView UI element on Android and UIWebView/WKWebView
  on iOS, and equivalents on Windows and OS X".
- Google, 2021 (https://developers.googleblog.com/en/upcoming-security-changes-to-googles-oauth-20-authorization-endpoint-in-embedded-webviews/):
  "All embedded webviews will be blocked starting on September 30, 2021"; users "will see a `disallowed_useragent` error".
- Google's OAuth policies (https://developers.google.com/identity/protocols/oauth2/policies, last modified 2026-08-05), "Use secure
  browsers": "A developer must not direct a Google OAuth 2.0 authorization request to an embedded user-agent", and "Embedded
  user-agents include, but are not limited to, software libraries". The native-app guide (updated 2026-10-16) names WKWebView.
- **WKWebView:** blocked (named). **WebView2 and WebKitGTK:** covered by the wording; whether Google blocks them today is not verified.
  How Google detects web views is not documented; a WebKitGTK report (https://bugs.webkit.org/show_bug.cgi?id=240041) found that
  changing the user agent did not reliably help. Current must not disguise its web view anyway: that breaks Google's policy.
- **Cloudflare Access (or oauth2-proxy, Pomerium, authentik, Authelia) with Google as the identity provider:** the gateway sends the
  user to Google's own sign-in page under the gateway's OAuth client, so Google's rule applies to whatever shows that page. In an
  embedded web view the user ends at `disallowed_useragent` (inferred; no Cloudflare page on this was found).
- RFC 8252 (OAuth 2.0 for Native Apps, BCP 212): §8.12 "native apps MUST NOT use embedded user-agents to perform authorization
  requests", because without "the usual address bar and visible certificate validation features ... it [is] impossible for the user to
  know if they are signing in to the legitimate site", and the host app can "copy session cookies". §7.3: loopback redirects
  `http://127.0.0.1:{port}/{path}`; servers "MUST allow any port". §8.3: "the use of localhost is NOT RECOMMENDED".
- Note the RFC's concern is the app seeing the user's identity provider password. A gateway session cookie is exactly what Current
  needs and is scoped to the user's own server; the password risk remains and is why the origin must be shown (section 8).

## 6. The system browser
- **macOS: `ASWebAuthenticationSession`** (https://developer.apple.com/documentation/authenticationservices/aswebauthenticationsession,
  macOS 10.15+): "the service sends a callback URL to the session with an authentication token. The session passes this URL back to
  the app". In macOS "the system opens the user's default browser if it supports web authentication sessions, or Safari otherwise".
  Cookies are shared with the browser unless `prefersEphemeralWebBrowserSession` is set, but **they never reach the app**: only the
  callback URL does. Callbacks are a custom scheme, or https on an associated domain from macOS 14.4 (`Callback.https(host:path:)`, "The
  host must be a member of a domain associated with the app"), which a self-hosted user's gateway cannot be.
- **Windows:** `OAuth2Manager` (Windows App SDK) "always uses the system browser" but "is not available in the stable channel"
  (https://learn.microsoft.com/en-us/windows/apps/develop/security/oauth2, 2026-08-30); it returns OAuth codes and tokens, not gateway
  cookies. `WebAuthenticationBroker` is a UWP API; its support in an unpackaged WinUI 3 app is not verified.
- **So the system browser helps only where the gateway hands a token to an app callback:** Pomerium (documented, loopback), Authelia
  (only with an OIDC client the administrator sets up, and a Bearer collision), Cloudflare (only through cloudflared's undocumented
  transfer service). For every cookie gateway (Authelia, authentik, oauth2-proxy, Cloudflare's normal flow) the browser keeps the cookie.

## 7. Where the credentials must flow

### Requests from the core
`current-net`'s `RequestAuth` adds headers and query pairs on every request and every followed redirect (ADR 0006). Gateway
credentials are a second layer of the same kind, per address. reqwest 0.13.5 is built without its `cookies` feature (root
`Cargo.toml`), so cookies would be a `Cookie` header we build, plus our own reading of `Set-Cookie` if gateways refresh them (Authelia's
5-minute inactivity window is extended by use; oauth2-proxy can refresh), or the `cookies` feature (a new dependency).

### libmpv (Windows, Linux)
- Options (mpv v0.41.0 `DOCS/man/options.rst`): `--http-header-fields` ("Set custom HTTP fields when accessing HTTP stream"),
  `--cookies` ("Disabled by default"), `--cookies-file` ("Netscape format"), `--user-agent`.
- They become FFmpeg's `headers` and `cookies` options (`stream/stream_lavf.c`, `mp_setup_av_network_options`), and FFmpeg's HLS demuxer
  copies them to every playlist, segment and key request (`libavformat/hls.c` `ffio_copy_url_options`; `aviobuf.c` copies
  `"headers", "user_agent", "cookies", ...`) and updates cookies from responses.
- FFmpeg sends cookies only where they belong (`libavformat/http.c`: skips `secure` cookies on non-https and cookies whose domain does
  not match the host). **Custom headers go on every request, including after a redirect to another host** (`goto redo` keeps
  `s->headers`; read from code, not tested). So a gateway secret given as a header follows a stream redirect off the server.
- mpv master adds a libcurl backend that "honors the network options" (`DOCS/interface-changes/http-backend.txt`); not released.
- `user:pass@` in the URL becomes Basic after a 401 (`httpauth.c`) and is lost on redirect; the secret would sit in the URL string.

### AVPlayer (macOS, later iOS and tvOS)
- `AVURLAssetHTTPCookiesKey` (documented, macOS 10.15+): "By default, AVURLAsset only has access to cookies in the client's default
  cookie storage that apply to the asset's URL", and in HLS requests "to different paths or hosts ... won't contain any cookies that
  don't apply to the AVURLAsset URL".
- `AVURLAssetHTTPHeaderFieldsKey`: an Apple engineer, 2024 (https://developer.apple.com/forums/thread/20421): "is not a supported API,
  so you should not use it. To provide custom headers … you can use AVAssetResourceLoader".
- `AVAssetResourceLoaderDelegate` has `resourceLoader(_:shouldWaitForResponseTo:)` for authentication challenges (macOS 10.10+), but
  serves only custom URL schemes for loading (a forum report; not verified by Apple), and "Video AirPlay is not supported when using a
  custom resource loader" (2015 forum post quoting Apple support; not verified).

### Images
SwiftUI's `AsyncImage` "uses the shared URLSession instance", which carries the shared cookie storage but no custom headers. WinUI's
`BitmapImage` takes a URI or a stream (`SetSourceAsync`); no way to add headers to `UriSource` was found (not verified). Loading image
bytes in the core and handing the UI a stream works on every platform.

### AirPlay and casting
- AirPlay: an Apple engineer, 2022 (https://developer.apple.com/forums/thread/705408): "I'm not aware of any standard headers that
  every AirPlay receiver … guarantee to supply… supply your authentication token as a query parameter in the Multivariant Playlist
  (manifest) URL. That should carry across to the AirPlay receiver". Whether cookies from `AVURLAssetHTTPCookiesKey` reach a receiver
  is not verified.
- Chromecast's default receivers: "there is no way to implement authentication for the content using either of these receivers… You
  can't override the license URL, pass custom parameters, or authenticate users with cookies or tokens"
  (https://support.google.com/widevine/answer/6072130). A custom receiver can add headers (`cast.framework.NetworkRequestInfo.headers`).
- DLNA renderers fetch the URL themselves (no source found; not verified).
- So **a receiver gets only what is in the URL**: Jellyfin's `ApiKey`, not a gateway's cookie, Basic credentials or header. It can
  play only from an address it reaches without the gateway (often the LAN address).

## 8. Web views
- **WebView2:** `CoreWebView2CookieManager.GetCookiesAsync(uri)` "Gets a list of cookies matching the specific URI"; `CoreWebView2Cookie`
  has `IsHttpOnly`, so HttpOnly cookies are listed (inferred from the property; not stated). `BasicAuthenticationRequested` reports
  Basic, Digest, NTLM and proxy challenges with `Uri` and `Challenge`; without credentials from the host "WebView2 will show the default
  authentication challenge dialog". `NavigationStarting` and `SourceChanged` report navigations. Browser data lives in a user data
  folder.
- **WKWebView:** `WKHTTPCookieStore.getAllCookies` "Fetches all stored cookies asynchronously" (HttpOnly included per a 2018 WebKit
  change; not verified); `WKWebsiteDataStore.nonPersistent()` "stores website data in memory, and doesn't write that data to disk";
  without `webView(_:didReceive:completionHandler:)` the web view rejects authentication challenges, so no Basic dialog by default.
- **WebKitGTK 6** (2.54.1 docs): `webkit_cookie_manager_get_cookies` lists cookies for a URI; `webkit_network_session_new_ephemeral`
  keeps nothing on disk; the `authenticate` signal's default handler shows WebKit's dialog ("Authentication required by %s:%i", "The
  site says: ...", `WebKitAuthenticationDialog.cpp`).
- No platform web view shows an address bar: the app must show where the page is (RFC 8252 §8.12 above).

## 9. Native presentation
- **Windows:** WinUI's guidance: "Use dialogs to … request confirmation or additional info before an action can be completed", with
  `ContentDialog` (https://learn.microsoft.com/en-us/windows/apps/design/controls/dialogs-and-flyouts/dialogs).
  `CredUIPromptForWindowsCredentials` is the system dialog for Windows credential providers, not for a web server's password. Chromium's
  (and so WebView2's) own Basic prompt is titled "Sign in" with "`$1` requires a username and password" (`components/login_dialog_strings.grdp`),
  naming the origin and not the realm; Edge's exact text is not verified.
- **macOS:** HIG: "A sheet is useful for requesting specific information from people… In macOS… a sheet is always modal".
  `URLProtectionSpace` scopes a password to "host, port, protocol, and realm". `ASWebAuthenticationSession` shows "a modal view telling
  them which domain the app is authenticating with". Safari's sheet text is not verified.
- **GNOME:** WebKitGTK's and Epiphany's dialog (above): host and port, the realm quoted as "The site says", "Remember password".

## 10. Not researched
- Client certificates (mutual TLS: Cloudflare Access, NPM and others can require them). Deferred: rustls-platform-verifier covers server
  certificates only; client certificates from the Windows certificate store or the macOS keychain need their own spike.
- Digest, NTLM and Negotiate challenges.
- Seerr and Stash behind a gateway (M4): the per-address gateway layer would apply unchanged. Seerr's API key travels as `X-API-Key`
  (ADR 0014's notes), so it does not collide with a gateway's `Authorization`; Stash's API key header was not checked.
