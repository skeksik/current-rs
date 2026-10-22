# Desktop: storage, secrets, packaging, single instance (from platform notes)

> Reference only, copied unchanged from the earlier Kotlin Multiplatform project (`skeksik/Current` at `66452b2`, 2026-10-22).
> Facts carry the dates they were checked on. ADR numbers, module names and "the app" refer to that project. Nothing here is a
> decision for this repo; see `README.md` in this folder.

## Desktop settings storage (checked 2026-10-20 on Windows 11 with JDK 21; ADR 0016)
- `java.util.prefs` on Windows writes under `HKCU\Software\JavaSoft\Prefs`, one key shared by every Java app (`WindowsPreferences`
  holds the string `Software\JavaSoft\Prefs`; the key on this machine already held other vendors' nodes). Uppercase letters are
  escaped with `/` (`/Jet/Brains.`). Limits: `MAX_KEY_LENGTH` 80, `MAX_VALUE_LENGTH` 8192, `MAX_NAME_LENGTH` 80 (`javap -constants`).
- Not verified here (no Linux machine): that the Linux implementation writes to `~/.java/.userPrefs` and syncs on a timer. ADR 0016 uses
  a JSON file instead, so nothing depends on it.
- The desktop app keeps settings in `%APPDATA%\Current` on Windows and `$XDG_CONFIG_HOME/current` on Linux, falling back to
  `~/.config/current` when the variable is unset or not an absolute path, as the XDG Base Directory specification asks
  (https://specifications.freedesktop.org/basedir-spec/latest/).
- A start of the desktop app with the new Koin modules created no folder: the store reads only, and writes on the first change.
- Not verified: whether an MSIX (Microsoft Store) package redirects `%APPDATA%` writes to a per-package folder (M8, Store edition).

## Secrets on the desktop (checked 2026-10-20; ADR 0017)
- Windows Credential Manager through JNA (`CredWriteW`, `CredReadW`, `CredDeleteW`, `CredFree`; generic credentials, persisted on the
  local machine): a round trip with a non-ASCII value, overwrite, delete and a missing key (`ERROR_NOT_FOUND`, 1168) passed on Windows 11
  with JDK 21, and `cmdkey /list` showed no entry left. A value over 2560 bytes is refused before the call.
- Linux, checked in WSL (Ubuntu 24.04, OpenJDK 21.0.12, `libsecret-1-0`, `gnome-keyring`), with the test classes built on Windows:
  - GNOME Keyring unlocked in a `dbus-run-session` (`echo -n <password> | gnome-keyring-daemon --unlock --components=secrets`):
    `secret_password_store_sync`, `lookup` and `clear` with our schema round-trip a value.
  - **A lookup is no availability check.** With no session bus at all, `secret_password_lookup_sync` returned NULL without a `GError`.
    So the app treats the Secret Service as available when a session bus exists (`DBUS_SESSION_BUS_ADDRESS`, or `$XDG_RUNTIME_DIR/bus`)
    and `libsecret` loads.
  - **Calls can block on the user.** On a bus without an unlocked keyring, D-Bus activation started `gnome-keyring-daemon`, which asked
    `gcr-prompter` to create a keyring; under WSLg that prompt can open a window. `secret_password_store_sync` then waits until the
    prompt is answered. The app runs each call on a worker thread with a 2-minute limit and falls back to memory; with no display for
    the prompt, the store failed within the test's 10-second limit.
  - Without a keyring, the opt-in secrets file and its folder got `rw-------` and `rwx------`.
- WSL (Ubuntu) runs a per-user session bus at `/run/user/<uid>/bus` and sets `DBUS_SESSION_BUS_ADDRESS` to it.
- Not verified: KWallet's Secret Service interface (KDE Plasma), a real desktop session's unlock prompt, Flatpak or Snap sandboxes.

## Credential storage in other apps (checked 2026-10-20; ADR 0017's alternatives)
Read from public documentation of a widely used app framework's secure-storage API, a widely used Git credential helper, and a widely
used command-line tool; named in the conversation, not here (CLAUDE.md).
- Windows: the framework encrypts with DPAPI and leaves the encrypted data to the app; the credential helper defaults to Credential
  Manager (and offers DPAPI-protected files), noting that Credential Manager does not persist over a network or SSH session.
- Linux: all three use the Secret Service (libsecret, GNOME Keyring, KWallet) first. Without one: the framework encrypts with a
  hard-coded password ("basic_text"); the command-line tool silently writes a plain-text file (with a flag to force it); the credential
  helper has no default store on Linux and offers Secret Service, GPG/pass, an in-memory cache, or plain-text files it warns against.
- macOS: the Keychain everywhere.


## Desktop packaging (read 2026-10-21 in the Compose Multiplatform docs and the compose-multiplatform plugin source on `master`; packaged only in CI)
- jlink copies only listed modules into the packaged runtime. "The Gradle plugin does not automatically determine necessary JDK Modules
  ... failure to provide the necessary modules can lead to `ClassNotFoundException` at runtime." `suggestModules` runs jdeps, whose
  "output might be incomplete or list unnecessary modules"; `includeAllModules` is the size-for-safety alternative
  (https://kotlinlang.org/docs/multiplatform/compose-native-distribution.html). In the plugin source on `master` (not the 1.12.1 tag) the task is named
  `suggestRuntimeModules`, and the defaults are `java.base`, `java.desktop`, `java.logging`, `jdk.crypto.ec`
  (`gradle-plugins/compose/.../dsl/JvmApplicationDistributions.kt`, `DEFAULT_RUNTIME_MODULES`).
- Windows screen readers work through Java Access Bridge, "disabled by default" (`jabswitch.exe /enable`). A packaged app needs the
  `jdk.accessibility` module. The page names NVDA's Speech Viewer and JAWS' speech history for checking announcements
  (https://kotlinlang.org/docs/multiplatform/compose-desktop-accessibility.html).
- `packageName` is "the application's name"; `description` and `vendor` have no default. `windows { upgradeUuid }` "enables users to update
  the application via the installer, when there is a version newer than an installed version. The value must remain constant".
  `perUserInstall`, `menu`, `menuGroup`, `shortcut` and `dirChooser` (default true) exist in the plugin's `WindowsPlatformSettings`;
  `linux { packageName, debMaintainer, appCategory, menuGroup, shortcut }` in `LinuxPlatformSettings`. MSI versions are
  `MAJOR.MINOR.BUILD` (255, 255, 65535 at most); a DEB version must start with a digit (docs page above).
- jpackage needs JDK 17 or newer, taken from `JAVA_HOME` or `javaHome` (docs page above); the plugin falls back to the `java.home` of the
  JVM running Gradle (`JvmApplicationData.kt`). The desktop app sets `javaHome` from a Java 21 Gradle toolchain, any vendor (cross-cutting review A7).
- WiX: on Windows the plugin downloads WiX 3.11 binaries from the wixtoolset GitHub releases (`downloadWix`, `unzipWix`) unless `WIX_PATH` is
  set or `compose.desktop.application.downloadWix=false` (`internal/wixToolset.kt`).
- The modules the app needs beyond the defaults: `java.net.http` (ktor-client-java uses the JDK's `HttpClient`) and `jdk.accessibility`.
  With those two listed, `suggestRuntimeModules` in CI (2026-10-21, Temurin 21.0.12) suggested `java.instrument` and `jdk.unsupported`,
  so both are listed too. JNA (Credential Manager, libsecret, the title bar) logs through `java.logging`, a default. The packaged runtime
  also pulled in `java.datatransfer`, `java.xml` and `java.prefs` as dependencies. CI checks the runtime's `release` file
  (`scripts/check-desktop-runtime.sh`) and starts the packaged app for 20 s: it booted on Windows and under Xvfb on Linux, where Skiko
  fell back from OpenGL ("Cannot create Linux GL context") to its next renderer. The MSI was about 75 MB.
- Install folder (read in the plugin source on `master` and OpenJDK `jdk21u`; run in CI 2026-10-21):
  - `windows { installationPath }` becomes jpackage's `--install-dir` (`configureJvmApplication.kt`, `AbstractJPackageTask.kt`). On
    Windows that is a "Relative sub-path under the default installation location" (`MSG_Help_win_install_dir`), rejected if it
    contains `:` or `..`, and defaults to the app name. The default location is `ProgramFiles64Folder` system-wide and
    `LocalAppDataFolder` per user (`src/jdk.jpackage/windows/classes/jdk/jpackage/internal/WixAppImageFragmentBuilder.java`:
    `installDir = (systemWide ? PROGRAM_FILES : LOCAL_PROGRAM_FILES).resolve(WINDOWS_INSTALL_DIR...)`).
  - So a per-user install without it goes to `%LOCALAPPDATA%\Current`, which is `AppFolders.local()`. The generated WiX adds a
    directory cleaner on `INSTALLDIR` (`addDirectoryCleaner`, "rm -rf", `WixRemoveFoldersEx`), which deleted `local.json` on upgrade
    and uninstall (section "Desktop installer: Windows run").
  - `Programs\Current` (the usual place for per-user apps) cannot be built. Passed as `Programs\Current` it installed into
    `%LOCALAPPDATA%\ProgramsCurrent`: the plugin writes each value quoted into an `@...args.txt` file for jpackage
    (`AbstractJvmToolOperationTask.kt`, `cliArgUtils.kt`), and a backslash inside quotes there is an escape. Passed as
    `Programs/Current` or `Programs\\Current`, `packageMsi` failed: `light.exe` exited with 204 (ICE validation). For a per-user
    install, jpackage adds a `<RemoveFolder>` component only for directories inside `INSTALLDIR` (`addDirectoryHierarchy`: "Per-user
    install requires <RemoveFolder> component in every directory"); `addRootBranch` creates the folders above it (`Programs`) and
    returns no components, so WiX's ICE64 rejects a per-user folder without a `RemoveFile` entry. The same code is in `jdk25u` and
    `jdk` `master` (2026-10-21), so a newer JDK does not help. The plugin does not expose `--resource-dir` for a patched `main.wxs`
    (it uses its own `build/compose/tmp/resources`).
  - The app sets `installationPath = "Current App"`, i.e. `%LOCALAPPDATA%\Current App`. Only the folder differs: the installed-apps
    entry, the Start menu group and shortcut and the desktop shortcut are named Current. CI installs a 0.1.0 MSI, writes
    `%LOCALAPPDATA%\Current\local.json`, upgrades to 0.1.1 and uninstalls, and checks the file survives, the shortcuts exist and then
    go, and one entry is left at 0.1.1 (`scripts/check-msi-upgrade.ps1`).
  - **Rule: nothing may ever be written under the install folder.** Removing the product (uninstall, and the old version during an
    upgrade) deletes it recursively.
  - An install of an older build (into `%LOCALAPPDATA%\Current`) still deletes that folder when it is upgraded or removed.
- Installed-apps icon: jpackage's `main.wxs` sets `ARPPRODUCTICON` from the launcher's icon (`WinMsiBundler.java`, `JpIcon`). Windows
  Installer keeps it as the product's `ProductIcon` under `HKCU\Software\Microsoft\Installer\Products`, not as `DisplayIcon` in the
  Uninstall key, which is why the registry showed no `DisplayIcon`; whether Settings > Apps shows it is not checked. The plugin has no
  option for `DisplayIcon`.
- Screen readers: AWT loads assistive technologies from `javax.accessibility.assistive_technologies` or the `assistive_technologies`
  line in `~/.accessibility.properties` (`java.awt.Toolkit`, `jdk21u`). `jabswitch /enable` writes that line
  (`assistive_technologies=com.sun.java.accessibility.AccessBridge`) and sets the Windows accessibility registry key (`jabswitch.cpp`).
  The plugin's jlink step passes `--strip-native-commands` (`AbstractJLinkTask.kt`), which is why the bundled runtime has the Access
  Bridge DLLs but no `jabswitch.exe`. Narrator cannot read the app with any setting, and an installed NVDA turns the bridge on by itself:
  see "Windows screen readers and the desktop app".
- Single instance: a second launch opened a second window over the same settings files (Windows run). Fixed by a file lock and a
  loopback signal (section "Single instance on desktop").
- Window icon: `main.kt` sets the `Window` icon from `icons/current.png`, which the build puts on the classpath (the installer's own
  PNG, no new file). Not yet seen on screen.
- jpackage writes the DEB `Maintainer` field as `<vendor> <debMaintainer>`, so `debMaintainer` is an address only (seen in CI with
  `dpkg-deb --info`).
- Not verified: NVDA through the packaged runtime, a DEB on a real Debian or Ubuntu desktop. The MSI: see "Desktop installer: Windows run".

## Desktop installer: Windows run (checked 2026-10-21 on Windows 11 Pro 10.0.26200.9711; PR 45 at db896ed)
- Build, pass: `packageMsi` took 30 s and wrote `Current-0.1.0.msi`, 75,137,448 bytes. No JDK download: the toolchain took the Zulu
  21.0.11 already in `~/.gradle/jdks` (the runtime's `release` file says 21.0.11 and lists the expected 11 modules). WiX 3.11 was
  downloaded (`downloadWix`).
- Install with `msiexec /i ... /qn`, pass: no elevation (log: "LUA installation capable with no elevation required"), into
  `%LOCALAPPDATA%\Current` (about 138 MiB). Start menu group "Current" with a "Current" shortcut and a desktop shortcut, both to
  `Current.exe` with the exe's own icon. One Windows Installer entry (per user): Current 0.1.0, publisher skeksik, no `DisplayIcon`. Read
  from the registry; Settings > Apps not looked at.
- Launch from the Start menu shortcut, partly fail: the window title is "Current" and the taskbar button has the Current icon, but the
  title bar shows a generic icon, not Current's (the owner confirmed it by eye; `main.kt` sets no window `icon`). Added a loopback Jellyfin
  stand-in (`http://127.0.0.1:18096`), signed in, Settings > About: "Version 0.1.0". Also seen: a second launch starts a second instance
  (two windows, each with its own state over the same settings files); there is no single-instance check.
- Upgrade to 0.1.1 (local `current.version` only), partly fail: in place, one entry, version 0.1.1, MSI 75,137,444 bytes; the server was
  still saved and signed in (token in Credential Manager) and About showed 0.1.1. **Bug:** the install folder is the app's machine-local
  folder (`AppFolders.local()`, `%LOCALAPPDATA%\Current`). Removing the old product runs `WixRemoveFoldersEx` on `INSTALLDIR` recursively,
  so `local.json` (the device id, and a secrets file if the user chose one) was deleted; the next launch made a new device id. Not
  checked: what a new device id does to a session on a real Jellyfin server.
- Uninstall with `msiexec /x ... /qn`, pass with the same bug: the files, both shortcuts and the entry are gone, and so is all of
  `%LOCALAPPDATA%\Current` with `local.json`. `%APPDATA%\Current\settings.json` and the Credential Manager tokens stay.
- Owner check, fail: name and icons right except the title bar (above). Narrator read only "Current window" for the whole window, no
  button or page title. NVDA was not run (not installed; Access Bridge not enabled). The bundled runtime has the Access Bridge DLLs but
  no `jabswitch.exe`; the toolchain JDK's works.

Re-run (2026-10-21, same machine, PR 45 at 629e4ce), only what CI can't see: the title bar icon and an upgrade over real app data. No
earlier build was installed; the existing `local.json` and `settings.json` were backed up first.
- Build and install 0.1.0, pass: `packageMsi` wrote `Current-0.1.0.msi` (75,190,713 bytes); `msiexec /i ... /qn` exit 0 with no
  elevation, files in `%LOCALAPPDATA%\Current App` (about 139 MiB), and `%LOCALAPPDATA%\Current` still holds only `local.json`.
- Launch from the Start menu, pass: the owner confirmed by eye that the title bar and the taskbar both show the Current icon.
- Device id, pass: the SHA-256 of `local.json` matched the backup after the install and the launch.
- Upgrade to 0.1.1 (`-Pcurrent.version=0.1.1`, 75,186,617 bytes) over 0.1.0 with `/qn`, pass: exit 0, one entry, Current 0.1.1. The
  owner saw About "0.1.1", the saved servers and sign-ins still there, and both icons still right. The `local.json` hash was unchanged
  after the upgrade and after the launch.
- 0.1.1 left installed at the owner's choice; uninstall not re-run here (CI covers it).


## Single instance on desktop (read 2026-10-21 in the JDK 21 javadoc, the Win32 docs and the Compose `application` source on `jb-main`; tested on Linux in a container)
Design: docs/architecture.md, "Single instance (desktop)". What it relies on:
- The lock dies with the process. `FileLock`: "A file-lock object is initially valid. It remains valid until the lock is released by
  invoking the release method, by closing the channel that was used to acquire it, or by the termination of the Java virtual machine,
  whichever comes first." So a lock file left by a crash does not block: the next `tryLock` succeeds. The test
  `anotherProcessHoldsTheLockUntilItDies` kills a child JVM that holds it (`destroyForcibly`) and claims it next.
- `FileChannel.tryLock()` returns "A lock object representing the newly-acquired lock, or null if the lock could not be acquired
  because another program holds an overlapping lock", and throws "OverlappingFileLockException - If a lock that overlaps the requested
  region is already held by this Java virtual machine". `FileLock`: "File locks are held on behalf of the entire Java virtual machine.
  They are not suitable for controlling access to a file by multiple threads within the same virtual machine." Both mean "held"; the
  second only happens in tests.
- Advisory is enough: "it is strongly recommended that the locks provided by this API be used as if they were advisory locks"
  (`FileLock`). Only the app takes this lock and nothing reads `instance.lock`'s content.
- The port: `ServerSocket(port, backlog, bindAddr)`, "A port number of 0 means that the port number is automatically allocated,
  typically from an ephemeral port range", and with `bindAddr` the socket "will only accept connect requests to one of its
  addresses". The address is 127.0.0.1 from its bytes (`InetAddress.getByAddress(byte[])`: "This method doesn't block, i.e. no reverse
  lookup is performed"), not `getLoopbackAddress()`, which may be `::1`.
- `instance.port` is written to a temporary file and moved into place. `Files.createTempFile` takes "optional file-attributes to set
  atomically when creating the file", so on POSIX it is created `rw-------` before the token is in it. On Windows it gets the default
  ACL of `%LOCALAPPDATA%\Current`; what that ACL is on the owner's machine was not checked. `ATOMIC_MOVE`: "The move is performed as an
  atomic file system operation and all other options are ignored. If the target file exists then it is implementation specific if the
  existing file is replaced or this method fails by throwing an IOException." So on that `IOException` the code deletes the old file
  and moves again; a reader in between finds no file and retries.
- The token is compared with `MessageDigest.isEqual`; the port and the token are never logged.
- Exit: `application` ends the process: "Set [exitProcessOnExit] to `false`, if you need to execute some code after [application]
  block, otherwise the code after it won't be executed, as [application] will exit the process" (`Application.desktop.kt`, `jb-main`;
  not compared with the 1.12 tag). So the claim is released by a shutdown hook (port file deleted first, then the lock).
- Bringing the window forward: `Window.toFront()`, "Some platforms may not permit this VM to place its Windows above windows of native
  applications, or Windows of other VMs. [...] developers should not assume that this method will move this Window above all other
  windows in every situation." On Windows, `SetForegroundWindow` works for a process only if, among other conditions, "The calling
  process was started by the foreground process" or "The calling process received the last input event"; otherwise "Windows flashes the
  taskbar button of the window to notify the user". The later launch was just started by the user, so before signalling it calls
  `AllowSetForegroundWindow(pid)` for the running app (JNA, `WindowsForeground`): "A process that can set the foreground window can
  enable another process to set the foreground window by calling AllowSetForegroundWindow." The call "will fail if the calling process
  cannot set the foreground window"; the result is ignored and the signal goes out anyway. Not read: whether AWT's `toFront` on
  Windows calls `SetForegroundWindow`. Seen on Windows: the window takes the foreground over another app, and the pid in the file is
  the inner `Current.exe` that owns the window (section "Single instance: Windows run").
- Tested in a Linux container: the tests in `FileLockSingleInstanceTest` (claim, signal, close, stale files, wrong token, no answer,
  a killed child process); CI runs them on Linux and Windows. On the installed MSI: section "Single instance: Windows run".

## Single instance: Windows run (checked 2026-10-22 on Windows 11 Pro 25H2, 10.0.26200.9711; PR 50 at c98d486)
- Build: `packageMsi` with `-Pcurrent.version=0.1.2`, because the branch says 0.1.0 and 0.1.1 was installed, and a lower version does
  not upgrade in place. `msiexec /i <msi> /qn` over the per-user 0.1.1: exit code 0, one Installed-apps entry, 0.1.2. Every launch was
  a click on Current in the Start menu.
- 1, launch while running: one window, one `Current.exe` pair, Current in front. Pass.
- 2, minimised, then launch: the window is restored and in front. Pass.
- 3, Settings > Remove dialog open, then launch: the window is in front and the dialog still open. Pass.
- 4, a maximised File Explorer in front, then launch: Current takes focus (it is the foreground window), no taskbar flash. Pass.
- 5, ended with `taskkill /F` (Task Manager runs elevated, so it could not be driven; End task also terminates the processes):
  `instance.lock` and `instance.port` stay behind, the next launch starts normally with no message and writes a new `instance.port`. Pass.
- 6, the app's JVM process suspended (`NtSuspendProcess`; Resource Monitor runs elevated), then launch: "Current is already running but
  isn't responding. Close it in Task Manager and try again." shows between 2 and 5 s after the click (not timed more finely); OK ends
  the new launch. After a resume, the next launch brings the window forward. Pass.
- 7, the `local.json` and `settings.json` hashes after all cases match the backup taken before the install. Pass.
- Also seen: the pid in `instance.port` is the inner `Current.exe`, the child of the outer launcher process and the owner of the window;
  `AllowSetForegroundWindow` for it is enough for case 4. Closing the window deletes `instance.port` and leaves an empty `instance.lock`.

