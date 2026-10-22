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
- **Home:** the empty state before any server exists: "No servers yet". Server setup is M1.
- **Settings:** the core's version (`CoreHost.AppInfo`) with "© 2026 Skeksik" under it (plain text; the Windows SDK licence asks for a
  copyright notice; no licences screen), the capabilities state through `ObserveCapabilities` (empty: "No servers are
  set up."), and "Copy diagnostics", which puts `DiagnosticsText()` on the clipboard (`Clipboard.SetContent`, then `Flush` so it
  stays after the app exits) and shows a success `InfoBar`. The clipboard is shared, and Windows refuses it with
  `CLIPBRD_E_CANT_OPEN` while another process has it open ("OpenClipboard fails if another window has the clipboard open",
  OpenClipboard, learn.microsoft.com, read 2026-10-22); the owner's run hit that in `Flush` with clipboard history on, and the
  unguarded call ended the app. `ClipboardText.TryCopyAsync` now retries `SetContent` and `Flush` up to 5 times, 100 ms apart (our
  choice: Microsoft's `Clipboard.Flush` page gives no retry guidance), and the page shows an error `InfoBar` if the clipboard stays
  busy. The handler is `async void` and catches everything itself; `Application.UnhandledException` logs and marks handled any
  exception that still escapes a UI event handler, so none ends the process. The smoke run copies a marker and reads it back; the
  refused-clipboard case itself cannot be produced in CI. The subscription starts on the page's `Loaded` and is disposed on
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
7. the main window (XAML, Mica, TitleBar, NavigationView, Home) builds and closes.
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
| Licence and notice files | (from the packages above, plus `apps/windows/licences/WindowsSDK-projection.txt`) | 11 | 0.5 MB | | |
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
