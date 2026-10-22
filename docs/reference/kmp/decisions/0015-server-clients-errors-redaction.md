# 0015: Server clients, error model and log redaction

Status: accepted (2026-10-20), with the first `core:network` clients.

## Context
Every data layer talks to servers through `core:network`. A server can have several addresses, local and internet, and the app should
move between them as the network changes (roadmap, M1 server setup), so a client cannot fix its base URL when it is made. Credentials
travel in many places: Jellyfin's `MediaBrowser` header and its `ApiKey` query parameter on media URLs, Stash's admin key in every stream
URL, Quick Connect secrets in the query. Logs and problem reports must never carry them, nor where the user's servers run.

## Decision
- **One client per configured server**, made by `ServerClients.create(server, address, auth)` from the app's one `HttpClient`, so every
  server shares the platform engine. Paths are relative to the server (`get("System/Info/Public")`).
- **The address is swappable.** A `ServerAddressSource` holds the current `ServerUrl` (http or https, host, port, optional path prefix);
  the client reads it for every request and reports addresses it cannot connect to. `FixedServerAddress` has one address; choosing among
  several came with roaming (ADR 0019).
- **Credentials are the data layer's.** `RequestAuth` adds them to each request after the address is applied; core knows no scheme.
- **Redirects are not followed and only 2xx succeeds**, so credentials never go to a host the user did not configure.
- **Errors:** every call goes through `ServerClient.request`, which throws `ServiceException(server, kind, status, detail)` with a kind:
  `Unreachable`, `Timeout`, `Unauthorized`, `Forbidden`, `NotFound`, `Rejected` (other 4xx), `ServerError` or `InvalidResponse` (a body
  that does not decode, or a redirect). Cancellation and exceptions that are not request failures (bugs) pass through unchanged.
  Repositories throw; ViewModels turn exceptions into UI state, so none reaches Swift or JavaScript. The message names only the server's
  opaque key, and the original exception is not kept as the cause: Ktor's carry the full request URL, and decoding errors quote the body.
  Service-specific error bodies (Seerr's messages) are parsed in the data layer when a feature needs them.
- **Lenient JSON** (`ignoreUnknownKeys`, `coerceInputValues`, `explicitNulls = false`).
- **Our own request log, not `ktor-client-logging`.** A small client plugin hands a `NetworkLogger` one entry per request: method, path and
  query with secret values redacted, status and duration, under the server's key instead of its host. It never sees headers or bodies.
  `ktor-client-logging` 3.6.0 can mask header values (`sanitizeHeader`) but writes every URL as is, query secrets included, and its
  body levels would log passwords and tokens; redacting its text afterwards would be fragile. `Redaction` is public for other logs, such as
  stream URLs handed to an engine.

## Consequences
- Roaming between addresses needs no new clients: a smarter `ServerAddressSource` replaces `FixedServerAddress`.
- Every data layer call is wrapped in `request { }`; code outside it gets raw Ktor exceptions.
- No request or response bodies in logs, even in debug builds. If debugging needs them, that is a separate, opt-in tool.
- Mapping connection failures relies on them being `IOException`s. That is checked on the JVM engine only; the Darwin, Android and JS
  engines are listed in docs/platform-notes.md as not verified.

## Addendum (2026-10-21): where the log goes
- The app binds `RequestLogBuffer` as the `NetworkLogger`: the last 200 lines in memory, nothing on disk. It also serves as
  `RequestLogHistory` for Settings > About's "Copy diagnostics" (`AboutViewModel`), which copies the app version, the platform and those
  lines. Nothing is uploaded.
- A line is `<service>/<server key> METHOD <scheme>://…<path and query, redacted> <status, "no response" or "cancelled"> (<ms> ms)`.
  The scheme (http or https) tells apart the probes of one address; the host and port are never kept.
- Bugs that end background work (`NetworkLogger.bug`) go to the same log, as a line with the class name only.
- Printing as it happens is off by default: logcat (tag `CurrentNet`) in an Android build that is debuggable, stderr on desktop with
  `-Dcurrent.log=debug` or `CURRENT_LOG=debug`, `console.debug` on the web with `localStorage['current.log'] = 'debug'` (then reload).
  Apple prints nothing yet; the unified log comes with the Apple app.
- Paths keep their route, with secret query values hidden and id-like path segments and query values masked as `…` (2026-10-21):
  GUIDs (32 hex digits, with or without dashes, as Jellyfin writes user, item and session ids) and any other run of 16 or more hex
  digits. A line reads `Users/…/Items/…?ParentId=…&Fields=Overview`. Short numbers (Seerr's movie ids) and words stay. `Redaction.url`
  masks the same way; it and `pathAndQuery` are for logs only, never for the URL a request goes to.
