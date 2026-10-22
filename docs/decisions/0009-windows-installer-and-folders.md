# 0009: Windows installer and folders: Inno Setup 7, per user, data kept apart

Status: proposed (2026-10-22), from D5b; accepted when its PR merges. Installer tool chosen by the owner (2026-10-22). Details and
sources: `docs/notes/windows-shell.md`.

## Context
The Windows app is unpackaged and self-contained (ADR 0007, `apps/windows`). It needs an installer for direct downloads that installs
per user without admin rights, upgrades in place and uninstalls cleanly, and must never delete the user's data. The candidates were
Inno Setup, Velopack, WiX and a hand-built MSI (comparison below). A Microsoft Store (MSIX) build may come at release time.

## Decision
- **Inno Setup 7** (7.1.0, owner, 2026-10-22; pinned by version and SHA-256 in CI) builds a 64-bit installer
  (`SetupArchitecture=x64`), `Current-<version>-x64-setup.exe`, from `apps/windows/installer/Current.iss`.
  - `PrivilegesRequired=lowest`: no elevation, always a per-user install.
  - A fixed `AppId`: a newer setup upgrades in place.
  - One Start menu shortcut that carries the app's AppUserModelID `Skeksik.Current`.
  - `AppMutex=Skeksik.Current.Running`, a mutex the app holds while it runs, so Setup and Uninstall detect a running Current;
    `[UninstallDelete] Type: dirifempty` for the program folder.
  - It ships its uninstaller (`unins000.exe`, `unins000.dat`) into the program folder.
- **Folders**:
  - Program: `%LOCALAPPDATA%\Programs\Current` (`FOLDERID_UserProgramFiles`, Inno's `{autopf}` in non-admin mode).
  - Data: `%LOCALAPPDATA%\Current` (`FOLDERID_LocalAppData`).
  - The two are separate trees, and the installer never names the data folder, so neither upgrade nor uninstall can delete it. CI
    checks this with a marker file.
- **Store-ready:** the app does not depend on how it was installed:
  - no install paths are hard-coded; folders come from the known-folder API, or `ApplicationData.Current.LocalFolder` with package
    identity;
  - the AppUserModelID is set only without package identity;
  - single instance goes through `AppInstance`, which works in both modes.
- **Updates:** none yet. A later self-update for direct downloads goes in the core: it downloads the newer setup and runs it silently;
  Inno's `AppId` makes that an upgrade.
- **Not in this decision:** code signing. Public releases need it, for SmartScreen and the Store's installer route.

## What an MSIX (Store) build would change
From the owner's list (2026-10-22); not verified against Microsoft's documentation in this task.
- Windows removes the package's data on uninstall: the platform norm for packaged apps, unlike the direct download.
- Updates come from the Store; the app must not self-update.
- The Store signs the package; a sideloaded MSIX needs our own signature.
- The Store's content policies apply to what the app can show (relevant for M6).
- The identity (AUMID) comes from the package manifest instead of `SetCurrentProcessExplicitAppUserModelID` and the shortcut.

## Alternatives
| Tool | Licence | Why not |
|---|---|---|
| Velopack | MIT | Delta updates and a Rust crate are attractive, but it adds its own NuGet package and `Update.exe` to the install, and its install layout under `%LOCALAPPDATA%` was not checked against the folder rule |
| WiX v6/v7 | MS-RL plus the Open Source Maintenance Fee EULA (a fee above US$10,000 revenue a year; v7 requires accepting it) | The fee terms |
| Hand-built MSI | (needs a tool anyway) | Per-user MSIs are awkward, and the tool question remains |

## Consequences
- CI builds the installer and runs an install, upgrade and uninstall test on `windows-latest` (job "App (Windows)").
- Inno Setup's licence is permissive with notice-keeping conditions; it is recorded in ADR 0003, and its `license.txt` ships in
  `licences\`. Its authors request a purchased licence from commercial users above USD 5,000 a year (not required): open before first
  distribution.
- Uninstall leaves the data folder by design. Removing the data is for the app (a future "remove my data" action) or the user, never
  the uninstaller.

## Sources (read 2026-10-22)
- Inno Setup License (tag `is-7_1_0`, `license.txt`, identical to `is-6_7_3`'s): https://github.com/jrsoftware/issrc/blob/is-7_1_0/license.txt
- Inno Setup downloads (7.1.0 of 2026-08-12; no checksums published): https://jrsoftware.org/isdl.php
- Inno Setup 7 release notes (`whatsnew.htm`, tag `is-7_1_0`): `SetupArchitecture=x64` builds a 64-bit installer and makes
  `x64compatible` the default; "Great care has been taken to ensure maximum backward compatibility with Inno Setup 6"; existing
  installs from 32-bit installers "can be updated by 64-bit installers without issue": https://github.com/jrsoftware/issrc/blob/is-7_1_0/whatsnew.htm
- Inno Setup commercial licence request: https://jrsoftware.org/isorder.php
- Inno Setup help, `PrivilegesRequired` ("Setup will not request to be run with administrative privileges") and Constants (`{autopf}` is
  `{userpf}` in non-administrative installs): https://jrsoftware.org/ishelp/
- KNOWNFOLDERID (`FOLDERID_UserProgramFiles` default `%LOCALAPPDATA%\Programs`; updated 2023-08-30):
  https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid
- Velopack repository (MIT): https://github.com/velopack/velopack
- WiX Open Source Maintenance Fee: https://docs.firegiant.com/wix/osmf/
