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
    Pages/                HomePage, AddServerPage, SignInPage (+ SignInRequest), SettingsPage
    ViewModels/           ServersViewModel and ServerRow (saved servers joined with their status, for Home and Settings)
    DeviceName.cs         the device name Jellyfin shows (GetComputerNameExW, DNS host name)
    NetworkObserver.cs    NetworkFacts from Windows.Networking.Connectivity (ADR 0014)
    ProblemText.cs        the core's problems in plain words
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

Project settings that matter, each found the hard way in CI:
- **Component packages, not the metapackage.** `Microsoft.WindowsAppSDK.WinUI` 2.3.9 and `Microsoft.WindowsAppSDK.InteractiveExperiences`
  2.1.9, the versions the Windows App SDK 2.5.1 metapackage pins. The metapackage also put the AI, ML (onnxruntime, 21.7 MB), Widgets
  and Search runtimes into the self-contained app, which uses none of them. The Windows App SDK 1.8 release notes, "NuGet
  Metapackage" (learn.microsoft.com, ms.date 2026-09-24, read 2026-10-22): "This allows developers to choose either the metapackage
  or select specific component packages for their applications." WinUI 2.3.9 asks for InteractiveExperiences 2.1.8, which nuget.org
  does not have, so 2.1.9 is named directly and NuGet's NU1603 notice stays a warning. Foundation 2.3.12 (AppLifecycle, which the
  single instance uses) and Base 2.0.4 come with WinUI; Base also brings `Microsoft.Windows.SDK.BuildTools.MSIX`, so no reference to
  it is needed.
- **`EnableMsixTooling=true`** although the app is unpackaged. Without it the publish folder had no `Current.pri` and loading
  `MainWindow.xaml` failed with "Cannot locate resource from 'ms-appx:///MainWindow.xaml'". The typical WinUI project in "Project
  properties and auto-initializers" (learn.microsoft.com, ms.date 2026-10-02, read 2026-10-22) sets it. `build.ps1` fails if the
  publish folder has no app PRI.

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
  not exist in WinUI 3 desktop, so the window root has `KeyboardAccelerator`s (Left with Menu and Right with Menu in XAML; GoBack added in code, because XAML
  fails with "Failed to create a 'Windows.System.VirtualKey' from the text 'GoBack'", seen in CI) and a
  `PointerPressed` handler registered with handledEventsToo that checks `IsXButton1Pressed` and `IsXButton2Pressed`. The forward stack
  is the Frame's own: it drops it on a new navigation; nothing custom is added (manager, 2026-10-22). There is no forward button, as in
  the NavigationView's design. Windows+Backspace and the gamepad B button come through `SystemNavigationManager`, which a desktop
  window does not have.
- **Home:** see "Screens" below.
- **Settings:** see "Screens" below. The clipboard behaviour of "Copy diagnostics" is unchanged: the clipboard is shared, and Windows
  refuses it with `CLIPBRD_E_CANT_OPEN` while another process has it open ("OpenClipboard fails if another window has the clipboard
  open", OpenClipboard, learn.microsoft.com, read 2026-10-22); the owner's run hit that in `Flush` with clipboard history on, and the
  unguarded call ended the app. `ClipboardText.TryCopyAsync` retries `SetContent` and `Flush` up to 5 times, 100 ms apart (our
  choice: Microsoft's `Clipboard.Flush` page gives no retry guidance), and the page shows an error `InfoBar` if the clipboard stays
  busy. Handlers are `async void` and catch everything themselves; `Application.UnhandledException` logs and marks handled any
  exception that still escapes a UI event handler, so none ends the process. The smoke run copies a marker and reads it back; the
  refused-clipboard case itself cannot be produced in CI.
- **Core lifetime:** one `CoreHost` per process, opened in `OnLaunched` (see "Start") and disposed when the main window closes, after
  the network observer.

## Start (M1)
- The window shows at once; `App.OpenCoreAsync` then awaits `CoreHost.OpenAsync(AppIdentity.DataFolder, DeviceName.Get(),
  SecretStore.Keyring)` on the UI thread (the adapter runs the file and keyring work on the core's threads). Pages wait for
  `App.CoreReady`; Home shows a `ProgressRing` only if opening takes more than 0.5 s.
- The load report shows as a notice above every page only when the user should know: `ResetCorrupt` ("Your settings couldn't be
  read and were reset", with the backup file's name) and `NewerVersion` (update Current to sign in or save changes), both Warning.
- If the data folder cannot be opened (`CoreStoreException`), the app runs on `CoreHost.Create()` (nothing stored) with an Error
  notice: "Current couldn't open its settings folder. Sign-ins won't be saved this time.", plus the step and kind of the error and
  never the path (manager, 2026-10-23).
- Then the network observer starts, and `RestoreSessionsAsync` runs in the background; the capabilities state shows each server's
  outcome.
- Notices are `InfoBar`s in a stack above the `Frame` (`MainWindow.ShowNotice`); the user closes them. Also used for a sign-in that
  is not remembered.

## Device name (ADR 0015)
ADR 0015's Windows rule is what Jellyfin's desktop client sends: Qt's `QHostInfo::localHostName()`, which on Windows is
`QSysInfo::machineHostName()`, which calls `GetComputerNameEx(ComputerNameDnsHostname, …)` (qtbase `src/corelib/global/qsysinfo.cpp`,
read 2026-10-23). `DeviceName.Get` calls `GetComputerNameExW(ComputerNameDnsHostname)` through `DllImport` (no `unsafe`), with a
256-character buffer and one retry at the size Windows asks for on `ERROR_MORE_DATA` ("GetComputerNameExW", learn.microsoft.com,
ms.date 2018-12-05, read 2026-10-23). `Environment.MachineName` is not used: it "Gets the NetBIOS name" (learn.microsoft.com, read
2026-10-23), which "may be a truncated version of the DNS host name" (COMPUTER_NAME_FORMAT, same date). A failure passes "" and the
core uses `Windows`. The name is never logged or printed (the smoke run checks only that it is not empty).

## Network observer (ADR 0014)
`NetworkObserver` reports `NetworkFacts` once at start and on every `NetworkInformation.NetworkStatusChanged` ("Occurs when the
network status changes for a connection"; the page does not say on which thread, so each report hops to the UI thread, which also
keeps `NetworkChanged` on one thread). Sources: the Windows.Networking.Connectivity reference pages on learn.microsoft.com
(ms.date 2017-02-08, updated 2025-11, read 2026-10-23).
- Profile: `GetInternetConnectionProfile()`, re-read on every event ("Re-query the profile when you receive a network status change
  event", ConnectionProfile). Without one, the connected profile with the most access, because "Apps should attempt to connect to
  their services whenever `LocalAccess` or higher connectivity is reported" (NetworkConnectivityLevel) and a server on a LAN
  without internet is still reachable. No profile at all: offline.
- `connectivity`: `GetNetworkConnectivityLevel()`: None → offline, LocalAccess → local only, ConstrainedInternetAccess ("possibly
  due to a captive portal") → captive portal, InternetAccess → full.
- `metered`: `GetConnectionCost()`, by the ConnectionProfile page's own example: `NetworkCostType != Unrestricted || Roaming ||
  OverDataLimit`; an `Unknown` cost type is unknown.
- `constrained`: unknown (Windows has no such fact; ADR 0014).
- `interface`: `NetworkAdapter.IanaInterfaceType`: 6 Ethernet, 71 Wi-Fi, 243 WWAN (NetworkAdapter page) and 244 (the IANA
  registry's other WWAN type) → cellular, anything else → other.
- `vpn`: unknown. ADR 0014 marks the Windows tunnel type "not verified"; the interface type alone would be a guess.
- `network_key`: `NetworkAdapter.NetworkItem.NetworkId` ("Gets the network ID", a GUID) and `NetworkAdapterId`, joined. Stable on
  one network, different between networks, and the same network through Wi-Fi and Ethernet stays apart. That `NetworkId` is the
  NLM network GUID ADR 0014 names is not verified. The key goes into the core only: never logged, traced or printed, and the
  bridge's `NetworkFacts.ToString()` redacts it. No Wi-Fi name (`WlanConnectionProfileDetails`), which needs location access.
- Foreground: `MainWindow.Activated` with `Deactivated` sets a flag; the next activation calls `ReturnedToForeground()`, so
  unreachable servers race again at once. Restoring from minimised and a redirected second launch both activate the window.

## Screens (M1)
Sources, read 2026-10-23 on learn.microsoft.com: "Dialog controls" (ms.date 2026-07-15), "Guidelines for password boxes" (ms.date
2025-02-26), "InfoBar" (ms.date 2025-02-26), "Guidelines for progress controls" (ms.date 2026-07-08), "Guidelines for app settings"
(ms.date 2026-04-08), "Keyboard accessibility" (ms.date 2026-10-04). Each screen renders the core's state and sends the user's
actions; nothing is decided in C#.
- **Home** (`HomePage`): before any server, "No servers yet", "Add a Jellyfin server to browse and play your media." and an accent
  "Add a server" button. With servers, a card per saved server: name, "Signed in as …" or "Signed out", "Jellyfin <version>", and
  "Connected", "Can't reach the server" or "Checking…" (plus "address N of M" with more than one address); a signed-out server has a
  "Sign in" button (sign in again), and "Add another server" follows. Browse is M2. `ServersViewModel` joins `ObserveServers` with
  `ObserveCapabilities` and updates rows in place. If the list mirror faults (`ListSubscription.IsFaulted`, which the adapter reports through `PropertyChanged`), the page says
  "Couldn't refresh the server list." with "Reload", which subscribes anew.
- **Add a server** (`AddServerPage`, one `AddServerFlow` and one `DiscoveryFlow` from `OnNavigatedTo` to `OnNavigatedFrom`):
  - "Server address" box (`InputScope="Url"`, placeholder `192.168.1.10:8096 or media.example.com`); Enter or "Connect" probes.
    While probing, the box is read-only (it keeps focus), a 20 px `ProgressRing` with "Connecting…" shows (the progress page's own
    keyword for a ring), and "Cancel" or Esc stops it.
  - Every `ProbeProblem` in plain words (`ProblemText.Probe`), in an error `InfoBar` under the box, on the page and not in a dialog:
    "Don't use dialogs for errors that are contextual to a specific place on the page, such as validation errors … use the app's
    canvas itself to show inline errors" (Dialog controls). `Tls` reads "The secure connection failed. The server's certificate may
    not be trusted by this PC."
  - "Servers on your network": the search starts by itself (manager, 2026-10-23); an indeterminate `ProgressBar` while searching
    (it does not block the page, so a bar, not a ring); a `ListView` of servers in the order they answered, appended so the list
    stays put; clicking one picks it. Done with none: "No servers found on this network. Enter the address instead."; each
    `DiscoveryProblem` in words; "Search again".
  - `ConfirmPlainHttp`: a `ContentDialog`, "Connect without encryption?", "<name> answered over http. Your password and everything
    you watch would cross the internet unencrypted. Use https if the server offers it.", primary "Connect anyway", close "Cancel".
    The close button is the safe action and answers Esc (Dialog controls); `DefaultButton = Close`, so Enter does the safe thing too.
  - `Ready` → the sign-in page with `AddServerFlow.SignIn()`.
- **Sign in** (`SignInPage`, owns the `SignInFlow` it is navigated with): "Sign in to <name>"; when setup fell back from https
  (`HttpsFailedTls`), a Warning: "This server offers https, but its certificate isn't trusted by this PC, so Current is using
  http."
  - "User name" box, then "Password" (`PasswordBox`, the default reveal button) on its own row, then "Sign in". One password box,
    nothing right next to it (PasswordBox page: "Don't put another control right next to a password input box"). Enter in either
    box submits. The password is read from `PasswordBox.Password` into `WithPasswordAsync` at the call and kept in no field; the box
    is cleared when the page is left.
  - "Use Quick Connect" below the form: shown once `CheckQuickConnectAsync` says the server offers it (disabled while unknown,
    hidden when off). Waiting shows the code in `TitleLargeTextBlockStyle`, selectable, named for Narrator digit by digit ("Quick
    Connect code 1 2 3 4 5 6"), with "Waiting for approval…" and "Cancel" (Esc too).
  - `SigningIn`: a ring with "Signing in…" (the progress page's example of a ring); Cancel or Esc.
  - Each `SignInProblem` in plain words (`ProblemText.SignIn`) in an error `InfoBar`.
  - `SignedIn`: back to Home and the back and forward history cleared (`MainWindow.ClearHistory`, which also updates the title
    bar's back button), so Back does not return into a finished sign-in (manager, 2026-10-23).
  - The page owns and disposes its flow, so it is never returned to: `MainWindow.OnNavigated` removes sign-in entries from the
    back and forward stacks, and the page ignores a Back/Forward navigation or a request already taken. Back from a sign-in goes to
    the page before it.
    `Remembered: false` adds a notice: "Signed in for this session only. Windows Credential Manager isn't available, so you'll need
    to sign in again next time."
- **Settings** (`SettingsPage`), laid out as "Guidelines for app settings" says: one scrollable column of at most 1000 px,
  BodyStrong section headers, `SettingsCard` and `SettingsExpander` from the Windows Community Toolkit (below), "Accounts" (its
  example of a one-word label), and About last:
  - **Accounts:** a card per saved server (name; user, version and reachability as above) with "Sign out", or "Sign in" when signed
    out. Signing out asks nothing (manager, 2026-10-23): signing in again undoes it. `ServerTold: false` (the server did not answer)
    shows "Signed out of <name> on this PC. The server couldn't be told, so the session may still show in its dashboard."
  - **Diagnostics:** "Copy diagnostics" now copies `ExportDiagnosticsAsync()` (version, platform, settings state, then the
    redacted request log).
  - **About:** a `SettingsExpander` "Current" with the version, and "© 2026 Skeksik" inside.
- **Keyboard and Narrator:** every input has a `Header` that names it; buttons in lists carry `AutomationProperties.Name` with the
  server ("Sign in to <name>"); page titles and section headers have `AutomationProperties.HeadingLevel`; progress texts are
  `LiveSetting="Polite"`, and because "Connecting…", "Signing in…" and "Waiting for approval…" never change, their peer raises
  `LiveRegionChanged` when they appear (`Announce.Shown`), else Narrator would have nothing to read; the Quick Connect code is
  announced by a UI Automation notification carrying its digit-by-digit text ("Quick Connect code 1 2 3 4 5 6", `Announce.Say`:
  `RaiseNotificationEvent` with `AutomationNotificationKind.Other` and `ImportantMostRecent`, "ImportantMostRecent: … the most
  recent notification supersedes all of the other notifications", AutomationNotificationProcessing; the method is listed for
  Windows App SDK 0.8 to 2.0, AutomationPeer.RaiseNotificationEvent; both learn.microsoft.com, read 2026-10-23) when it appears and
  when a new code replaces it, once each, instead of "Waiting for approval…", so the two do not cut each other off. It is not a
  live region: the owner's runs found the code first unread, then, as a live region, read by its visible text as one number
  ("six hundred ninety-seven …"), not by its name. The same text stays its `AutomationProperties.Name`, for when the user reaches
  it. Not verified by ear: whether Narrator says "6 9 7" as single digits (if not, commas between the digits are the next try),
  and whether focus moving to "Cancel" at the same moment cuts the notification short; found servers are named by
  their server's name on the list item (`ContainerContentChanging`); an error `InfoBar` is closed and opened again when its message changes, because "Once the control is
  open, any changes made to the various properties … will not raise a notification event … we recommend you close and re-open the
  control" (InfoBar). Focus goes to the address box, and to the user name box (the password box when signing in again), once the page is
  loaded. While signing in, the disabled password box (it has no read-only mode) hands focus to "Cancel"; after a cancel or a
  failure focus returns to the form. The address box is read-only, not disabled, while probing, so it keeps focus. Enter
  submits; Esc cancels a running probe, sign-in or Quick Connect (a page `KeyboardAccelerator`) and closes the dialog.

## Windows Community Toolkit: SettingsControls
Owner, 2026-10-23: Microsoft's Windows platform packages, the Windows Community Toolkit included, are approved in advance, and the
owner chose this exact set. Read on nuget.org (registration API and nuspecs) on 2026-10-23.
| Package | Version | Licence | Ships |
|---|---|---|---|
| `CommunityToolkit.WinUI.Controls.SettingsControls` | 8.3.260402-preview2 (preview) | MIT | `.dll` (106 KB), `.pri` (36 KB) |
| `CommunityToolkit.WinUI.Triggers` | 8.3.260402-preview2 | MIT | `.dll` (32 KB), `.pri` |
| `CommunityToolkit.WinUI.Helpers` | 8.3.260402-preview2 | MIT | `.dll` (77 KB), `.pri` |
| `CommunityToolkit.WinUI.Extensions` | 8.3.260402-preview2 | MIT | `.dll` (129 KB), `.pri` |
| `CommunityToolkit.Common` | 8.2.1 | MIT | `.dll` (35 KB) |
- Why a preview: the stable 8.2.251219 (and 8.3.260211-preview1) depend on `Microsoft.WindowsAppSDK >= 1.6.250108002`, the whole
  SDK package, which would resolve to the old all-in-one 1.6 package next to our 2.x components and bring back what the component
  packages left out (Layout). 8.3.260402-preview2 depends only on `Microsoft.WindowsAppSDK.WinUI >= 1.8.260204000`, which our 2.3.9
  satisfies, so NuGet adds no platform package. It was built against WinUI 1.8; the smoke run builds the settings page, so CI shows
  whether it loads with 2.3.9 (it does, below). If it does not, the fallback is hand-built cards (manager, 2026-10-23). Move to 8.3 stable when it
  ships (docs/roadmap.md).
- The packages carry only the SPDX expression, no licence file: `apps/windows/licences/CommunityToolkit-Windows-License.md` (from
  CommunityToolkit/Windows at the package's commit 39c45efe) and `CommunityToolkit-dotnet-License.md` (CommunityToolkit/dotnet at
  `CommunityToolkit.Common`'s commit 2258fd31) ship in `licences\` (`build.ps1`). Both: MIT, "Copyright © .NET Foundation and
  Contributors".
- All five are pinned exactly with NuGet's bracket form (`[8.3.260402-preview2]`, `[8.2.1]`), the transitive ones as direct
  references. The WinUI and InteractiveExperiences references stay minimums, with NuGet's NU1603 notice a warning (Layout): pinning
  them exactly is a follow-up (docs/roadmap.md).
- What ships (`out/files.txt`, CI run 39884007811, 2026-10-23): the five `.dll` files only (34,952 + 106,320 + 129,360 + 76,624 +
  31,568 bytes); the packages' `.pri` files are merged into `Current.pri` and their `.pdb` and `.xml` files are not published. The two
  licence texts ship in `licences\`.
- The same run's smoke run built the settings page, `SettingsCard`s included, with WinUI 2.3.9 ("home, add-server, sign-in and
  settings pages built"). Building is not rendering: the pages are not shown in the smoke run, so how the cards look is the owner's
  check.

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
- Running app: the app holds a named mutex, `Skeksik.Current.Running`, from start to exit, and `AppMutex` names it, so Setup and
  Uninstall see a running Current ("[Setup or Uninstall] has detected that [AppName] is currently running. Please close all
  instances of it now, then click OK to continue, or Cancel to exit.", Inno's AppMutex topic, read 2026-10-22). Without it, the
  owner's run found that uninstalling with the app open neither closed nor warned and left the 47 loaded files behind. The topic
  does not say what happens in a silent run. CI shows it (PR run 39822556373, 2026-10-22): a silent uninstall with the app running
  stops at once and removes nothing; Inno's log says "Defaulting to Cancel for suppressed message box (OK/Cancel): Uninstall has
  detected that Current is currently running." Interactively, the same message lets the user close Current and continue.
  `[UninstallDelete] Type: dirifempty` removes the then empty program folder, which an uninstall after a reinstall over leftovers
  had left.
- `test-install.ps1` (CI, job "App (Windows)"): installs 0.0.1, checks `Current.exe`, the Apps entry's version, the shortcut and its
  `System.AppUserModel.ID`, runs the smoke run from the install; writes `%LOCALAPPDATA%\Current\install-test-marker.txt`; installs
  the workspace version over it and checks again plus the marker; uninstalls silently, waits for the uninstaller (it runs from a temp
  copy) and checks that the program folder, shortcut and Apps entry are gone and the marker remains. Before that final uninstall it
  starts `Current.exe` and runs the silent uninstaller with the app open: it passes if the uninstaller removed nothing (the app
  still running) or removed everything (the app closed), and fails on any partial removal or a hang.
- Not done: code signing (needed for public releases: SmartScreen, the Store's installer route), a licence page (see "Open before first distribution"), the
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
7. on a temporary data folder with secrets in memory: a first `OpenAsync` reports `Created`, a second `Loaded`; the saved servers
   arrive on the UI thread, empty; probing an empty address fails with `InvalidAddress(Empty)` without a request; the folder is
   deleted after;
8. the home, add-server, sign-in and settings pages build (settings uses the toolkit's `SettingsCard`);
9. the network facts and the device name can be read (neither is printed beyond the connectivity level);
10. last, the main window (XAML, Mica, TitleBar, NavigationView, Home) builds and closes: closing the only window ends XAML's message
    loop, so nothing can be awaited after it (CI run 39883272147 stopped at the next await and exited 0 without a result).
No check sends anything over the network; discovery (UDP) is left out.
`smoke.ps1` fails on a non-zero exit code, a missing "smoke run passed" line or a run longer than 120 s.

## Licences of what ships
Every file in `apps/windows/out/app`, from CI run 39815107217 (2026-10-22): 465 files, 179.8 MB. `build.ps1` prints each file with
the package `Current.deps.json` names for it; the native Windows App SDK and WebView2 files that `deps.json` does not list were
matched by name against the contents of the restored packages. The installer adds Inno Setup's `unins000.exe`, `unins000.dat` and
`licences\InnoSetup-license.txt`. Licence and notice files ship in `licences\` (ADR 0003).

| Group | Package (version) | Files | Size | Licence | ADR 0003 |
|---|---|---|---|---|---|
| Current: `Current.exe`, `Current.dll`, `Current.pri`, `Current.deps.json`, `Current.runtimeconfig.json`, `Current.Bindings.dll`, two `.pdb`, `current_ffi.dll` | ours | 9 | 3.9 MB | Proprietary (ours). `current_ffi.dll` statically links the workspace's Rust crates, checked by `cargo deny` (`deny.toml`), with UniFFI under the MPL-2.0 exception | covered |
| .NET runtime: `coreclr.dll`, `System.*.dll`, `hostfxr.dll`, `msquic.dll` and the rest | `Microsoft.NETCore.App.Runtime.win-x64`, the runtime pack of the SDK that builds it (10.0.12 in that CI run; 10.0.13 in the owner's build). `build.ps1` prints the version and names the licence files after it | 187 | 80.4 MB | MIT, with `THIRD-PARTY-NOTICES.TXT` | allowed (MIT) |
| WinUI: `Microsoft.ui.xaml.dll`, `Microsoft.UI.Xaml.Controls.dll`, `Microsoft.WinUI.dll`, `WinUIEdit.dll`, `*.mui` (86 languages x 2), `Microsoft.UI.Xaml\Assets` | `Microsoft.WindowsAppSDK.WinUI` 2.3.9 | 185 | 43.6 MB | Microsoft Software License Terms, Windows App SDK | platform runtime (owner) |
| Composition, input and windowing: `Microsoft.UI*.dll`, `dcompi.dll`, `dwmcorei.dll`, `DwmSceneI.dll`, `CoreMessagingXP.dll`, `marshal.dll`, `wuceffectsi.dll`, `Microsoft.InteractiveExperiences.Projection.dll` | `Microsoft.WindowsAppSDK.InteractiveExperiences` 2.1.9 | 21 | 18.0 MB | same | platform runtime (owner) |
| Runtime, lifecycle and resources: `Microsoft.WindowsAppRuntime*.dll`, `MRM.dll`, `RestartAgent.exe`, `Microsoft.Windows.*.winmd` and projections | `Microsoft.WindowsAppSDK.Foundation` 2.3.12 | 47 | 5.0 MB | same | platform runtime (owner) |
| Windows SDK projection: `Microsoft.Windows.SDK.NET.dll`, `WinRT.Runtime.dll` | `Microsoft.Windows.SDK.NET.Ref` 10.0.26100.57 | 2 | 26.9 MB | Microsoft Software License Terms, Windows SDK for Windows 10; on its redist list | platform runtime (owner) |
| WebView2: `WebView2Loader.dll`, `Microsoft.Web.WebView2.Core.dll`, `Microsoft.Web.WebView2.Core.Projection.dll` | `Microsoft.Web.WebView2` 1.0.3719.77 (a WinUI dependency) | 3 | 1.6 MB | BSD-3-Clause | allowed (BSD) |
| Windows Community Toolkit: `CommunityToolkit.WinUI.Controls.SettingsControls.dll`, `…Triggers.dll`, `…Helpers.dll`, `…Extensions.dll`, `CommunityToolkit.Common.dll` (added in M1, CI run 39884007811; their `.pri` resources merge into `Current.pri`) | `CommunityToolkit.WinUI.*` 8.3.260402-preview2, `CommunityToolkit.Common` 8.2.1 | 5 | 0.4 MB | MIT | allowed (MIT; Microsoft's platform packages pre-approved, owner 2026-10-23) |
| Licence and notice files | (from the packages above, plus `apps/windows/licences/WindowsSDK-projection.txt` and the two Community Toolkit texts) | 13 | 0.5 MB | | |
| Uninstaller: `unins000.exe`, `unins000.dat`, `InnoSetup-license.txt` | Inno Setup 7.1.0 | 3 | | Inno Setup License | allowed (owner, ADR 0009) |

- No AI, ML (onnxruntime), Widgets or Search files ship (they came with the metapackage, see Layout).
- No shipped file is LGPL or GPL as far as the notices show: the WinUI and WebView2 `NOTICE.txt` files list only MIT components
  (Newtonsoft.Json); the Base package's `NOTICE.txt` lists onnxruntime's components (onnx, protobuf, Eigen and others), which belong
  to the ML runtime we do not ship. Not checked binary by binary.
- The `.pdb` files of our own two assemblies ship too (small; useful for crash reports). Removing them is a release-time choice.

## Open before first distribution
Nothing built here goes to anyone else, testers included, until these are done (owner, 2026-10-22; ADR 0003, ADR 0009):
1. End-user terms that protect Microsoft's code at least as much as the Windows App SDK licence (its section 3(b)(ii)) and the
   Windows SDK licence (for the .NET projection) do; the owner provides them.
2. An installer licence page showing those terms (Inno's `LicenseFile`).
3. The indemnities in the Windows App SDK licence's 3(b)(iii) and in the Windows SDK licence: the owner's to accept.
4. The Windows App SDK's telemetry (its licence's 2(a)): check what it sends and which options turn it off.
5. Code signing of the app and the installer (SmartScreen; the Store's installer route).
6. Inno Setup's commercial licence: its authors request (do not require) one from for-profit users with more than USD 5,000 annual
   revenue, for any version (jrsoftware.org/isorder.php, read 2026-10-22). The owner decides.
7. A copyright notice shown in the app (the Windows SDK licence: "Display your valid copyright notice on your programs"): done in
   this PR, "© 2026 Skeksik" under the version in Settings.

## What the owner checks by hand
On the owner's PC, with an installer built locally by the commands above (CI keeps no installer: there is no approved
upload-artifact step in `ci.yml`), or `apps/windows/out/app/Current.exe`:
1. The window has Mica, the title bar is part of the window, it drags and double-click maximises; the caption buttons and snap layouts
   (hover over maximise) work.
2. Switch Windows between light and dark: the app follows at once.
3. Home shows the empty state. Settings shows the version, "© 2026 Skeksik", "No servers are set up." and "Copy diagnostics"; paste the clipboard
   somewhere: the text is there, also after closing the app.
4. Back and forward: go Home -> Settings, then back with the title bar's back button, Alt+Left, and a mouse Back button; each
   returns to Home, and at Home with an empty back stack the back button is disabled (after Home -> Settings -> Home it stays
   enabled, as in Windows Settings). After going back, Alt+Right and a mouse Forward button return to
   Settings. Narrow the window until the pane collapses: the pane toggle in the title bar opens it.
5. Keyboard only: Tab reaches the back button, the pane and the settings controls; Narrator reads them by name.
6. Start a second copy (from Start or the exe): no second window; the first one comes to the front, also when minimised.
7. Taskbar and Start show "Current" with the app's name (not "Current.exe"); pin to the taskbar and start from the pin.

## M1 screens: what the owner checks
With a Jellyfin server on the owner's network (nothing about it goes into the repo), on an installer or `out/app` build from the M1
PR, starting with no `%LOCALAPPDATA%\Current` folder:
1. First start: Home says "No servers yet" with "Add a server". Settings → Accounts says "No servers are set up.".
2. Add a server: the server appears under "Servers on your network" within about 2 s. Note whether Windows Defender Firewall shows a
   prompt when the search starts (ADR 0016 leaves this not verified).
3. Pick it: the sign-in page opens ("Sign in to <name>"). First a wrong password: "The user name or password is incorrect." Then the
   right one with Enter: Home shows the server with "Signed in as <user>", the version and "Connected".
4. Quit and start again: Home shows it signed in without asking; Settings → Accounts the same.
5. Settings → Accounts → Sign out: "Signed out" at once; the server's dashboard (Devices) no longer lists this PC. The device name
   there is the PC's name.
6. Sign in again from the card with Quick Connect: the code is large and readable; approve it from another Jellyfin client: signed
   in. Try once more and press Cancel (and Esc): it stops and the form is back.
7. Type the address instead of picking: an address that does not answer (wording), `https://` to a server whose certificate this PC
   does not trust (wording), and, if available, a plain `http://` address from outside the LAN (the "Connect without encryption?"
   dialog; Enter and Esc both cancel).
8. With the app open, turn Wi-Fi off or unplug the network: the card shows "Can't reach the server"; reconnect: "Connected" again.
   Minimise, change network, restore: it re-checks at once.
9. Narrator (Win+Ctrl+Enter) through add-server and sign-in: every box and button is read by name, errors are read when they
   appear, the Quick Connect code is read digit by digit. Keyboard only: Tab order follows the page, Enter submits, Esc cancels.
10. Settings: the cards look like Windows Settings' cards (SettingsCard); About opens to show "© 2026 Skeksik". "Copy diagnostics"
    now copies the version, platform and settings state even without requests.
11. Narrator on, start Quick Connect: the code is read once, digit by digit ("Quick Connect code 6 9 7 …", not "six hundred
    ninety-seven"); Cancel and start it again: the new code is read once more (see "Quick Connect code and Narrator" below).

## Windows shell: owner's run
2026-10-22, head 4e56bcd, Windows 11 Pro 25H2 (build 26200.9711), x64. Built locally as above, since CI keeps no installer artifact:
Rust 1.99.1, .NET SDK 10.0.402 (runtime pack 10.0.13; the licence table above has 10.0.12), MSVC from Visual Studio 2026,
uniffi-bindgen-cs v0.11.1+v0.31.0, Inno Setup 7.1.0 (SHA-256 as in CI). C# tests 12/12, smoke run passed,
`Current-0.1.0-x64-setup.exe` 45.5 MB. The owner looked; Claude guided and read the install state, the event log and the clipboard.
1. Install (normal setup): **pass.** No admin prompt; `Current.exe` in `%LOCALAPPDATA%\Programs\Current`; Apps entry "Current
   0.1.0"; Start shows "Current", and its shortcut has `System.AppUserModel.ID` `Skeksik.Current`.
2. Window: **pass.** Mica, title bar part of the window, drag, double-click maximises, caption buttons, snap layouts.
3. Light/dark: **pass.** Follows at once both ways.
4. Home and Settings: **fail.** Home's empty state, the version, "© 2026 Skeksik" and "No servers are set up." are there, but
   "Copy diagnostics" crashes the app on every click (3 of 3). Event log: `Microsoft.UI.Xaml.dll`, exception 0xc000027b (stowed
   exception), WER signature `combase.dll` / `800401d0` (`CLIPBRD_E_CANT_OPEN`). The clipboard is empty afterwards, so `SetContent`
   took it and the unguarded `Clipboard.Flush()` in `SettingsPage.OnCopyDiagnostics` threw. Cause not verified: Windows clipboard
   history is on and no other clipboard tool runs; the same `SetContent` + `Flush` from a Windows PowerShell STA succeeded 20 of 20.
   Suggested: catch the `COMException`, retry `Flush` briefly, and show an error instead of crashing.
5. Back and forward: **pass.** Title bar back button, Alt+Left, mouse Back; Alt+Right and mouse Forward; the pane toggle when
   collapsed. Back is disabled at Home only when the back stack is empty (Home -> Settings -> Home keeps it enabled), as in Windows
   Settings: expected.
6. Keyboard and Narrator: **pass.** Tab reaches the back button, the pane and the settings controls, and Narrator names them. While
   tabbing Narrator reads only focusable controls, not the empty-state text (its scan mode reads that); not a defect.
7. Single instance: **pass.** From Start and from the exe, also when minimised: no second window, the first comes to the front.
8. Taskbar and Start: **pass.** "Current" in both; pinned and started from the pin.
9. Uninstall from Settings > Apps: **pass with the app closed; finding with it open.** Closed: program folder, Start shortcut and
   Apps entry gone; a marker file in `%LOCALAPPDATA%\Current` stays (a fresh install plus silent uninstall gave the same). Open (the
   copy started from the pin): the uninstaller neither closed Current nor warned (the Restart Manager logged a session only for the
   install), reported success and left the program folder with the 47 files the running process had loaded. After a reinstall over
   that folder, the next uninstall removed every file but left the empty folder. Inno's `AppMutex`, with a named mutex the app
   creates, would make setup and uninstall detect the running app (not verified here).

## Windows shell: owner's re-check
2026-10-22, head 71b6c4d, same PC and tools as the owner's run; C# tests 12/12, smoke run passed, setup 45.5 MB, installed with the
normal setup. Only cases 4 and 9.
4. Copy diagnostics: **pass.** 10 clicks with clipboard history on (5 spaced, 5 in a quick burst): no crash, no event log entry,
   "Diagnostics copied to the clipboard." every time, the error InfoBar never. Before each spaced click a marker was put on the
   clipboard; each click replaced it, and the text stayed after Current exited. With no servers the text is empty (the request log
   is empty, as `App::diagnostics_text` is designed), so the paste shows nothing yet.
9. Uninstall with Current open: **pass.** Settings > Apps: after the uninstaller's confirmation, "Uninstall has detected that
   Current is currently running." OK with Current still open asks again; Cancel removed nothing, and Current started again from
   Start. After closing Current, OK: the program folder (the folder itself too), the Start entry and the Apps entry are gone.
   `%LOCALAPPDATA%\Current` did not exist (nothing writes there yet), so its staying was not seen here (the install test checks it).
   Running the setup with Current open gives "Setup has detected that Current is currently running."; Cancel exits it.

## M1 Windows run (2026-10-23)
Head a10652a, Windows 10.0.26200.9711, x64, the same PC and tools as the owner's run (Rust 1.99.1, .NET SDK 10.0.402,
uniffi-bindgen-cs 0.11.1+v0.31.0, Inno Setup 7.1.0). Built locally with the three scripts; smoke run passed; setup 47.1 MB, built
but not installed: the cases ran `out/app/Current.exe`. No `%LOCALAPPDATA%\Current` before the first start. The owner's server, on
the LAN; the owner looked and typed, Claude guided and read the data folder, firewall state and the network from a shell.

M1's "done when" (sign in and out, restart signed in) **holds**: cases 3, 4 and 5 pass. Cases 2 and 9 each found a defect.
1. Fresh start: **pass.** "No servers yet" with "Add a server"; Settings → Accounts "No servers are set up.".
2. Add a server: **fail.** The first search found nothing ("No servers found on this network. Enter the address instead."). Search
   again found the server while the PC was on Wi-Fi, never while on the wired network (same LAN). No firewall prompt: no rule for
   `Current.exe` exists afterwards and the firewall log since start has only Windows' own Teredo rule changes. Cause, from a shell
   sending the same `who is JellyfinServer?` datagram: from an unbound socket to `255.255.255.255:7359`, 1 reply in 9 tries; bound
   to the wired or the Wi-Fi address, 1 reply every time. This PC has Wi-Fi and wired on the same LAN plus WSL/Hyper-V and two VPN
   adapters; the lowest-metric route for `255.255.255.255` is a Hyper-V virtual adapter, so the one limited broadcast
   does not reliably leave on the LAN. Suggested: send the request from each up interface (or to each subnet broadcast), as ADR
   0016 anticipates for per-interface addresses from the platform.
3. Sign in: **pass.** Wrong password: "The user name or password is incorrect."; right one with Enter: "Signed in as …", the
   version and "Connected". The data folder then holds only `settings.json`, with no token or password in it.
4. Quit and restart: **pass.** Closing the window ended the process; started again: signed in, no prompt.
5. Sign out: **pass.** "Signed out" at once. While signed in the dashboard's Devices showed this PC's name and "Current" + version;
   after sign-out the device was gone.
6. Quick Connect: **pass.** Code readable, approval from another client signs in; Cancel and Esc both stop it.
7. Typed addresses: **pass, with a finding.** No answer, an untrusted https certificate and plain http from outside the LAN (Enter
   and Esc both cancel the dialog) all as expected. Finding: `host:port` without a scheme, for a port where another, non-Jellyfin
   web app speaks plain http, says "The secure connection failed. The server's certificate may not be trusted by this PC."; with
   `https://` it correctly says it isn't a Jellyfin server. Without a scheme both https and http are tried on that port
   (`address.rs`); https fails its handshake (`Tls`) and http gets `NotJellyfin`, and `ProbeError::rank` puts `Tls` (4) above
   `NotJellyfin` (3). Suggested (core): when plain http on the same port answered, prefer its result over the https handshake
   failure.
8. Network changes: **pass.** Network dropped: "Can't reach the server"; back: "Connected"; minimise, change network (phone
   hotspot), restore: re-checked at once. Four addresses were saved by the end (1: LAN IP, 2: DNS name, 3: LAN IP, all http;
   4: DNS name, https, added during the run). On the LAN Home showed "address 2 of 4", on the hotspot "address 4 of 4". Worth a look:
   on the LAN it used the DNS name, not one of the LAN IPs.
9. Narrator and keyboard: **fail (one part).** Field names and errors are read, Tab order, Enter and Esc work, Alt+Left/Right after
   sign-in never land on a dead sign-in page. The Quick Connect code is not read: `CodeText` gets the digit-by-digit name
   (`SignInPage.xaml.cs`), but only `WaitingText` is announced when the panel shows, and the code itself is never announced.
   Suggested: announce the code (by its digit-by-digit name) when it appears.
10. Settings: **pass.** Windows Settings-style cards; About "© 2026 Skeksik"; Copy diagnostics copies text with the version; pasted
    into Notepad (not saved) it had no address, user name or token.

## Quick Connect code and Narrator: owner's check
For fix/narrator-quick-connect-code (the code announced by a notification, see "Keyboard and Narrator" under Screens), on a build
of that branch, signed out of a server that offers Quick Connect:
1. Narrator on (Win+Ctrl+Enter); open the sign-in page and press "Use Quick Connect". The code is read digit by digit, once, and
   not again as a number. Note whether "Cancel, button" is also read and whether it cuts the code short.
2. Press Cancel, then "Use Quick Connect" again: the new code is read digit by digit, once.
3. In scan mode (Caps Lock+Space), arrow to the code: Narrator reads "Quick Connect code" and the digits.
