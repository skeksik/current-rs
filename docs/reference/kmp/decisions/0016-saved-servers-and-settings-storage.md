# 0016: Saved servers, settings storage and address checks

Status: accepted (2026-10-20), with server setup for Jellyfin.

## Context
Server setup needs to keep what the user added across restarts, on every platform, before any database exists (ADR 0004 is still open
for the web). A server can have several addresses, local and internet (roadmap, M1), and credentials must only ever go to the server the
user added, never to whatever answers at an address on another network. docs/platform-notes.md has the research: Jellyfin and Seerr
report a stable id without credentials, Stash does not.

## Decision
- **`KeyValueStore`** (`core:database`): strings only, synchronous, one implementation per platform, never for secrets.
  - Desktop: one JSON file, kept in memory and replaced atomically on every write (temporary file, then move). `%APPDATA%\Current\settings.json`
    on Windows, `$XDG_CONFIG_HOME/current/settings.json` (default `~/.config/current`) on Linux. Chosen over `java.util.prefs`, which
    writes to a registry key shared by all Java apps on Windows and, on Linux, to a hidden folder outside the XDG layout, saved on a timer.
    The app needs its own folders for cache and logs anyway. An unreadable file is kept as `settings.json.unreadable` and the app starts empty.
  - Web: `localStorage` (prefix `current.`), plain text readable by any script on the origin. Android: `SharedPreferences`. Apple: `NSUserDefaults`.
- **`SavedServer(ref, serverId, name, addresses)`** (`core:model`): `serverId` is the id the server reported when it was added; addresses
  are in the user's priority order, each `Local` or `Internet`. `ServerRepository` (`core:network`) keeps one entry per server plus an
  order index, and puts every saved server into the capabilities state at start (signed out until sign-in).
- **Address scope** is decided from the host alone: private IPv4 ranges, `100.64.0.0/10` (Tailscale), loopback, link-local, IPv6 unique
  local and link-local, single-label names, and `.local`, `.lan`, `.home.arpa`, `.localhost`, `.internal`, `.ts.net` are `Local`.
  Plain HTTP on a `Local` address is accepted; on an `Internet` address the user is warned and confirms.
- **Address checks:** a `ServerProbe` per service identifies an address without credentials (Jellyfin: `GET /System/Info/Public`, its
  `Id`). Typed text without a scheme becomes candidates: `https`, then `http`, then `http` on the service's default port (8096). All are
  asked at once; the first in that order that answers wins. A new server whose id is already saved is refused; an address added to a saved
  server must report that server's id. Redirects are reported, not followed (ADR 0015).
- **Stash** (later) cannot be identified without its key, so its key goes only to addresses the user typed or confirmed.

## Consequences
- Server setup works before any screen: `ServerSetupViewModel` (`feature:settings`) is ready for the Compose and React shells.
- Moving or renaming a server keeps its key; changing its data directory (a container without a volume) changes its id, and the app then
  treats it as a different server, which is the safe side.
- The web store is per origin and per browser profile; a user who clears site data loses the saved servers.
- Not decided here: roaming between addresses (a `ServerAddressSource` that probes, ADR 0015), discovery, and where tokens live (sign-in).
