# Windows app shell (D5b)

The WinUI 3 app in `apps/windows`: its layout, folders, identity, single instance, the smoke run, and what the owner checks by hand.
Dates are when a source was read.

## Layout
```
apps/windows/
  Current.Shell/          the app (WinUI 3, net10.0-windows10.0.26100.0, unpackaged, self-contained, x64); builds Current.exe
    Program.cs            own Main: AppUserModelID, single instance, then XAML
    AppIdentity.cs        AppUserModelID, single-instance key, data folder
    App.xaml(.cs)         owns the CoreHost (one per process) and the main window
    MainWindow.xaml(.cs)  Mica, TitleBar, NavigationView over one Frame, back navigation
    Pages/                HomePage (empty state), SettingsPage
    ViewModels/           SettingsViewModel (version, capabilities, diagnostics)
    DispatcherQueueUi.cs  IUiDispatcher over DispatcherQueue (ADR 0004)
    SmokeRun.cs           Current.exe --smoke
  build.ps1               dotnet publish -c Release -r win-x64 -p:Platform=x64 into apps/windows/out/app, plus licences\ and
                          out/files.txt (every shipped file)
  smoke.ps1               runs Current.exe --smoke and checks its result
  installer/
    Current.iss           the Inno Setup script (ADR 0009)
    build-installer.ps1   ISCC into apps/windows/out/installer/Current-<version>-x64-setup.exe
    test-install.ps1      install, upgrade, uninstall test
```
Build: `pwsh bindings/csharp/build.ps1` (core, generated C#, bindings), then `pwsh apps/windows/build.ps1`, then
`pwsh apps/windows/smoke.ps1 -Exe apps/windows/out/app/Current.exe`. CI runs the same three in the job "App (Windows)".

The app references `Current.Bindings` by project; `-p:CoreLibDir` (set by `build.ps1`) flows to it, and its `current_ffi.dll` item is
copied into the app's output and publish folder. The pitfalls in `docs/notes/uniffi-bridge.md` apply: `-p:Platform=x64` with
`-r win-x64`, `ImplicitUsings`, and `x:DataType` only on public types (the settings page binds `Current.Bindings.ServerStatus`).

## Shell behaviour (ADR 0001: the platform's default)
- **Window:** `MicaBackdrop` (Base), `ExtendsContentIntoTitleBar` and `SetTitleBar` on a WinUI `TitleBar` control titled "Current";
  the system caption buttons stay the system's. Theme: no `RequestedTheme` anywhere, so the app follows the Windows light/dark setting.
- **Navigation:** a left `NavigationView` with Home and the built-in Settings item over one `Frame`, as in the NavigationView page's
  example (learn.microsoft.com/windows/apps/develop/ui/controls/navigationview, ms.date 2026-10-02, read 2026-10-22): navigate on
  `ItemInvoked` only when the page differs (no duplicate back stack entries), keep the selection and header in step on
  `Frame.Navigated` (the settings item via `SettingsItem`), and do not go back while the pane overlays the content.
- **Back button in the title bar.** The TitleBar page (learn.microsoft.com/windows/apps/develop/ui/controls/title-bar, ms.date
  2026-10-13, read 2026-10-22): "Fluent Design guidance recommends that these controls be placed in the title bar when a custom title
  bar is used." So the NavigationView's back and pane toggle buttons are hidden and the TitleBar's are used, enabled from
  `Frame.CanGoBack`.
- **Alt+Left, the GoBack key and the mouse Back button** call the same `TryGoBack`; **Alt+Right and the mouse Forward button** call
  `TryGoForward` (`Frame.GoForward` when `CanGoForward`). The WinUI 3 page "Navigation history and backwards navigation" (ms.date
  2026-09-19, read 2026-10-22) no longer lists these inputs; the UWP page (learn.microsoft.com/windows/uwp/ui-input/back-navigation,
  ms.date 2025-04-10, read 2026-10-22) does: "Alt+Left arrow, VirtualKey.GoBack" and "VirtualKey.XButton1 (Such as the back button
  found on some mice.)", and "We recommend that you support accelerator keys for both forward and back navigation because users who
  rely on these will expect both", with Alt+Right and `IsXButton2Pressed` for forward. Its `CoreDispatcher` and `CoreWindow` events do
  not exist in WinUI 3 desktop, so the window root has `KeyboardAccelerator`s (Left with Menu, GoBack, Right with Menu) and a
  `PointerPressed` handler registered with handledEventsToo that checks `IsXButton1Pressed` and `IsXButton2Pressed`. The forward stack
  is the Frame's own: it drops it on a new navigation; nothing custom is added (manager, 2026-10-22). There is no forward button, as in
  the NavigationView's design. Windows+Backspace and the gamepad B button come through `SystemNavigationManager`, which a desktop
  window does not have.
- **Home:** the empty state before any server exists: "No servers yet". Server setup is M1.
- **Settings:** the core's version (`CoreHost.AppInfo`), the capabilities state through `ObserveCapabilities` (empty: "No servers are
  set up."), and "Copy diagnostics", which puts `DiagnosticsText()` on the clipboard (`Clipboard.SetContent`, then `Flush` so it
  stays after the app exits) and shows a success `InfoBar`. The subscription starts on the page's `Loaded` and is disposed on
  `Unloaded` (ADR 0004 lifetimes).
- **Core lifetime:** one `CoreHost` per process, created in `OnLaunched` and disposed when the main window closes.

## Identity: the AppUserModelID
- `Skeksik.Current` (owner's choice, 2026-10-22). "Application User Model IDs" (learn.microsoft.com/windows/win32/shell/appids, updated
  2025-04-15, read 2026-10-22): the form is `CompanyName.ProductName.SubProduct.VersionInformation`, "no more than 128 characters and
  cannot contain spaces"; `VersionInformation` "should be omitted so that an upgraded version can use the same AppUserModelID".
- Only without package identity (`GetCurrentPackageFullName` fails with `APPMODEL_ERROR_NO_PACKAGE`; with identity it reports
  `ERROR_INSUFFICIENT_BUFFER` for an empty buffer, learn.microsoft.com, read 2026-10-22): a packaged build gets its identity from the
  package. The process sets it first thing in `Main` with `SetCurrentProcessExplicitAppUserModelID`, which "must" be called "before the
  application presents any UI" (same page). The same page says an app with an explicit ID "must also assign the same AppUserModelID to
  all running windows or processes, shortcuts, and file associations": the installer writes it into the Start menu shortcut's
  `System.AppUserModel.ID` (installer section).
- Whether the media flyout and taskbar then show "Current" (ADR 0007's open item) is checked by hand once the shortcut exists; the
  flyout needs the player (M2), so for now only the taskbar and Start are checked.

## Single instance
`Program.Main` follows "How to create a single-instanced WinUI app with C#" (learn.microsoft.com/windows/apps/windows-app-sdk/
applifecycle/applifecycle-single-instance, ms.date 2026-09-10, read 2026-10-22): `DISABLE_XAML_GENERATED_MAIN`,
`AppInstance.FindOrRegisterForKey("main")`; a later launch calls `RedirectActivationToAsync` on a worker thread while the STA waits in
`CoWaitForMultipleObjects`, then hands the foreground to the first instance's window and exits. The first instance's `Activated`
handler restores its window if minimised and activates it. `--smoke` skips the key, so a smoke run is never redirected.

## Folders
| What | Where | Why |
|---|---|---|
| Program | `%LOCALAPPDATA%\Programs\Current` | `FOLDERID_UserProgramFiles`, default path `%LOCALAPPDATA%\Programs`, per user ("KNOWNFOLDERID", learn.microsoft.com/windows/win32/shell/knownfolderid, updated 2023-08-30, read 2026-10-22) |
| Data | `%LOCALAPPDATA%\Current` | `FOLDERID_LocalAppData` (same page): per user, not roaming; a separate tree from the program folder |

- The program folder is never the data folder, and the installer never names the data folder, so uninstall and upgrade cannot delete
  it. The install test checks this with a marker file.
- The data folder is resolved in C# (`AppIdentity.DataFolder`) for now: `Environment.SpecialFolder.LocalApplicationData` (the
  known-folder API) plus `Current` when unpackaged, `ApplicationData.Current.LocalFolder` with package identity. Nothing writes there
  yet. When storage arrives (M1), the
  platform UI resolves its native data folder and passes it to the core when it creates `App` (manager, 2026-10-22).

## Installer (ADR 0009)
- Inno Setup 7.1.0 (owner, 2026-10-22), installed in CI from its official 64-bit installer (`github.com/jrsoftware/issrc` release
  `is-7_1_0`, `innosetup-7.1.0-x64.exe`) per user (`/CURRENTUSER`, documented for Inno's own installer since 6.1.1 per its release
  notes) into the runner's temp folder. The SHA-256 is our own measurement of that download on 2026-10-22, since jrsoftware.org
  publishes no checksums: trust on first download. The script builds a 64-bit installer (`SetupArchitecture=x64`, new in 7).
  Inno 7's release notes say constants like `{autopf}` behave as before; the install test checks the resulting folder, so a
  difference would fail CI.
- `Current.iss`: fixed `AppId`, `PrivilegesRequired=lowest`, `DefaultDirName={autopf}\Current`, x64 only, one shortcut
  `{autoprograms}\Current` with `AppUserModelID: "Skeksik.Current"` (Inno's `[Icons]` parameter: "Specifies the Windows 7 (or later)
  Application User Model ID for the shortcut", ishelp, read 2026-10-22), and an "Open Current" check box at the end (not in silent
  installs). It ships the published app, Inno's `license.txt` in `licences\`, and its own `unins000.exe`/`unins000.dat`.
- Upgrade: the same `AppId` installs over the old version into the same folder; files of the old version that the new one no longer
  has stay until uninstall (Inno's uninstall log covers every file it ever installed there). No `[InstallDelete]` wildcard: a user may
  have chosen a folder that holds other files.
- Running app: Inno's default `CloseApplications=yes` uses the Restart Manager to close Current before replacing its files (not tested
  in CI).
- `test-install.ps1` (CI, job "App (Windows)"): installs 0.0.1, checks `Current.exe`, the Apps entry's version, the shortcut and its
  `System.AppUserModel.ID`, runs the smoke run from the install; writes `%LOCALAPPDATA%\Current\install-test-marker.txt`; installs
  the workspace version over it and checks again plus the marker; uninstalls silently, waits for the uninstaller (it runs from a temp
  copy) and checks that the program folder, shortcut and Apps entry are gone and the marker remains.
- Not done: code signing (needed for public releases: SmartScreen, the Store's installer route), a licence page (see Licences), the
  MSIX build (ADR 0009 lists what it would change).

## Smoke run: `Current.exe --smoke`
Starts XAML and the core without showing a window, prints one line per check and exits 0 only if all pass. These are the bridge
checks D1 left to a person (`docs/notes/uniffi-bridge.md`, "What the owner runs by hand"), now run in CI:
1. the core starts and reports a version;
2. subscribing to the capabilities state makes the core hold one more listener;
3. the first value arrives, and on the UI thread (`DispatcherQueue.HasThreadAccess` inside the `PropertyChanged` handler);
4. it has no servers;
5. disposing the subscription brings `LiveListeners` back to where it was (within 10 s);
6. the diagnostics text can be read;
7. the main window (XAML, Mica, TitleBar, NavigationView, Home) builds and closes.
`smoke.ps1` fails on a non-zero exit code, a missing "smoke run passed" line or a run longer than 120 s.

## Open before first distribution
Nothing built here goes to anyone else, testers included, until these are done (owner, 2026-10-22; ADR 0003, ADR 0009):
1. End-user terms that protect Microsoft's code at least as much as the Windows App SDK licence does (its section 3(b)(ii)); the
   owner provides them.
2. An installer licence page showing those terms (Inno's `LicenseFile`).
3. The indemnity in the Windows App SDK licence's 3(b)(iii): the owner's to accept.
4. The Windows App SDK's telemetry (its licence's 2(a)): check what it sends and which options turn it off.
5. Code signing of the app and the installer (SmartScreen; the Store's installer route).
6. Inno Setup's commercial licence: its authors request (do not require) one from for-profit users with more than USD 5,000 annual
   revenue, for any version (jrsoftware.org/isorder.php, read 2026-10-22). The owner decides.

## What the owner checks by hand
On the owner's PC, with the installer from CI (or `apps/windows/out/app/Current.exe`):
1. The window has Mica, the title bar is part of the window, it drags and double-click maximises; the caption buttons and snap layouts
   (hover over maximise) work.
2. Switch Windows between light and dark: the app follows at once.
3. Home shows the empty state. Settings shows the version, "No servers are set up." and "Copy diagnostics"; paste the clipboard
   somewhere: the text is there, also after closing the app.
4. Back and forward: go Home -> Settings, then back with the title bar's back button, Alt+Left, and a mouse Back button; each
   returns to Home, and at Home the back button is disabled. After going back, Alt+Right and a mouse Forward button return to
   Settings. Narrow the window until the pane collapses: the pane toggle in the title bar opens it.
5. Keyboard only: Tab reaches the back button, the pane and the settings controls; Narrator reads them by name.
6. Start a second copy (from Start or the exe): no second window; the first one comes to the front, also when minimised.
7. Taskbar and Start show "Current" with the app's name (not "Current.exe"); pin to the taskbar and start from the pin.
