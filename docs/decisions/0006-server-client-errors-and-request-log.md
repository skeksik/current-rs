# 0006: Server client, errors and request log

Status: accepted (2026-10-22).

## Context
`current-net` is the one HTTP client every service crate builds on. Its errors reach the UI, and its log is meant to be pasted into bug
reports in a public repo, so neither may carry a server's address or a secret.

## Decision
- **One client per configured server** (`ServerClient`), named by its `ServerRef`. Its base address comes from an `AddressProvider`
  asked before every request; `FixedAddress` is the only implementation. Roaming between addresses is M1 and needs its own research:
  only the seam exists.
- **Credentials through a hook.** Service crates implement `RequestAuth`, which adds headers and query pairs. The log entry is built
  from the request as the caller made it, before the hook runs and before the address is joined, so neither credentials nor the host
  can reach it (tested).
- **Every request has a timeout:** the client's (default 30 s for the whole request, 10 s to connect) unless the request sets its own.
- **Redirects stay on the server, with their credentials.** reqwest follows none (`Policy::none()`); `ServerClient` follows them
  itself: at most 5 hops, each to the original request's host, on the same scheme and port or as an http to https upgrade (reverse
  proxies commonly redirect to https or add a trailing slash). `RequestAuth` runs again on every hop, on fresh copies of the caller's
  headers and the new location's query, so header and query credentials survive a followed redirect (reqwest's own following drops
  `Authorization` on a scheme change and loses query credentials: reqwest `src/redirect.rs`, `remove_sensitive_headers`). 301 and 302
  turn anything but GET and HEAD into a GET without body, 303 always does (except HEAD); 307 and 308 keep method and body, as browsers
  do. One timeout covers all hops. Any other redirect (another host or port, https to http, too many hops) stops with
  `NetError::Redirected`, whose `Location` lets server setup show where the server points; its Debug output hides the address.
  Credentials therefore never reach another host (tested).
- **Errors (`NetError`, thiserror), by what the UI does:** `Unreachable` (offline, DNS, refused or dropped connection), `Timeout`,
  `Tls`, `Auth` (401/403), `Status` (any other non-2xx), `Redirected`, `Decode`, `InvalidRequest` (a path or body the calling crate got wrong; no
  server involved). `Auth` and `Status` carry the server's message (from a JSON `message`, `detail`, `title` or `error` field, or a
  short text body, cut at 300 characters) for the UI only; `NetError`'s Debug output leaves it out, so logging an error with
  `tracing::debug!(?error)` is safe. No variant carries a URL, header or library error text: transport errors are
  sorted by kind after `reqwest::Error::without_url()`, and `Decode` keeps only line and column because a decoder's message can quote
  the response. Tests check no error's Display or Debug contains the stand-in server's address.
- **Request log:** a bounded ring buffer (default 200 entries) shared by all clients, with time, `ServerRef` (in place of the host),
  method, path, query, status or error kind, and duration. Query values are redacted deny-by-default: keys stay, every value becomes
  `[redacted]` except a short allowlist of paging, sorting and field-selection keys (`limit`, `startIndex`, `sortBy`, `fields`, ...),
  because search terms can hold library titles. `RequestLog::to_text()` gives the copyable text. The server's error message is never
  logged. The same line goes to `tracing` at debug level.
- **Rule for service crates:** paths are logged as given, so a path must never embed a secret (tokens and keys go through
  `RequestAuth` as headers or query pairs). Ids in paths are fine: they are opaque.
- **TLS:** reqwest 0.13 with rustls and rustls-platform-verifier, which checks certificates with the OS's own verifier and store.
- **Crypto provider: ring** (owner, 2026-10-22), not reqwest's default aws-lc-rs. ring builds with a C compiler alone; aws-lc-sys
  builds a much larger C library and is heavier to cross-compile for Android and iOS (build requirements per target not verified).
  Both are permissively licensed. reqwest's `rustls` feature hard-wires aws-lc-rs, so `current-net` uses `rustls-no-provider` and
  depends on rustls directly (default features off; `ring`, `std`, `tls12`). With that feature reqwest takes rustls's process-wide
  default provider and panics at client build if none is set, so `ServerClient::new` installs ring once (guarded by `Once`; an
  already installed provider is kept). **The provider is process-wide:** anything else in the process using rustls's default gets ring
  too.
- **Proxies: the OS's settings** (owner, 2026-10-22), as native apps do. reqwest always reads the `HTTP_PROXY`, `HTTPS_PROXY` and
  `NO_PROXY` environment variables; its `system-proxy` feature adds the Windows and macOS system settings. It adds
  `system-configuration`, `system-configuration-sys`, `core-foundation` 0.9 (macOS) and `windows-registry`, `windows-result`,
  `windows-strings` (Windows), all MIT OR Apache-2.0.

## Consequences
- A UI can map each error to one message and action (sign in again, retry, check the connection) without parsing text.
- Diagnostics are safe to paste, at the cost of hiding query values that would sometimes help debugging.
- A server that redirects to another host (for example a login portal in front of it) fails with `Redirected` until the user enters
  the address it points to.

## Sources (read 2026-10-22)
- reqwest 0.13.5 source, crates.io: `Cargo.toml` features (`rustls` enables aws-lc-rs and rustls-platform-verifier;
  `rustls-no-provider` only the verifier), `src/async_impl/client.rs` (`CryptoProvider::get_default()`, else a panic without
  aws-lc-rs), `src/error.rs` (`without_url`), `src/redirect.rs` (custom policies), `src/lib.rs` (environment proxies are always read;
  `system-proxy` uses "Windows and macOS system proxy settings").
- hyper-util 0.1.21 `Cargo.toml`: `client-proxy-system` pulls `system-configuration` and `windows-registry`.
- Licences from each crate's `Cargo.toml`: ring 0.17.14 "Apache-2.0 AND ISC"; rustls 0.23.46 "Apache-2.0 OR ISC OR MIT"; aws-lc-sys
  0.46.0 ISC, Apache-2.0, MIT and BSD-3-Clause terms.
