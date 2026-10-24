# 0013: Settings, secrets and the device identity

Status: proposed (2026-10-22), from M1; accepted when its PR merges. Dependencies approved by the owner (2026-10-22).

## Context
The core keeps settings, sign-in secrets and a device id between runs (ADR 0002: `current-store`). Each platform puts app data in its
own place, and some of them only the UI can know (MSIX, the macOS sandbox, Flatpak, Android). Jellyfin ties sessions and tokens to
the client's `DeviceId`. Sources were read on 2026-10-22 (below).

## Decision

### The UI passes the data folder
`App::open(data_dir, secrets)` takes the folder at start. The core never looks it up: the UI knows how it was packaged, and a
directory-lookup crate could not cover Android or tvOS anyway.

| Platform | Folder the UI passes |
|---|---|
| Windows, unpackaged | `%LOCALAPPDATA%\Current` (`FOLDERID_LocalAppData`, ADR 0009) |
| Windows, MSIX (later) | The same path works: new AppData files are redirected to a per-package location and "merged at runtime to appear in the real AppData location", and removed on uninstall. `ApplicationData.Current.LocalFolder` is the explicit alternative (ADR 0009) |
| macOS | `~/Library/Application Support/<bundle id>` from `FileManager`; in the sandbox it resolves inside the app's container (not verified against a quote) |
| Linux (later) | `$XDG_CONFIG_HOME/<app>` (default `~/.config`); Flatpak sets it to `~/.var/app/<app-id>/config` |
| iOS (later) | Application Support in the app's container (not verified) |
| tvOS (later) | Open: only 500 KB of persistent local storage, through `NSUserDefaults`; anything else is purgeable. Decided by the tvOS task |
| Android (later) | `Context.getFilesDir()` (not verified) |

### The settings file
- `settings.json` in the data folder: a JSON object with `"version"` (the format, now 1) and the settings' fields (now `device_id`;
  `servers` came with ADR 0015, and `last_server_key`, the highest server key ever given out, with removing a server on
  2026-10-23, both defaulted; `app_theme` on 2026-10-24: `"light"`, `"dark"` or `"system"`, left out while it is the default,
  system; a value this version does not know, of any JSON type, reads as system and is written back unchanged until the theme is
  changed here, so it never makes the file corrupt).
- **Address ids (ADR 0017, 2026-10-24):** every saved address has an `id`, a number that stays with the address when the addresses are
  reordered or its URL changes and is never given to another address of the same server: each server keeps `last_address_id`, the
  highest id it gave out. Ids are given when an address is saved; an address read without one (older files) or with an id an earlier
  address of the server already has gets the next one, and the file is saved. An address's `gateway` field lists the kinds of gateway
  credential it has (`"basic"`, `"cookies"`, `"headers"`; open, so an unknown kind is kept), never a value. All three fields are
  defaulted and left out while empty, so no format change.
- **Adding a field** gives it `#[serde(default)]`: older files read without a migration. Fields this version does not know are kept
  and written back, so an older build never drops what a newer one wrote. The server list (several addresses per server, users with a
  reference to their secret) arrives this way in the server-setup and sign-in tasks.
- **A format change** raises the version and adds one step to `MIGRATIONS`. Steps run in order from the file's format, then the file
  is saved in the new format. The step machinery is tested with pretend formats 2 and 3.
- **Atomic save:** write `settings.json.tmp` in the same folder, `sync_all`, rename it over `settings.json`. `std::fs::rename` is
  "MoveFileExW with a fallback to SetFileInformationByHandle on Windows" and "On Windows 10 version 1607 and above, the behavior is the
  same as Unix if the filesystem supports FileRenameInfoEx" (Rust std docs). `ReplaceFileW`, which also carries the old file's ACLs
  and streams over, would need `unsafe` and adds nothing for a file only we write. A rename that fails with "access denied" (another
  process has the file open for a moment; not verified as the cause) is retried 4 times, waiting 10 to 40 ms, at most 100 ms in all.
- **A corrupt file** (not JSON, not an object, no valid version, a known field of the wrong type) is renamed to
  `settings.corrupt-<unix seconds>.json` and the settings start from defaults. `LoadReport::ResetCorrupt` lets the UI say so. If the
  backup rename fails, opening fails: the file is never overwritten in place. Resetting also makes a new device id, even when only
  another field was wrong, so each server sees a new device and the old one stays in its device list until an admin removes it.
  That is acceptable for a corrupt file: it is rare, and picking fields out of a file we cannot trust is not worth the code.
- **A newer file** (a downgrade) is read for the fields this version knows and never written. Saves fail with `NewerVersion`, so
  the newer version finds its file unchanged.
- **One writer:** one `SettingsStore` per process (one `App`), with saves serialized by its lock. On Windows, ADR 0009's single
  instance keeps it to one process. On macOS and Linux, single instance is the shells' job.
- Errors and `Debug` output carry no paths: the data folder's path holds the user's account name.

### Secrets in the OS keyring
- **ADR 0002's "keyring 4" is now keyring-core plus a store crate per platform.** keyring 4.2.0's README: "developers should take
  dependencies on the keyring-core crate and the specific credential stores they want to use". keyring-core lets us build entries
  from a store object we hold, with no global default store.
- **Backends:**

| Platform | Crate | Store |
|---|---|---|
| Windows | `windows-native-keyring-store` 1.1.0, without its `search` feature (drops `regex`) | Credential Manager, generic credentials |
| macOS | `apple-native-keyring-store` 1.0.2, `keychain` feature | The login keychain |
| Linux, iOS, tvOS, Android | none yet | Every call returns `Unavailable` |

  - Linux is deferred to its app task (M5). ADR 0011 left the choice to M1 or M5: the Secret Service (`zbus-secret-service-keyring-store`,
    about 55 more crates, which in Flatpak needs access to `org.freedesktop.secrets`) or the Secret portal plus our own encrypted store. secret-service 5.2.0 has no portal support (its source, searched).
  - iOS needs the `protected` (Data Protection) keychain, which requires a provisioning profile (the store's README); later.
- **Naming:** service `Current`; account `<service type>/<server key>/<user id>`, e.g. `jellyfin/1/5d1c…`. The user part is the
  server's id for the user (`UserId`), never a user name, and the server key is ours, never a hostname. So the OS keyring viewer shows
  nothing private, and any number of servers and users fit. On Windows the credential's target name is `<account>.Current` (the store's
  default `{user}.{service}`).
- **Windows persistence `Local`:** "The credential persists for all subsequent logon sessions on this same computer. It is visible to
  other logon sessions of this same user on this same computer and not visible to logon sessions for this user on other computers"
  (`CRED_PERSIST_LOCAL_MACHINE`). The store's default, `Enterprise`, roams with a domain profile; a token belongs to this install's
  device id.
- **Limits:** `CredentialBlobSize` "cannot be larger than CRED_MAX_CREDENTIAL_BLOB_SIZE (5*512) bytes"; larger secrets fail with
  `TooLong`. Tokens and API keys are far smaller. The Windows store serializes Credential Manager calls with a named mutex, because
  Credential Manager "loses updates when credentials, even different ones, are written and deleted concurrently" (its README).
- **Gateway secrets (ADR 0017, 2026-10-24):** gateway secrets under `gateway/<server key>/<address id>`, stored in chunks when larger
  than a Windows credential. The chunks are numbered, `gateway/<server key>/<address id>/0`, `.../1`, ..., and written and deleted
  together; a secret that fits one credential is one chunk, so every gateway secret has the same format. Windows' store writes a
  password as UTF-16 (windows-native-keyring-store 1.1.0, `validate_password`, read 2026-10-24), so a chunk holds at most 1,280 UTF-16
  units. Chunk 0 starts with the number of chunks and the secret's length; a write deletes the earlier chunk 0 first and writes the
  new one last, so a write cut short leaves no secret, never a mix of two; a read that finds a chunk missing, one too many or a wrong
  length treats the secret as absent and deletes every chunk. Removing an address or a server deletes its gateway secrets after the
  settings are saved, as for tokens.
- **No keyring** (no backend on the platform, a locked or refused keyring): calls return `Unavailable`. Sign-in works for the run
  but is not remembered, and the UI says so.
- **No fallback to the settings file.** A token in a plain file can be read by any process of the user and travels into backups,
  sync folders and support zips. A fallback would also make "remembered" mean different security on different machines without the
  user knowing.
- **Secret values never reach logs:** `Secret` has a redacted `Debug` and no `Display`, and `SecretsError` carries no platform text
  (keyring-core's `BadEncoding` holds the stored bytes).
- The API is sync. Keyring calls can block or show a system prompt, so callers run them off the async runtime.

### The device identity
- Jellyfin reads `DeviceId` and `Device` from `Authorization: MediaBrowser Client=…, Device=…, DeviceId=…, Version=…, Token=…`
  (`AuthorizationContext.cs`). At sign-in it logs out every token for the same device id and user ("Logout any existing sessions
  for the user on this device", `SessionManager.GetAuthorizationToken`), then creates one device record holding the new token.
  Sessions are keyed by app, device id and user. Quick Connect records the requesting device id and authorizes that device
  (`QuickConnectManager`).
- So the id must be **stable per install** and **different between installs**. A changed id leaves the old device and token in the
  server's device list and adds a new one. Two installs sharing an id would sign each other out.
- **Ours:** 128 bits from the OS random number generator (`getrandom`), written as 32 lowercase hex digits. It is made on first start
  and stored in `settings.json`, never derived from hardware. A stored id must be 1 to 64 ASCII letters, digits or hyphens (safe
  in a quoted header value); one that is missing or malformed is replaced.
- It **survives upgrades and reinstalls** of the direct download: the uninstaller never removes the data folder (ADR 0009). A
  future "remove my data" action makes a new one. An MSIX uninstall removes it with the package's data, the platform norm.

### Where the work runs
`current-store` is a sync API with blocking file and keyring I/O. Following ADR 0002's portability rule, callers on an async path
schedule it on a blocking thread (`spawn_blocking` in `current-ffi`); the store holds no runtime.

## Dependencies (owner, 2026-10-22; all MIT OR Apache-2.0, no new licence)
- `keyring-core` =1.0.1, no features (with `log`)
- `windows-native-keyring-store` =1.1.0, `default-features = false` (Windows only; `byteorder`, `zeroize`, `windows-sys`)
- `apple-native-keyring-store` =1.0.2, `keychain` (macOS only; `security-framework`, already in the tree)
- `getrandom` =0.4.3 (already in the tree through `tempfile`)
- `tempfile` =3.27.0, tests only (already in the tree through UniFFI)
- Not taken: the `keyring` 4.2.0 wrapper (pulls the zbus Secret Service store by default), `dbus-secret-service-keyring-store` (links
  the C libdbus), `linux-keyutils-keyring-store` (the kernel keyring is gone after a reboot), `dirs`/`directories` (the UI passes
  the folder), `uuid` (not needed for a random id).

## Consequences
- Windows and macOS remember sign-ins. Other platforms don't until their backend is added.
- The server-setup and sign-in tasks add fields to `Settings` and use `SecretKey`, with no migration.
- The keyring round trip runs for real on the Windows and macOS CI runners, under a random user id that a guard deletes. On Linux,
  a test checks that the keyring reports `Unavailable`.

## Sources (read 2026-10-22)
- MSIX AppData redirection: https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes
- Apple, File System Programming Guide, macOS Library directories ("By convention, all of these items should be put in a subdirectory
  whose name matches the bundle identifier"; "Your app is responsible for creating this directory as needed"):
  https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html
- Apple TV Programming Guide ("your app can only access 500 KB of persistent storage that is local to the device (using the
  NSUserDefaults class)"): https://developer.apple.com/library/archive/documentation/General/Conceptual/AppleTV_PG/index.html
- XDG Base Directory Specification: https://specifications.freedesktop.org/basedir-spec/latest/
- Flatpak conventions (XDG directories under `~/.var/app/<app-id>`): https://docs.flatpak.org/en/latest/conventions.html
- Rust `std::fs::rename`: https://doc.rust-lang.org/std/fs/fn.rename.html
- `CREDENTIALW` (blob size, persistence): https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw
- keyring 4.2.0 README: https://github.com/open-source-cooperative/keyring-rs; keyring-core 1.0.1:
  https://github.com/open-source-cooperative/keyring-core; store READMEs and sources (windows-native-keyring-store 1.1.0,
  apple-native-keyring-store 1.0.2, zbus-secret-service-keyring-store 1.0.1, secret-service 5.2.0) on crates.io
- Jellyfin server, `master` at c7ce6a1 (the latest tag is v12.2): `Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`,
  `Emby.Server.Implementations/Session/SessionManager.cs`, `Emby.Server.Implementations/QuickConnect/QuickConnectManager.cs`:
  https://github.com/jellyfin/jellyfin
