# 0017: Secrets and device identity

Status: accepted (2026-10-20), before Jellyfin sign-in.

## Context
Sign-in (M1) must keep a token per server so the app restarts signed in, without writing it where other users, other apps' data or a
copied disk expose it. Jellyfin also identifies each install by a device id: signing in again with the same id revokes the previous token,
so two machines must never share one. `%APPDATA%` roams with domain profiles, and phone backups restore app data onto new devices.

## Decision
- **`SecretStore`** (`core:database`) for tokens and keys, with `storage` telling the UI where they are:
  - **Windows: Credential Manager.** Generic credentials named `Current/<key>`, persisted on this machine only
    (`CRED_PERSIST_LOCAL_MACHINE`), at most 2560 bytes, visible and removable by the user in Control Panel. Called through JNA
    (`CredWriteW`, `CredReadW`, `CredDeleteW`); `jna-platform` has no binding for them.
  - **Linux: the Secret Service** (GNOME Keyring, KWallet) through the system's `libsecret` via JNA, in the default collection, schema
    `com.skeksik.current.Secret`. The app does not ship `libsecret`.
  - **No keyring** (no `libsecret`, or no Secret Service on the session bus): memory, so the user signs in after every start. The UI may
    offer a file only the user can read (`rw-------` in `$XDG_STATE_HOME/current/secrets.json`); choosing it is remembered and moves what
    memory holds. Never written without that choice.
  - **A keyring that fails at run time** (locked and dismissed): memory for the rest of the run.
  - **Web: `localStorage`** under `current.secret.`. It survives restarts, which staying signed in needs, and any script on the origin can
    read it. The web app must keep foreign scripts out with a strict Content-Security-Policy: the production build carries it as
    a `<meta>` tag, and a host sends it as a header (docs/web-hosting.md). The tabs of one browser share it: each follows the others'
    writes through the `storage` event, adopting a session another tab started and dropping one it ended, and never writes a token
    on that path.
  - **Android and Apple: memory until M4**, which brings a Keystore-backed store and the Keychain (this device only).
- **Machine-local store** for values that must not roam: `%LOCALAPPDATA%\Current\local.json` on Windows, `$XDG_STATE_HOME/current/local.json`
  (default `~/.local/state/current`) on Linux. Elsewhere it is the settings store's kind under its own name; on Android the backup rules
  keep it out of backups (PR 46).
- **`DeviceIdentity`**: a random 32-hex device id made once per install in the machine-local store, and a device name for server
  dashboards (the computer's name on desktop, "Chrome on Windows" on the web).
- **`AppInfo.VERSION`** is generated from `current.version` in `gradle.properties`, for the `Version` servers ask for.
- **JNA 5.19.1** (`jna`, `jna-platform`; LGPL-2.1-or-later or Apache-2.0, allowed by ADR 0009) is a dependency of `core:database` on the JVM.

## Alternatives considered
Common practice among widely used apps and developer tools (docs/platform-notes.md, "Credential storage in other apps"):
- **Windows, DPAPI-encrypted file:** what app frameworks commonly do (`jna-platform` has `Crypt32Util`). Equally safe against other users
  and a copied disk, but invisible to the user. Credential Manager is what credential helpers commonly default to; chosen for visibility.
- **Linux, a pure-Java D-Bus client** (`secret-service` with `dbus-java`, both MIT): no native library, but two more dependencies whose
  recent releases are alphas or JDK 17 only builds.
- **Linux without a keyring:** others fall back silently to a plain-text file, or encrypt with a key hard-coded in the app (no real
  protection), or have no default store. An explicit, remembered choice is stricter than the first two and friendlier than the third.
- **Web, a WebCrypto key in IndexedDB wrapping the token:** harder to copy off disk, but any script on the page can still decrypt it;
  `sessionStorage` signs out with the tab.
- **Java's FFM API instead of JNA:** no dependency, but final only from JDK 22; the build runs on JDK 21. Revisit with the libmpv binding (M2).

## Consequences
- A Windows uninstall leaves `Current/…` credentials unless the app removed them on sign-out; sign-out and removing a server must delete them.
- Over SSH sessions Credential Manager does not persist (a known limit); the desktop app is not used that way.
- The Secret Service may ask the user to unlock a keyring; calls are `suspend` and run off the main thread.
- Tests run the real Credential Manager on Windows and the real Secret Service in CI's Linux job (GNOME Keyring in a D-Bus session).

## Addendum (2026-10-21): a keyring failure is retried once
- A keyring call that fails is tried once more at once, so one passing failure does not drop the run to memory. A call that timed out
  (the Secret Service after 2 minutes) is not retried, since the worker may stay blocked; nor is a value too long for Credential Manager.
- If the retry fails too: memory for the rest of the run, as before, so a user who dismissed an unlock prompt is not asked again at every
  sign-in. Removals still try the keyring. The next start uses the keyring again. Nothing goes to a file without the user's choice.
