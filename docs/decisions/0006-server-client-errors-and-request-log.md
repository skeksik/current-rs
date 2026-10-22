# 0006: Server client, errors and request log

Status: accepted (2026-10-22). Crypto provider: see below.

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
- **No redirects are followed:** a 3xx is reported as a status error, so credential headers never travel to another host.
- **Errors (`NetError`, thiserror), by what the UI does:** `Unreachable` (offline, DNS, refused or dropped connection), `Timeout`,
  `Tls`, `Auth` (401/403), `Status` (any other non-2xx), `Decode`, `InvalidRequest` (a path or body the calling crate got wrong; no
  server involved). `Auth` and `Status` carry the server's message (from a JSON `message`, `detail`, `title` or `error` field, or a
  short text body, cut at 300 characters) for the UI only. No variant carries a URL, header or library error text: transport errors are
  sorted by kind after `reqwest::Error::without_url()`, and `Decode` keeps only line and column because a decoder's message can quote
  the response. Tests check no error's Display or Debug contains the stand-in server's address.
- **Request log:** a bounded ring buffer (default 200 entries) shared by all clients, with time, `ServerRef` (in place of the host),
  method, path, query, status or error kind, and duration. Query values are redacted deny-by-default: keys stay, every value becomes
  `[redacted]` except a short allowlist of paging, sorting and field-selection keys (`limit`, `startIndex`, `sortBy`, `fields`, ...),
  because search terms can hold library titles. `RequestLog::to_text()` gives the copyable text. The server's error message is never
  logged. The same line goes to `tracing` at debug level.
- **Rule for service crates:** paths are logged as given, so a path must never embed a secret (tokens and keys go through
  `RequestAuth` as headers or query pairs). Ids in paths are fine: they are opaque.
- **TLS:** reqwest 0.13 with rustls and rustls-platform-verifier (the OS's certificate store). Crypto provider: **pending the owner's
  decision** (ring through `rustls-no-provider` and a process-wide `install_default`, or reqwest's default aws-lc-rs); recorded here
  with sources when decided.

## Consequences
- A UI can map each error to one message and action (sign in again, retry, check the connection) without parsing text.
- Diagnostics are safe to paste, at the cost of hiding query values that would sometimes help debugging.
- A server behind an http-to-https redirect must be configured with its https address.

## Sources (read 2026-10-22)
- reqwest 0.13.5 source, crates.io: `Cargo.toml` features (`rustls` enables aws-lc-rs and rustls-platform-verifier;
  `rustls-no-provider` only the verifier), `src/async_impl/client.rs` (`CryptoProvider::get_default()` before reqwest's own default),
  `src/error.rs` (`without_url`), `src/lib.rs` (environment proxies are always read; `system-proxy` adds Windows and macOS settings).
