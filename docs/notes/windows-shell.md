# Windows app shell (D5b)

The WinUI 3 app in `apps/windows`: its layout, folders, identity, single instance, the smoke run, and what the owner checks by hand.
Dates are when a source was read.

## Layout
```
apps/windows/
  Current.Shell/          the app (WinUI 3, net10.0-windows10.0.26100.0, unpackaged, self-contained, x64); builds Current.exe
    Program.cs            own Main: AppUserModelID, single instance, then XAML
    AppIdentity.cs        AppUserModelID, single-instance key, data folder
    App.xaml(.cs)         owns the CoreHost (one per process) and the main window; the page title and column styles
    MainWindow.xaml(.cs)  Mica, TitleBar, NavigationView over one Frame, back navigation, pane, minimum size
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
  the system caption buttons stay the system's, with their glyphs and their hover and pressed backgrounds in the app's theme
  (below). Theme: the user's choice in Settings, "Windows default" (the Windows light/dark setting) until one is made ("App theme",
  below).
- **Caption button colours (owner's new finding 3, 2026-10-24).** WinUI sets only the caption buttons' `ButtonBackgroundColor` and
  `ButtonInactiveBackgroundColor` (transparent) and leaves the rest unset (read from `AppWindow.TitleBar` in a throwaway build). The
  system then drew a light theme's hover in dark mode, #191919 on the #202020 title bar, from the start, and kept it after a switch
  to light while the app ran; after a restart in light it was #E5E5E5. `AppWindowTitleBar.PreferredTheme` (`UseDefaultAppMode`,
  "The Windows default app mode theme is used", and `Dark`; learn.microsoft.com, read 2026-10-24) did not change it. Photos and
  Windows Settings hover at #2D2D2D dark and #EAEAEA light (Settings #E9E9E9; measured on the owner's PC, 2026-10-24), which is
  WinUI's `SubtleFillColorSecondary` (#0FFFFFFF dark, #09000000 light) over the title bar (`Common_themeresources_any.xaml`,
  microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24). `MainWindow.ApplyCaptionButtonColors` sets
  `ButtonHoverBackgroundColor` to it and `ButtonPressedBackgroundColor` to `SubtleFillColorTertiary`, on load and on every
  `ActualThemeChanged`, as the WinUI Gallery re-applies its caption colours "as AppWindow TitleBar doesn't update caption button
  colors correctly when changed while app is running" (`TitleBarHelper.cs`, main at 14061ee, read 2026-10-24; the Gallery uses its
  own fixed colours). Measured after the change: #2D2D2D dark, #EAEAEA light, after a live switch and after a restart. The pressed
  colour is not measured. High contrast resets both to the system's own; `AccessibilitySettings.HighContrastChanged` cannot be
  subscribed to in a desktop window (`COMException` 0x80070490 in the smoke run), so a switch to high contrast while the app runs is
  caught only if it changes `ActualTheme` (not verified).
- **Tall title bar (48 px).** The owner found the title bar lower than Windows Settings', which with a back button uses the taller
  one (owner's run, 2026-10-24; its exact height is not verified). "Windows app title bar" (learn.microsoft.com, ms.date 2024-07-31,
  read 2026-10-24): "The standard title bar has a height of 32px", and "Increase the size of the title bar to 48px" when it holds a
  search box or a person picture. The caption buttons are as tall as the title bar, 48 × 48 as Photos' (owner's new finding 1,
  2026-10-24, measured on the owner's PC): "Set PreferredHeightOption to Tall so the caption buttons are the same height as the
  title bar" ("Title bar customization", ms.date 2026-10-13, read 2026-10-24), as the WinUI Gallery's `MainWindow` does after
  `ExtendsContentIntoTitleBar` (main at 14061ee, read 2026-10-24). An earlier version kept them Standard, 46 × 32 as Windows
  Settings' (owner's run, 2026-10-24); the owner chose Photos' (2026-10-24). The `TitleBar`
  control's layout root is `TitleBarCompactHeight` (32) and becomes `TitleBarExpandedHeight` (48) only when `Content`, `LeftHeader`
  or `RightHeader` is set (`TitleBar.xaml`, `UpdateHeight` in `TitleBar.cpp`, microsoft-ui-xaml, tag winui3/release/2.5.1, read
  2026-10-24). Ours has none, so `MainWindow.xaml` raises `TitleBarCompactHeight` to 48 on `AppTitleBar` (lightweight styling).
  The guideline's standard is 32 px, and 48 only with a search box or a person picture; 48 here is the owner's choice (owner,
  2026-10-24).
- **Back button inset (owner's finding 1, 2026-10-24).** "Windows app title bar" (ms.date 2024-07-31, read 2026-10-24): "The back
  button should be 16px from the left-most border in LTR, or right-most border in RTL", "16px by 16px" with "a full bleed
  backplate". The TitleBar's back button is 40 px wide with a 2 px margin (`TitleBarBackButtonStyle`), and `UpdatePadding` sets the
  left padding column to `AppWindowTitleBar.LeftInset` in place of `TitleBarLeftPaddingWidth` (2) (`TitleBar_themeresources.xaml`,
  `TitleBar.cpp`, microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24). With an inset of 0 the backplate started 2 px from
  the border and the glyph 14 px; `Margin="2,0,0,0"` on `AppTitleBar` puts the backplate at 4 px and the glyph at the guideline's
  16. A property of the control, not a resource override. That the left inset is 0 in a left-to-right window is not verified on
  screen; the 2 px left of the title bar are not part of its drag region. Photos shows no back button in its title bar; Windows
  Settings' has the same 4 px inset.
  - Height (owner's new finding 1, 2026-10-24): the control's styles stretch the back and pane buttons to the bar's height less a
    2 px margin (`TitleBarBackButtonStyle`, `TitleBarPaneToggleButtonStyle`, `VerticalAlignment` Stretch, `Margin` 2; same file),
    40 × 44 in a 48 px bar, touching the border. Windows Settings' back button is 40 × 36, 6 px below the top of its 48 px title bar
    (measured on the owner's PC, 2026-10-24). `MainWindow.xaml` bases both styles on the control's own and sets `Margin` to 2,6:
    40 × 36, 6 px from the top, as Settings'.
- **Minimum window size: 500 × 500 effective px** (manager, 2026-10-24): the narrowest page column (Sign in, 400 px plus 24 on
  each side) and the title bar (back and pane buttons, title, caption buttons) fit. `OverlappedPresenter.PreferredMinimumWidth` and
  `PreferredMinimumHeight` ("Gets or sets the preferred minimum width for the window", learn.microsoft.com, Windows App SDK 1.7 and
  later, read 2026-10-24); `Window.MinWidth` exists only in the 2.0 experimental API, so it is not used. The API page names no unit;
  the WinUI Gallery's `WindowHelper.SetWindowMinSize` multiplies by `XamlRoot.RasterizationScale` (physical pixels), and so does
  `MainWindow`, again on every `XamlRoot.Changed` so a move to a display with another scale keeps 500 effective px (the unit is not
  verified beyond that sample). The Gallery itself uses 640 × 500; Windows Settings' own minimum is not verified.
- **Navigation:** a left `NavigationView` with Home and the built-in Settings item over one `Frame`, as in the NavigationView page's
  example (learn.microsoft.com/windows/apps/develop/ui/controls/navigationview, ms.date 2026-10-02, read 2026-10-22): navigate on
  `ItemInvoked` only when the page differs (no duplicate back stack entries), keep the selection and header in step on
  `Frame.Navigated` (the settings item via `SettingsItem`). The pane open over the page (Compact) counts as a back
  step (owner, 2026-10-24): back then only closes the pane, from the title bar's Back button, Alt+Left, the GoBack key and the
  mouse Back button alike, as the control's own back button does ("clicking the back button will close the Pane and raise the
  PaneClosing event instead", same page, read 2026-10-24); with the pane closed, back goes back. The title bar's Back button is
  enabled while the Frame can go back or the pane overlays the page, so it also closes the pane on a page with no history (Home at
  start); `MainWindow.UpdateBackButton` runs on navigation, and on `PaneOpening`, `PaneOpened`, `PaneClosed` and
  `DisplayModeChanged` queued on the dispatcher, so Back is enabled as the pane starts to open, as Windows Settings' is (owner's
  recording, 2026-10-24). Read at once, crossing 1008 px it saw Compact while the full pane was still open (the navigation view
  closes it after raising `DisplayModeChanged`) and Back flashed enabled for up to 16 ms (owner's recording; a traced build logged
  it on every crossing, the queued update on none, 2026-10-24). The full pane
  inline on a wide window is not a back step. The first version followed the example, which does not go back while the pane
  overlays the content, and left the title bar's Back button without effect and Alt+Left only closing the pane (owner's run,
  2026-10-24). Closing the pane this way counts as the navigation view's forced close, so after it a medium window widened to wide
  keeps the rail (see "Pane" below).
- **Back button in the title bar.** The TitleBar page (learn.microsoft.com/windows/apps/develop/ui/controls/title-bar, ms.date
  2026-10-13, read 2026-10-22): "Fluent Design guidance recommends that these controls be placed in the title bar when a custom title
  bar is used." So the NavigationView's back and pane toggle buttons are hidden and the TitleBar's are used, enabled from
  `Frame.CanGoBack`.
- **Pane: never hidden (owner's finding 2, 2026-10-24).** Always the full pane or the icon rail, with the title bar's pane button
  always shown: on a wide window it switches between the full pane and the rail inline, on a narrower one the rail shows and the
  pane opens over the page. This is the NavigationView page's "Compact" adaptive pattern (learn.microsoft.com, ms.date 2026-10-02,
  read 2026-10-24): "an expanded left pane on large window widths, and a LeftCompact, icon-only, nav pane on both medium and small
  window widths", recommended when "It is important to always show all navigation options on screen" and "Your navigation
  categories can be clearly represented with icons"; "To configure this behavior, set CompactModeThresholdWidth to 0." So
  `PaneDisplayMode="Auto"` with `CompactModeThresholdWidth="0"`; Expanded from 1008 px (the default `ExpandedModeThresholdWidth`).
  In the template (`UpdateVisualStateForDisplayModeGroup`, `NavigationView.cpp`, microsoft-ui-xaml, tag winui3/release/2.5.1, read
  2026-10-24) Expanded is a `CompactInline` SplitView, so closing the pane there leaves the icon rail and the content moves with
  it; Compact is `CompactOverlay`, so the pane opened there covers the content. Minimal is never reached, so `MainWindow` counts
  only Compact as the pane over the page. The earlier version used the default thresholds (Minimal below 641 px), which hid the
  pane on a small window. The title bar's pane button flips `NavView.IsPaneOpen` (the WinUI Gallery shows it in every left mode,
  `OnPaneDisplayModeChanged`, github.com/microsoft/WinUI-Gallery, main at 14061ee, read 2026-10-24).
  - Width of the full pane (owner's finding 3): `OpenPaneLength`, matched to Windows Settings' full pane (owner, 2026-10-24). No
    Microsoft guideline gives a value: the NavigationView page names none, the control's default is 320 (`OpenPaneLength`,
    learn.microsoft.com, read 2026-10-24), and the WinUI Gallery keeps the default. So the value comes from Windows Settings:
    **300 px**, measured by the owner (2026-10-24, Windows 11 build 26300, 150%): Settings' `SplitView` pane `PaneRoot` is 300
    effective px wide, its content starting at x = 300, with no border between pane and content, and its nav items inset 16.7 px
    and 279 px wide. Windows Settings itself has no icon rail (its full pane down to about 800 px, then only a menu button); the
    owner's rule (never hidden) is kept anyway.
  - Remembering a closed pane: the navigation view does it for the run. Closing the pane through `IsPaneOpen` while it is open
    sets `m_wasForceClosed` (`OnIsPaneOpenChanged`), and on entering Expanded it reopens the pane only without that flag
    (`UpdateAdaptiveLayout`); a close caused by resizing does not set it, and changing `PaneDisplayMode` clears it (same file).
    Nothing is stored between runs, and no code of ours is involved. One consequence, as with the navigation view's own button
    (`OnPaneToggleButtonClick`): opening the pane over the content on a medium window and closing it with the button keeps the
    rail when the window is widened; closing it by clicking the page (light dismiss) does not.
  - The button is named as the NavigationView names its own, "Open Navigation" while the pane is closed and "Close Navigation"
    while it is open (`NavigationButtonClosedName` and `NavigationButtonOpenName`, NavigationView's `Resources.resw`), with the
    same tooltip (manager, 2026-10-24). The TitleBar names it "Toggle Navigation" whatever the state (`NavigationButtonToggleName`,
    `LoadPaneToggleButton` in `TitleBar.cpp`, same tag) and has no property for it, so `MainWindow` looks up its template part
    `PART_PaneToggleButton` after loading and sets the name with Back (`PaneOpening`, `PaneOpened`, `PaneClosed` and
    `DisplayModeChanged`, queued; see "Navigation"). A future WinUI that renames that part leaves the default name; nothing fails.
- **Pane opened over the page: the control's own acrylic (owner's finding 7, 2026-10-24).** The NavigationView page (ms.date
  2026-10-02, read 2026-10-24): "the pane uses in-app acrylic when open as an overlay on top of content (in Top, Minimal, or Compact
  mode)", through `NavigationViewDefaultPaneBackground`, which is `AcrylicInAppFillColorDefaultBrush`; the full pane inline uses
  `NavigationViewExpandedPaneBackground` (`SolidBackgroundFillColorTransparent`, so Mica) (`NavigationView_themeresources.xaml`,
  microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24). Nothing is overridden. The earlier version made the overlay pane
  solid (`SolidBackgroundFillColorBaseBrush`) and set `NavigationViewBorderThickness` to 0,0,1,0; both overrides are gone.
  - The 1 px line at its edges is the overlay pane's own border. The template gives `RootSplitView`
    `BorderThickness="{ThemeResource NavigationViewBorderThickness}"` (1 on every side) and
    `BorderBrush="{ThemeResource NavigationViewItemSeparatorForeground}"` (`NavigationView.xaml`, same tag), and the SplitView draws
    it around its pane while the pane overlays the page. With the navigation view under the title bar (below) the pane, its acrylic
    and that line reach the window's top: the line runs up the pane's right edge, and its top row is barely visible (dark #323232
    over #2E2E2E acrylic, owner's PC, 2026-10-24). With the TitleBar above the navigation view the line sat under the title bar,
    which the owner disliked earlier. It stays, as the control's default look (owner, 2026-10-24).
- **The navigation view fills the window, under the title bar (owner's new finding 2, 2026-10-24).** With the pane open over the
  page, the title bar's back and pane buttons showed no hover or pressed state and a click on them only closed the pane, which
  would also break any future control in the title bar (a search box; owner). The cause: a SplitView whose pane can be
  light-dismissed puts a popup "outer dismiss layer" over everything outside its own bounds while the pane is open, and skips it
  only if "the SplitView doesn't need any of the 4 elements, then it takes up the entire window" (`SetupOuterDismissLayer`,
  `SplitView_Partial.cpp`, microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24). With the TitleBar above the
  NavigationView, as the NavigationView page recommends ("place a TitleBar control above the NavigationView", "the pattern used by
  the WinUI 3 Gallery app"), the title bar was under that layer. Windows Settings keeps its title bar buttons working with the pane
  open (owner's recording, 2026-10-24). So the NavigationView spans the whole window and the TitleBar is above it in z-order
  (`Canvas.ZIndex`). The navigation view's own top padding for a title bar is set only on UWP (`UpdateTitleBarPadding` in
  `NavigationView.cpp`: "null in win32", same tag), so the pane's items and the content layer start below the 48 px title bar
  through the template's own margins, `NavigationViewPaneContentGridMargin` (-1,3 → -1,51,-1,3) and `NavigationViewContentMargin`
  (0 → 0,48,0,0) (`NavigationView_themeresources.xaml`, same tag). Measured afterwards: the title, content layer and colours as
  before (Windows run below); the pane's background now reaches the window's top behind the title bar.
- **One content background on every page (owner's finding 4, 2026-10-24).** Settings included: no page opts out. "Mica material"
  (learn.microsoft.com, ms.date 2026-07-22, read 2026-10-24): "The content layer should pick up the material behind it, Mica, using
  the `LayerFillColorDefaultBrush`, a low-opacity solid color, as its background", and "By default, NavigationView in Left mode
  includes the content layer in its content area". That layer is the template part `ContentGrid` (the `Content` of
  `RootSplitView`), with `NavigationViewContentBackground` (`LayerFillColorDefaultBrush`: #80FFFFFF light, #4C3A3A3A dark),
  `NavigationViewContentGridBorderBrush` (`CardStrokeColorDefaultBrush`), a 1,1,0,0 border and an 8,0,0,0 corner
  (`NavigationView.xaml`, `NavigationView_themeresources.xaml`, `Common_themeresources_any.xaml`, microsoft-ui-xaml, tag
  winui3/release/2.5.1, read 2026-10-24); the Minimal state, never reached now, would set the border to 0,1,0,0.
  - It matches the Photos app's content area, so it is kept with no override. Measured by the owner (2026-10-24, Windows 11 build
    26300, 150%, dark wallpaper, constant across 7 window positions): Photos' content is #F9F9F9 light and #272727 dark over a pane
    and backdrop of #F3F3F3 and #202020, a separate layer with a 1 px stroke at its left and top (#E5E5E5 light, #1D1D1D dark) and
    an 8 px rounded top-left corner. The owner's run of 2026-10-24 measured this layer at exactly those colours and that stroke.
    The owner had found the content "too dark"; that came from Settings showing no layer next to the content pages, which is gone.
  - The earlier version switched the layer off for Settings in code (`PageLayout.HasContentLayer`, `MainWindow.ApplyContentLayer`);
    that switch is gone.
  - The smoke run shows the window and reads `ContentGrid`'s background on Home and again after opening Settings: a brush with
    some opacity, the same on both. If a WinUI version renames `RootSplitView` or `PART_PaneToggleButton`, nothing fails at run
    time: `MainWindow` traces the part's name once loaded (`Trace.TraceWarning`), and the smoke run fails on the missing layer.
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
- The window shows as soon as the core is open, or after 300 ms if opening takes longer, so the saved app theme is applied before
  the window is first drawn ("App theme", below); `App.OpenCoreAsync` then awaits `CoreHost.OpenAsync(AppIdentity.DataFolder, DeviceName.Get(),
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
- **Page layout (owner's findings 5, 6 and 8, 2026-10-24):** on every page the title sits over one column, and both are centred
  together up to a maximum width; the navigation view's header is not used. This replaces the earlier rules (titles and columns
  left-aligned and placed by `PageLayout.cs`, with per-page offsets and the navigation view's header margins set to 0), which are
  gone with that file. The owner's rule: the controls' defaults first, and every size from Microsoft's guidelines, else from
  Microsoft's own apps, saying which.
  - Pattern: the WinUI Gallery's settings page (`Pages/SettingsPage.xaml`, github.com/microsoft/WinUI-Gallery, main at 14061ee,
    read 2026-10-24), which "Guidelines for app settings" (learn.microsoft.com, ms.date 2026-04-08, read 2026-10-24) names as the
    "complete implementation" example: a `TextBlock` title (`TitleTextBlockStyle`, `HeadingLevel` Level1, `MaxWidth="1064"`,
    `Margin="36,24,36,0"`) above a scrolling area whose column has the same `MaxWidth`. Both stretch, so each is centred in the same
    width and the title's left edge lines up with the column's. Each page here does the same with `PageTitleTextBlockStyle` and
    `PageColumnStyle` (`App.xaml`), the title outside the `ScrollViewer`, so it stays while the page scrolls. The title is the
    page's Level 1 heading for Narrator; headings inside a page are Level 2 ("No servers yet" on Home, "Sign in to <server>").
  - The column sits in a plain `Border` inside the `ScrollViewer`, as the Gallery's column sits in a `Border` inside its
    `ScrollView`. As the `ScrollViewer`'s direct content it was placed off-centre once it reached 1064 px, by an amount that
    depended on the page, and ran past the window's right edge (owner's run, case 8, 2026-10-24); in the `Border` it is centred
    under the title on every page (maximised: title and column both from x 906, 1064 wide).
  - Maximum width: 1064 px (`PageMaxWidth`, `App.xaml`), the Gallery's value, within the guideline's "constrained max width
    (around 1000–1100 px)". Used on every page; the earlier per-page columns (Home 920) go. The forms keep their own width at the
    left of the column, under the title: Add a server 560 px, Sign in 400 px (a `Grid` whose one `*` column has that `MaxWidth`;
    a `StackPanel` with `HorizontalAlignment="Left"` would take its content's width instead).
  - Margins: 24 px at the left, right and bottom of the column and at both sides of the title. The NavigationView page
    (learn.microsoft.com, ms.date 2026-10-02, read 2026-10-24): "We recommend 12px margins for your content area when
    NavigationView is in `Minimal` mode and 24px margins otherwise"; Minimal is never reached (see "Pane"). The Gallery uses 36;
    the guideline wins. The notices (`MainWindow.xaml`) take the same width and side margins.
  - Vertical: the title 24 px below the top of the content layer, the Gallery's top margin (no guideline gives one), and the
    column 24 px below the title, the distance the owner's run of 2026-10-24 matched against Windows Settings (24 px between the
    boxes). The guidelines say nothing about this gap. Settings' first section header has no top margin of its own.
  - Whether Windows Settings centres its column with the title over it on a very wide window is not verified from a Microsoft
    source; the owner compares. Home's empty and error states stay centred in the page.
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
    Each search (the first and "Search again") passes the up IPv4 interfaces (`LocalInterfaces.Read`, read just before): .NET's
    `NetworkInterface.GetAllNetworkInterfaces`, `OperationalStatus.Up` only, Loopback and Tunnel types skipped, each IPv4 unicast
    address with its `PrefixLength`. The core sends from each of them as well as from an unbound socket (ADR 0016, amended
    2026-10-23 after case 2 of the M1 Windows run). The addresses go into the core only and are never logged.
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
    Connect code 1 2 3 4 5 6"), with "Copy" next to it (named "Copy code" for Narrator: it copies the bare code and confirms in an
    `InfoBar`, "Code copied to the clipboard.", as Copy diagnostics does), "Waiting for approval…" and "Cancel" (Esc too).
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
    Each server is a `SettingsExpander` (2026-10-23): "Sign out" / "Sign in" stay in its header, and expanding it shows a card
    "Remove this server" ("Signs out every user here and forgets the server's addresses on this PC.") with "Remove", named
    "Remove <name>" for Narrator. This follows Windows Settings' own account removal: "Under Other users, select the flyout for
    the account you want to remove. Next to Account and data, select Remove." ("Manage User Accounts in Windows",
    support.microsoft.com, read 2026-10-23). "Remove" asks first, in a `ContentDialog` like the plain-http one: "Remove <name>?",
    "This signs out every user on this PC and forgets the server's addresses. You can add it again later.", primary "Remove",
    close "Cancel" (the default, so Enter and Esc cancel). Whether Windows Settings itself asks before removing an account, and
    with what words, is not verified. Unlike sign-out it asks, because signing in again does not undo it. Nothing is disabled
    while the dialog is open (a page flag keeps a second one from opening): a disabled Remove button lost focus, so Cancel left
    it on "Add another server" (the owner's run, 2026-10-23). After Cancel, Esc or Enter focus goes back to the Remove button,
    set by the page, since whether ContentDialog gives focus back to its opener by itself is not verified (no Microsoft source
    found, 2026-10-23). Only after "Remove" are the row's buttons disabled, until the removal ends. `CredentialsLeft` shows a
    warning: "Removed <name>, but its saved sign-in may still be in Credential Manager. Current no longer uses it. You can
    delete the entries ending in ".Current" under Windows Credentials."
    A settings error shows `ProblemText.Store`. Home and the list update from the saved-server list; rows are removed by key,
    so the other servers' expanders keep their state, and focus moves to "Add another server" (or "Add a server" when none is
    left) once the row is gone. The Remove button carries its row in `Tag` rather than relying on the DataContext the
    expander's items inherit.
  - **Diagnostics:** "Copy diagnostics" now copies `ExportDiagnosticsAsync()` (version, platform, settings state, then the
    redacted request log).
  - **About:** a `SettingsExpander` "Current" with the version, and "© 2026 Skeksik. All rights reserved." inside.
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
  ("six hundred ninety-seven …"), not by its name. The same text is the `AutomationProperties.Name` of a `SpokenText` around the
  code, for when the user reaches it: a named `TextBlock` was read twice in scan mode, by its name and then by its text (owner's
  run, 2026-10-24). TextBlock is sealed ("public sealed class TextBlock", TextBlock page), so `SpokenText` derives from `Grid`
  ("public class Grid : Panel", Grid page) and gives its own peer ("override OnCreateAutomationPeer so it returns a new instance of
  your peer type", "Custom automation peers"): control type Text, the name, and no children (`GetChildrenCore`: "The collection of
  AutomationPeer objects for child elements"); the `TextBlock` inside is also `AccessibilityView="Raw"` ("The primary use case is
  omitting template parts that do not contribute meaningful accessibility semantics", "Custom automation peers"); all on
  learn.microsoft.com, read 2026-10-24. Of the options weighed, a `Run` per digit still exposes the code as one text, and a hidden
  second element is read where nothing is shown. The owner heard the digits read one by one, once, and again for a new code
  (2026-10-24); not verified by ear: whether focus moving to "Cancel" at the same moment cuts the notification short. A finished
  sign-in is announced, "Signed in to <server> as <user>.", from the window's `NavigationView` (`App.Announce`), which outlives the
  sign-in page: Home has no live text, and the owner's run found Narrator reading only the title bar. Not verified by ear: whether
  that is cut short by the title bar or by the "Signed in for this session only." notice; found servers are named by
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
9. the network facts, the device name and the local interfaces can be read (none is printed beyond the connectivity level);
10. last, the main window (XAML, Mica, TitleBar, NavigationView, Home) builds and is shown (its content loads within 10 s); the
    navigation view's content layer (`ContentGrid`) has a visible background on Home and the same one after opening Settings;
    the window follows the core's app theme (`FollowAppTheme`): Dark, then Light, reach `Root.ActualTheme` and the caption buttons'
    glyph colour (white, then black) within 10 s, then it goes back to System; then it closes: closing the only window ends XAML's message loop, so nothing can be awaited after it (CI run 39883272147
    stopped at the next await and exited 0 without a result).
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
3. Home shows the empty state. Settings shows the version, "© 2026 Skeksik. All rights reserved.", "No servers are set up." and "Copy diagnostics"; paste the clipboard
   somewhere: the text is there, also after closing the app.
4. Back and forward: go Home -> Settings, then back with the title bar's back button, Alt+Left, and a mouse Back button; each
   returns to Home, and at Home with an empty back stack the back button is disabled (after Home -> Settings -> Home it stays
   enabled, as in Windows Settings). After going back, Alt+Right and a mouse Forward button return to
   Settings. Narrow the window below 641 px: the pane hides, and "Open Navigation" in the title bar opens it over the page.
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
    ninety-seven"), in scan mode too; Cancel and start it again: the new code is read once more; "Copy code" copies it and Narrator
    reads the confirmation; a finished sign-in is read as "Signed in to <server> as <user>." (see "Quick Connect code and Narrator"
    below).

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

### Re-check of case 2 (fix/discovery-per-interface)
On the same multi-adapter PC, start with no `%LOCALAPPDATA%\Current`:
1. Wired, Wi-Fi off: Add a server lists the server on the first search, within about 2 s. "Search again" three times: listed each
   time.
2. Wi-Fi only: the same.
3. Both on: the server is listed once, not twice.
4. Still no firewall prompt, and no rule for `Current.exe` afterwards.

#### Windows run (2026-10-23)
Heads fix/discovery-per-interface bea8ac9 and feat/remove-server a1467f6, merged in a throwaway local worktree (never pushed) and
built with the four scripts: binding tests 34 passed, smoke run passed. Windows 10.0.26200.9711, x64, the same multi-adapter PC
(wired and Wi-Fi on the same /24, WSL/Hyper-V, two VPN adapters); the cases ran `out/app/Current.exe`. The earlier run's
`%LOCALAPPDATA%\Current` and its credentials were no longer on the PC, so the order changed: discovery first from no data, then
the Quick Connect sign-in, then the address check after re-adding the addresses, then feat/remove-server's removal cases.
1. Wired, Wi-Fi off: **pass.** Listed on the first search within 2 s; "Search again" three times (each showed "Searching…" with
   the list cleared): listed each time, within 2 s.
2. Wi-Fi only (wired adapter disabled): **pass.** The same.
3. Both on: **pass.** Listed once, on the first search and on four "Search again".
4. **Pass.** No firewall prompt at any point; no rule for `Current.exe` (`Get-NetFirewallApplicationFilter`) after the discovery
   cases or at the end.

Quick Connect with Narrator (M1 case 9, with the fix from main): **fail.** Sign-in passes ("Signed in as …"), and Narrator now
speaks the code when it appears, but as a number ("six hundred ninety-seven …"), not digit by digit. `SignInPage.xaml.cs` sets the
digit-by-digit `AutomationProperties.Name` on `CodeText` and `Announce.Shown` raises `LiveRegionChanged`; what Narrator spoke
was the visible text read as a number, so that name is not what the live-region announcement reads here. Suggested: announce the
code with `RaiseNotificationEvent` carrying the digit-by-digit text, and check it by ear.

Address in use (M1 case 8): the server had 1: LAN IP (http, from discovery), 2: DNS name (http), 3: DNS name (https), added in
that order by signing in from typed addresses (no second LAN IP this time). Started on the wired LAN (Wi-Fi on too, same LAN):
Home "Connected, address 1 of 3" at once and still address 1 after 6 minutes, untouched. Copy diagnostics: only the start-up
requests, both to address 1; no address, user name, server id, device id or token in the text. Reading: with the LAN IP first
and nothing cached for this network, the app connects to address 1 and stays there. The earlier "address 2 of 4" did not come
back, but its saved order and cached winner were gone, so moving off a cached worse address after 5 minutes was not exercised.

## Remove a server: what the owner checks
To run with the discovery re-check, on a build from feat/remove-server, with the server signed in (and ideally a second user
signed in to it from this PC too, via "Sign in" after a sign-out):
1. Settings → Accounts: the server's card expands (chevron) and shows "Remove this server" with "Remove". Narrator reads the
   button as "Remove <name>"; Tab reaches it.
2. "Remove": the dialog "Remove <name>?" opens. Esc and Enter both cancel and nothing changes; "Cancel" too. After each,
   focus is back on the Remove button.
3. "Remove" again, then the dialog's "Remove": the card disappears from Settings and from Home at once (Home shows "No servers
   yet" if it was the only one). No warning appears. Focus lands on "Add another server" (or "Add a server"), and with two
   servers the other one's expander stays as it was.
4. Credential Manager (Control Panel → Credential Manager → Windows Credentials): no entry ending in `.Current` is left for that
   server (`jellyfin/<key>/<user id>.Current`).
5. `%LOCALAPPDATA%\Current\settings.json`: no entry for the server in `servers`; `last_server_key` holds its key.
6. The server's dashboard (Devices): this PC's device for each user is gone within a few seconds.
7. Quit and start again: the server does not come back. Add it again: it is added under a new key and signs in normally.
8. Optional: remove a server while it is unreachable (network off): it is removed at once all the same; the dashboard keeps the
   device until the server's own clean-up (nothing could tell it).

### Windows run (2026-10-23)
Heads feat/remove-server a1467f6 and fix/discovery-per-interface bea8ac9, merged in a throwaway local worktree (never pushed) and
built with the four scripts: binding tests 34 passed, smoke run passed. Windows 10.0.26200.9711, x64, the multi-adapter PC of the
M1 run; the cases ran `out/app/Current.exe`. The earlier run's data was no longer on the PC, so the server was added from
discovery first (key 1) and two users were saved: the first signed out, the second signed in from the card (one credential).
1. **Pass.** The card expands and shows "Remove this server" with "Remove"; Tab reaches it and Narrator reads "Remove <name>"
   (its UI Automation name is "Remove " plus the server's name).
2. **Pass.** "Remove <name>?" opens with "Cancel" as the default; Esc, Enter and "Cancel" each close it and nothing changes
   (same `settings.json`, same credentials). Minor: after Esc or Enter the focus ring is on "Add another server", not back on
   the "Remove" button that opened the dialog. Fixed afterwards (f3b4060): the button stays enabled while the
   dialog is open and gets focus back on a cancel.
3. **Pass.** Gone from Settings ("No servers are set up.") and Home ("No servers yet") at once, no warning; focus on "Add a
   server".
4. **Pass.** No entry ending in `.Current` left (`cmdkey /list`, the same Windows Credentials store).
5. **Pass.** `settings.json` has no `servers` (left out when empty); `last_server_key` is 1, the removed server's key.
6. **Pass.** The owner saw this PC's device gone from Devices for both users.
7. **Pass.** Quit and start: "No servers yet". Added again from discovery: key 2 (`last_server_key` 2), signed in normally, one
   credential under `jellyfin/2/`.
8. **Pass.** The owner removed it with wired and Wi-Fi off: gone at once, no warning; `settings.json` and the credential were
   cleared. The device stays in the dashboard's list, as expected.

## Layout fixes: owner's check
Three findings from the owner's use of the app (2026-10-23), fixed on fix/windows-layout-polish. Light and dark theme, a wide
window (wider than 1300 px) and a narrow one:
1. At Home with nothing to go back to, the title bar's back button is dimmed with no filled background, like Windows Settings';
   after Home -> Settings it looks as before (hover and press fills unchanged). With high contrast on, it still shows.
2. Settings: the section headers and cards start under the "Settings" title. Every card has the same width: the column fills
   the window up to 920 px of cards and stays at the left (not centred) on a wide window. Add a server and Sign in: the form
   starts under the page title, and the address, user name and password boxes fill the column (560 and 400 px at most). A notice
   (if one shows) lines up with them.
3. Home with a server added: the card starts under the "Home" title, with the same width as Settings' cards.
4. Narrow window (as narrow as it goes) and with the pane closed from the title bar: on each page the content still starts
   under the page title, and the cards shrink with the window.
5. Home "goes beyond the window border" (the owner's finding): look for a horizontal scroll bar at the bottom of Home, which
   element is cut off at the right edge (the card, its text, the "Sign in" button), and note the window width. The code shows
   no width that should exceed the window, so that part's cause is not found; if it still happens, it is fixed then. The scroll
   viewer should hand the column the viewport's width: its default style sets no horizontal bar visibility and the control
   starts at Disabled (`generic.xaml`, `ScrollViewer_Partial.cpp`, microsoft-ui-xaml main, read 2026-10-23; not verified at run
   time).

### Windows run (2026-10-24)
Heads fix/windows-layout-polish 6f1770b and fix/narrator-quick-connect-code efa74f9, merged in a throwaway local worktree (never
pushed) and built with the four scripts: binding tests 34 passed, smoke run passed. Windows 11 25H2, 10.0.26200.9711, x64, display
scale 150%; the cases ran `out/app/Current.exe`. No server was saved, so the owner added one from discovery and signed in. Window
widths (effective px): 1920 (2880 physical) wide, and 983 (1475 physical) narrower; the narrowest width was not measured.
1. **Pass.** Light and dark, wide and narrow.
2. **Pass.**
3. **Pass.**
4. **Pass.**
5. **Pass.** No horizontal scroll bar at the bottom of Home, nothing cut off at the right edge.

New findings from the owner during the run (not fixed here):
- The gap between the page title and the first section header or card is too small on every page; Windows Settings'
  Accessibility page, compared side by side, has a larger one. The column's top padding is `PageColumnPadding` (56,0,24,24,
  `App.xaml`), used by Home, Settings, Add a server and Sign in.
- Settings' About card: add "All rights reserved." (`Pages/SettingsPage.xaml`).
- The title bar's pane button: its name should be "Open Navigation", and it should show only on a small window, where the pane
  is forced closed; opened there, the pane overlaps the page content instead of pushing it, and the page keeps the same margin
  at both edges of the window. Today it is always shown (`IsPaneToggleButtonVisible="True"` on `AppTitleBar`) and the navigation
  view is fixed at `PaneDisplayMode="Left"` (`MainWindow.xaml`).
- The title bar should be taller (`AppTitleBar`, `MainWindow.xaml`).
- The window needs a sensible minimum width and height; none is set in the shell.

## Window chrome: owner's check
The findings of the Windows run of 2026-10-24 above, fixed on fix/windows-window-chrome: the gap under the page title, the pane, the
background per page, the title bar's height, a minimum window size, and "All rights reserved.". Pane and background: compare side by
side with Task Manager; title gap and title bar: with Windows Settings > Accessibility. Light and dark theme. Widths are effective
pixels (at 150% scale, 1008 px is 1512 physical and 641 is 962).
1. Wide window (1008 px or wider, e.g. maximised):
   - The full pane shows, and the title bar has the back button and the pane button. Narrator reads the pane button as "Close
     Navigation" and its tooltip says the same.
   - Press it: the pane narrows to the icon rail and the page moves left with it (nothing covers the page); the button now reads
     "Open Navigation". Press it again: the full pane returns and the page moves back. Same as Task Manager.
   - Close the pane, narrow the window to medium and widen it again: the pane stays a rail. Restart the app: it opens with the
     full pane (nothing is remembered between runs).
   - The title bar is as tall as Windows Settings', and the caption buttons fill its height; it still drags, double-click
     maximises, and snap layouts show on hovering maximise.
   - On Home (with a server), Settings, Add a server and Sign in, the first card, section header or text sits the same distance
     below the page title on every page, about as far as on Windows Settings' Accessibility page. The title and the content start
     56 px from the pane (or the rail), and a notice, if one shows, lines up with them.
   - Settings > About, expanded: "© 2026 Skeksik. All rights reserved."
2. Medium window (641 to 1007 px):
   - The icon rail shows; no stage with the full pane pushing the page. The pane button opens the full pane over the page (the
     page does not move); Esc, a click on the page, or the button closes it, and choosing Home or Settings in it navigates.
   - The title and content start 56 px from the rail.
3. Small window (below 641 px, down to the minimum):
   - No pane and no rail; the pane button opens the pane over the page.
   - With the pane closed, the page title, the content and any notice start 24 px from the window's left edge, and the content
     keeps 24 px at the right edge.
   - With the pane open (medium or small), Alt+Left and the mouse Back button do not go back (as before); with it closed, they do.
4. Background, at each width: Home, Add a server and Sign in show the content layer: a lighter (or, in dark theme, slightly
   different) surface with a thin border and a rounded top-left corner, starting just below the title bar, as on Task Manager's
   Users page. Settings shows no layer: the same Mica as the pane, with no border. Go Home -> Settings -> Home: the layer goes and
   comes back each time. Switch light/dark on Settings and go Home: the layer has the new theme's colours. At the small width
   the layer has a top border only and no rounded corner (the navigation view's Minimal state).
   Inside the layer, compare with Task Manager's Users page: where the page title sits (here 56 px from the layer's left edge, the
   navigation view's own header position, and 44 px below its top), and the gap between the content and the layer's right and
   bottom edges (here 24 px). If Task Manager's is tighter, note by how much: it is one constant in `PageLayout.cs`.
5. The minimum (drag a corner inwards as far as it goes): the window stops at 500 × 500 px. On each page nothing is cut off at the
   right edge and there is no horizontal scroll bar: the cards and boxes shrink, and the title bar still shows the back and pane
   buttons, "Current" and the caption buttons. Move the window to a display with another scale (or change the scale): the minimum
   stays 500 × 500 effective px. Note how narrow Windows Settings goes, for comparison.
Anything above that cannot be measured is not verified: the owner compares with Task Manager (pane, layer) or Windows Settings
(title gap, title bar height).

### Windows run (2026-10-24)
Heads fix/windows-window-chrome 2be79a6 and fix/narrator-quick-connect-code 9d970f8, merged in a throwaway local worktree (never
pushed) and built with the four scripts: binding tests 34 passed, smoke run passed. Windows 11 26H2, 10.0.26300.9711, x64; main
display at 150%, second display at 100%. The cases ran `out/app/Current.exe` with the saved server, in light and dark theme.
Window widths (the visible frame): 1400, 1011/1010, 900, 644/643, 641, 600 and the minimum. Measured from full-resolution captures
and UI Automation bounds; the owner looked and listened with Narrator.
1. **Pass.**
   - Pane button: Narrator reads "Close Navigation" and the tooltip says the same. Pressed, the pane narrows to the 48 px rail and
     the page moves with it; the button and tooltip read "Open Navigation". Pressed again, the full pane returns. With the pane
     closed, narrowing to 900 and widening to 1400 keeps the rail; after a restart the full pane is back.
   - Title bar 48 px, caption buttons 48 × 48 px, also maximised. Drag, double-click to maximise and back, and snap layouts on
     hovering maximise work. Windows Settings' title bar is also about 48 px, but its caption buttons are 46 × 32 px.
   - Gap under the title: on Home, Settings, Add a server and Sign in the first element's box starts 24 px below the title's box.
     From the title's baseline to the first text: Settings 56 physical px, Add a server 55; Windows Settings' Accessibility, title
     baseline to "Vision", 55. Title and content start 57.3 px from the pane or the rail (56 plus 1.3).
   - About: "© 2026 Skeksik. All rights reserved."
2. **Pass** (900 px): the rail shows; the pane button opens the full pane over the page (the title does not move); Esc, a click on
   the page and the button close it; Home and Settings in it navigate. Title and content 57.3 px from the rail. On the window's
   visible frame the thresholds are about 3 px above the documented ones: the rail from 644 px (navigation view 641.3), the full
   pane from 1011 px (1008), as the navigation view is 4 physical px narrower than the frame.
3. **Pass** (600 px): no pane and no rail; title and content start 24 px from the left edge, content keeps 24 px at the right.
   Pane open: Alt+Left does not go back but closes the pane; the mouse Back button does not go back (over the page it closes the
   pane). Pane closed: both go back, Alt+Right goes forward.
4. **Fail.** No content layer on any page, at any width, in either theme: Home, Add a server and Sign in show the same Mica as the
   pane and Settings (dark #202020, light #F3F3F3; no surface, border or rounded corner; scanned at 1400, 900 and 600 px).
   `ApplyContentLayer` (`MainWindow.xaml.cs`, the content grid of the template part `RootSplitView`) has no visible effect; the
   smoke run only reads `PageLayout.GetHasContentLayer` and never reaches the template. Whether the "RootSplitView not found" trace
   fired was not checked.
   Task Manager's Users page (light, 150%), for the layer's inside: the layer starts about 51 px below the window's top, has a
   1.3 px border at the top and left only, and runs to the window's right and bottom edges with no gap. Its title "Users" is a
   smaller heading (cap height 10 px, here 19.3) whose glyphs start 17.3 px from the layer's inner left edge and 20.7 px below
   its inner top; here 56 and 44 px, and 24 px at the right and bottom.
5. **Pass.** The window stops at 500 × 500 px (outer frame) at 150% and on the 100% display; the visible frame is 488 × 494 px at
   150% and 486 × 493 px at 100% (the rest is the invisible resize border). On Home, Settings and Add a server nothing is cut off
   and there is no horizontal scroll bar; the title bar keeps the back and pane buttons, "Current" and the caption buttons.
   Windows Settings goes down to about 349 × 322 px (visible frame).

New findings from the owner during the run (not fixed here):
- The pane should always have the title bar's colour. Opened over the page (medium and small) it is acrylic (light #F7-#FA) with
  a line (#E4E4E4) below the title bar; Windows Settings' opened pane is #F3F3F3 like its title bar, with no line.
- The caption buttons: Windows Settings' are smaller (46 × 32 px) with space around them; here they are 48 × 48 px and fill the
  title bar.
- With the pane open, the title bar's Back button should be clickable and close the pane; now it is not.
- With the pane open, Alt+Left should go back and close the pane; now it only closes the pane.
- The content's margins from the window's edges should be smaller, as on Windows Settings. Windows Settings at 686 px, pane
  closed: content 25 px from the left and right edges, the title's glyphs 54 px from the window's top (6 px below its 48 px title
  bar). Here at 600 px: 24 px at both sides, but the title's glyphs 105 px from the top (57 px below the title bar). On a wide
  window Windows Settings' content starts about 26 px after its pane (its pane edge estimated, not verified), here 56 px.

## Window chrome: owner's re-check
The fixes for the run above (case 4 and the new findings), on fix/windows-window-chrome. Light and dark theme; compare side by side
with Task Manager's Users page (layer) and Windows Settings (everything else). Widths are the navigation view's; on the window's
visible frame the thresholds sit about 3 px higher (the rail from about 644 px, the full pane from about 1011), as the run found the
navigation view 4 physical px narrower than the frame at 150% (noted, not changed).
1. Content layer (case 4 of the run), at a wide, a medium and a small width:
   - Home (with a server), Add a server and Sign in show the layer: a surface slightly different from the pane, with a thin
     border at its top and left and a rounded top-left corner, starting under the title bar and running to the window's right and
     bottom edges. At the small width it has a top border only and no rounded corner.
   - Settings shows no layer: the same Mica as the pane and the title bar.
   - Home -> Settings -> Home, and back with Alt+Left: the layer goes and comes back each time. Switch light/dark while on Settings,
     then go Home: the layer has the new theme's colours; and the other way round.
   - Inside the layer, against Task Manager's Users page: the title's glyphs about 17 px from the layer's inner left edge and
     21 px below its inner top; the cards and boxes 17 px from its right edge. Note any difference (the title stays the larger
     28 px heading).
2. Margins without the layer (Settings), against Windows Settings at about the same widths: wide, the title and cards about 24 px
   after the pane (Windows Settings about 26); small (around 600 to 690 px), 24 px at both window edges (Windows Settings 25) and
   the title's glyphs about 6 px below the title bar. The distance from the title to the first section header looks as before
   (it passed in the run). Same in every display mode, with the pane open or closed.
3. Pane colour: at a medium and a small width, open the pane over the page: it is the title bar's colour (#F3F3F3 light, #202020
   dark), not acrylic, and there is no line between the title bar and the pane. Note whether Windows Settings shows a line at the
   pane's right edge (here a 1 px line stays there). Wide: the full pane and the rail look as before.
4. Caption buttons: minimise, maximise and close are the standard 46 × 32 px at the top of the 48 px title bar, as in Windows
   Settings; drag, double-click and snap layouts still work.
5. Back with the pane open (medium and small widths):
   - Start the app at a medium width (Home, nothing to go back to): the title bar's Back button is dimmed. Open the pane over the
     page: Back becomes enabled; press it: the pane closes, the page stays Home, and Back is dimmed again.
   - Home -> Settings, open the pane over the page: the title bar's Back button closes the pane and stays on Settings; press it
     again: back to Home. The same with Alt+Left, and with the mouse Back button.
   - Wide window with the full pane: Back follows the history only (the pane is not a back step); at Home after a start it stays
     dimmed.
   - After closing the pane this way on a medium window, widening to wide keeps the rail (the navigation view's forced close).

### Windows run (2026-10-24)
Head fix/windows-window-chrome 05d77bf (Narrator merged in), built with the four scripts: binding tests 34 passed, smoke run passed.
Windows 11 26H2, 10.0.26300.9711, x64; main display at 150%. The cases ran `out/app/Current.exe` with the saved server, in dark
theme and, where noted, light. Window widths (the visible frame): 1400, 900, 686 (Settings only), 600, and maximised; Windows
Settings at 1400, 686 and 600. Measured from full-resolution captures (physical px / 1.5) and UI Automation (Back's IsEnabled,
the page heading); the mouse Back button was sent as XButton1 with SendInput.
1. **Pass.**
   - Home, Add a server and Sign in show the layer at 1400, 900 and 600 px (dark #272727, light #F9F9F9), with a 1.3 px border
     at the top and left (dark #1D1D1D, light #E5E5E5) from 48.7 px below the window's top, and the layer runs to the window's
     right and bottom edges. Wide and medium: rounded top-left corner (about 8 px). Small: top border only, no left border and no
     rounded corner. Settings shows no layer: the same colour as the pane and the title bar (#202020, light #F3F3F3).
   - Home -> Settings -> Home, and Settings -> Home with Alt+Left twice: the layer went and came back each time. Light on
     Settings, then Home: #F9F9F9 with the #E5E5E5 border; back to dark on Settings, then Home: #272727 with #1D1D1D.
   - Against Task Manager's Users page (same scale, both themes): the border is the same (1.3 px, same colours), but Task
     Manager's layer is darker in dark (#191919 against its #202020 title bar; here #272727) and white in light (#FFFFFF; here
     #F9F9F9). Title glyphs: here 19.3 px from the layer's inner left edge and 18.7 px below its inner top; Task Manager 16.7
     and 20.7, so 2.7 px further right and 2 px higher here (the first card's box is 17.3 px from the inner left edge). Right
     gap 16.7 px from the card to the layer's edge at 900 and 600 px (Task Manager's "..." glyph is 18 px from its right edge).
     At 1400 px the Home card stops at its maximum width (920 px), so the right gap is larger there.
2. **Partial: margins pass, the title sits about 3 px higher than in Windows Settings.**
   - Wide (1400): the cards and the title's glyphs start 25.3 and 26 px after the pane; medium (900, 686): 25.3 px after the
     rail. Windows Settings at 1400 starts its cards 4 px further right (349.3 px from the window's left edge, here 345.3); its
     pane edge is not visible, so its gap after the pane is not measured.
   - Small (600): the cards keep 24 px at both window edges (Windows Settings 25.3 at the left, 24.7 at the right), the title's
     glyphs start 24.7 px from the left edge (Windows Settings 26).
   - Title's glyphs below the 48 px title bar: here 2.7 px at every width; Windows Settings 5.3 px at 600 and 686, and 16 px at
     1400. The top of `PageLayout.PlainHeader` (-8) would take about 3 px less for the small width.
3. **Pass.** Opened over the page at 900 and 600 px, the pane is #202020 (light #F3F3F3), the title bar's colour, not acrylic.
   Under the title bar there is only a 1-level difference (#1F1F1F, light #F1F1F1) that does not show. At the pane's right edge
   a 1.3 px line (#2E2E2E, light #DFDFDF) and a 4 px shadow stay; Windows Settings' opened pane (600, 686) has neither a line nor
   a shadow at its right edge, and none under its title bar. The opened pane also has a 1.3 px line at its left edge inside the
   window border (#1D1D1D, light #E7E7E7). Wide: the full pane and the rail look as before.
4. **Pass.** Close's hover box is 46 × 32 px at the top of the 48 px title bar, restored and maximised (69 × 48 physical px);
   minimise and maximise sit beside it in the same row (not measured one by one). Dragging the title bar moves the window,
   double-click maximises and restores, and snap layouts show on hovering maximise.
5. **Pass**, at 900 and 600 px.
   - After a start, at Home: Back is disabled. Pane opened over the page: Back enabled; pressed: the pane closed, Home stayed,
     Back disabled again.
   - Settings with the pane open: the title bar's Back closed the pane and stayed on Settings (Back still enabled); pressed
     again: Home, Back disabled. The same with Alt+Left, and with the mouse Back button with the pointer over the pane and over
     the page.
   - After closing the pane this way at 900, widening to 1400 kept the rail.
   - Wide, after a restart with the full pane: Back disabled at Home; on Settings Back went straight to Home and the full pane
     stayed open.
Not run: Add a server and Sign in in light theme (only Home and Settings were checked in light); the second display.


## Quick Connect code and Narrator: owner's check
For fix/narrator-quick-connect-code (the code announced by a notification, see "Keyboard and Narrator" under Screens), on a build
of that branch, signed out of a server that offers Quick Connect:
1. Narrator on (Win+Ctrl+Enter); open the sign-in page and press "Use Quick Connect". The code is read digit by digit, once, and
   not again as a number. Note whether "Cancel, button" is also read and whether it cuts the code short.
2. Press Cancel, then "Use Quick Connect" again: the new code is read digit by digit, once.
3. In scan mode (Caps Lock+Space), arrow to the code: Narrator reads "Quick Connect code" and the digits, once, and not the code
   again as a number. With Narrator off, the code can still be selected with the mouse and copied with Ctrl+C.
4. Tab to "Copy" next to the code: Narrator reads "Copy code, button". Press it (Enter or Space): "Code copied to the clipboard."
   is shown, and Narrator reads it. Paste into Notepad (not saved): the bare code, without spaces.
5. Approve the code from another Jellyfin client: Home opens and Narrator reads "Signed in to <server> as <user>.", once, whole.
   Note whether the title bar or the element focused on Home is read before or after it, or cuts it short.
6. Sign out, then sign in with the password: the same announcement.

### Windows run (2026-10-24)
Heads fix/narrator-quick-connect-code efa74f9 and fix/windows-layout-polish 6f1770b, merged in a throwaway local worktree (never
pushed) and built with the four scripts: binding tests 34 passed, smoke run passed. Windows 11 25H2, 10.0.26200.9711, x64, display
scale 150%; the cases ran `out/app/Current.exe`. No server was saved, so the owner added one from discovery. Cases numbered as in
the run: 6-8 are 1-3 above, 9 is approving the code from another Jellyfin client, 10 is the Remove dialog's focus (main).
6. **Pass.** "Use Quick Connect": Narrator read "Quick Connect code" and the digits one by one, once, not again as a number.
   Whether "Cancel, button" was also read was not noted.
7. **Pass.** Cancel, then "Use Quick Connect" again: the new code was read digit by digit, once.
8. **Fail.** In scan mode, arrowing to the code, Narrator read "Quick Connect code" and the digits one by one, then the code
   again as one number. The code's `TextBlock` (`CodeText`, `Pages/SignInPage.xaml`) has the spoken digits as its UI Automation
   name (`Pages/SignInPage.xaml.cs`, `AutomationProperties.SetName`) and the bare code as its text; scan mode reads both.
9. **Fail.** After the code was approved from another client, Narrator read the title bar, not "Signed in as …". Nothing
   announces the sign-in: `Finish` (`Pages/SignInPage.xaml.cs`) navigates to Home and clears the history, and "Signed in as …"
   is only the server card's text (`ViewModels/ServersViewModel.cs`).
10. **Pass.** Settings -> Accounts -> the server -> "Remove": after Esc, Enter and "Cancel", focus is back on "Remove" and
    Narrator says "Remove <name>, button" each time.

New finding from the owner during the run (not fixed here): add a copy button for the Quick Connect code, next to `CodeText`. Cases 8 and 9 and the copy button were fixed afterwards in the same branch (owner, 2026-10-24); cases 3-6 above check them.

### Windows run (2026-10-24, cases 1-6)
Heads fix/narrator-quick-connect-code 9d970f8 and fix/windows-window-chrome 2be79a6, merged in a throwaway local worktree (never
pushed) and built with the four scripts: binding tests 34 passed, smoke run passed. Windows 11 26H2, 10.0.26300.9711, x64, display
scale 150%; the cases ran `out/app/Current.exe` with the saved server, signed out first from Settings. Cases 1-6 above (6-11 in
the run); the owner ran them with Narrator and reported that everything worked as described.
1. **Pass.** "Use Quick Connect": the code was read digit by digit, once, and not again as a number. Whether "Cancel, button" was
   also read was not noted.
2. **Pass.** Cancel, then "Use Quick Connect" again: the new code was read digit by digit, once.
3. **Pass.** In scan mode, arrowing to the code: "Quick Connect code" and the digits, once, not the number again. With Narrator off,
   the code could be selected with the mouse and copied with Ctrl+C.
4. **Pass.** Tab to "Copy": "Copy code, button". Pressed: "Code copied to the clipboard." was shown and read. Pasted into Notepad
   (not saved, then deleted): the bare code (6 digits).
5. **Pass.** Approved from another Jellyfin client: Home opened and Narrator read "Signed in to <server> as <user>.", once, whole.
   What else was read, and in which order, was not noted.
6. **Pass.** Signed out, then signed in with the password: the same announcement.
No new finding about Narrator; the window chrome findings of this run are under "Window chrome: owner's check" on
fix/windows-window-chrome.

## Windows shell defaults: owner's check
The owner's 8 findings of 2026-10-24 (on main cfa83b9), fixed on fix/windows-shell-defaults. Run each case at three widths: wide
(1008 px or more, e.g. 1400 or maximised), medium (e.g. 800) and the minimum (500), in light and in dark theme. Compare with Photos
(back button, overlay pane, content background) and Windows Settings (pane width, centred column). Widths are effective pixels (at
150%, 1008 px is 1512 physical).
1. Back button inset: the title bar's back button no longer touches the window's left edge. Hover it: its backplate starts a few
   px (4 effective) inside the border, and its arrow is about 16 px from the border. Compare with Windows Settings' back button
   (Photos has none in its title bar). The pane button sits right of it, as before. Both are 40 × 36, 6 px from the top (case 9).
2. Pane never hidden:
   - Wide: the full pane shows; the pane button switches between the full pane and the icon rail, and the page moves with it
     (nothing covers the page).
   - Medium and minimum: the icon rail always shows; the pane button opens the full pane over the page (the page does not move);
     Esc, a click on the page, or the button closes it. At 500 px the rail and the pane button are still there.
   - With the pane open over the page, the title bar's Back, Alt+Left and the mouse Back button only close it; with it closed,
     they go back. "Open Navigation" and "Close Navigation" as before.
3. Pane width: the full pane (wide, and opened over the page) is 300 px, as Windows Settings' (measured by the owner), against 320
   before.
4. Content background: Home, Add a server, Sign in and Settings all show the same content background (a surface with a thin border
   at its top and left and a rounded top-left corner, under the title bar): #F9F9F9 light, #272727 dark, with a #E5E5E5 / #1D1D1D
   stroke, as in Photos. Home -> Settings -> Home: it does not change. Switch light/dark on Settings: it follows.
5. Overrides removed: nothing else in the window looks different from the control's own look except what cases 1-4, 7 and 8
   and 9-11 describe; the 48 px title bar, the 500 × 500 minimum, the dimmed transparent Back button at Home and "All rights
   reserved." in Settings > About are unchanged.
6. Sizes: the page title sits 24 px below the content's top, the column 24 px from the left, right and bottom (at the minimum
   width too), the first item about as far below the title as before.
7. Pane opened over the page (medium and minimum): translucent acrylic (the wallpaper and the page show through, softly), as in
   Photos, not the title bar's solid colour. It reaches the window's top behind the title bar (case 10). Note the 1 px line at its
   edges, up its right edge to the window's top: it is the control's own border and stays unless the owner decides otherwise (docs
   above, "Pane opened over the page").
8. Centred content: on a wide window (maximised on a wide display) every page's column stops at 1064 px and is centred in the space
   right of the pane, with the page title over its left edge; the forms (Add a server, Sign in) keep their width at the left of
   that column. At medium and minimum widths the column fills the width less 24 px on each side. A notice, if one shows, lines up
   with the column. Narrator: each page's title is read as heading level 1 ("Home", "Settings", "Add a server", "Sign in").
The owner's new findings of the first run (below), fixed on the same branch:
9. Title bar buttons: the caption buttons are 48 × 48, as tall as the title bar, as Photos'; the back and pane buttons are 40 × 36
   with 6 px above and below, as Windows Settings' back button.
10. Pane open over the page (medium and minimum): the title bar's Back and pane buttons show their hover and pressed backplates and
    work as with the pane closed; a click on them does not only light-dismiss the pane. Back is enabled as soon as the pane starts
    to open. Resizing across 1008 px: Back does not flash enabled.
11. Caption button hover: #2D2D2D dark, #EAEAEA light, as Photos'. Switch Windows' mode with the app running: the hover follows.
Anything above that cannot be measured is not verified.

### Windows run (2026-10-24)
Head fix/windows-shell-defaults 97d8aff in a throwaway local worktree, built with the four scripts: binding tests 34 passed, smoke
run passed. Windows 11 26H2, 10.0.26300.9711, x64, display scale 150% (3840 × 2160); the cases ran `out/app/Current.exe` with the
saved server; for Sign in the owner signed out and back in. Widths (visible frame, effective px): 1400, 1420, 1500 and 1600,
maximised (2560 × 1392), 800 and 500 (the minimum: 488 × 494 visible); dark, then light (the owner switched Windows' mode with the
app running). Measured from full-resolution captures and UI Automation bounds, side by side with Photos and Windows Settings at
the same widths. Px are effective; a 1 px stroke is 2 physical px (1.3 effective) at 150%.
1. **Pass.** Hovered, the back button's backplate (#2D2D2D dark) starts 4 px inside the window's edge, 40 × 44 at 2 px from the
   top; the arrow's 16 px glyph box starts 16 px in (its ink 18 px). Windows Settings' back button: the same 4 px inset, 40 px
   wide. Photos showed no back button in its title bar (Gallery, a folder, its Settings, the viewer); the owner accepted the
   Windows Settings comparison. See new finding 1.
2. **Pass**, with new finding 2. At 1400 the pane button switches between the full pane (300) and the rail (48) inline, and the
   page moves with it. At 800 and 500 the rail always shows, and the pane button opens the full pane over the page, which does
   not move. Esc, a click on the page, the title bar's Back, Alt+Left and the mouse Back button each close it and the page
   stays. With the pane closed, they go back. "Open Navigation" / "Close Navigation" as before.
3. **Pass.** The full pane is 300 px, inline and over the page; Windows Settings' pane is 300 px (1400 wide). Photos' is 320.
4. **Pass.** Home, Add a server, Sign in and Settings show the same layer: dark #272727 with a #1D1D1D stroke at its top and left
   under a #202020 title bar and pane; light #F9F9F9, #E5E5E5 stroke, #F3F3F3. The rounded top-left corner has a radius of about
   8 px. Photos: the same three colours in both themes, its stroke at the same place. Home -> Settings -> Home: unchanged. It
   followed Windows' switch from dark to light without a restart.
5. **Pass.** 48 px title bar, 46 × 32 caption buttons; the minimum 500 × 500 (outer frame; 488 × 494 visible, as in earlier runs);
   after a start, Back at Home is disabled and dimmed (arrow #535353 dark, #C7C7C7 light) with no backplate; "All rights
   reserved." in Settings > About. New findings 1 and 3 are about the caption buttons.
6. **Pass.** The page title starts 24 px inside the layer's stroke, at its top and left (25.3 from the layer's outer edge). At 500,
   Settings scrolled to its end: 24 px left, 24 right, 24.7 below the last card. The first item sits 24 px below the title's box.
   It was not compared with main ("as before" is not verified).
7. **Pass.** Acrylic over the page: dark #2D2D2D-#303030 grain, light #F8F8F8-#FAFAFA. Photos' pane over its gallery takes on
   the photos' colours, the same material. The 1 px line: dark #353535 on the right edge and #2F2F2F under the title bar; light
   #E1E1E1 with a soft shadow, and #D5D5D5 under the title bar. Photos' pane over the page has the same line (dark #3E3E3E).
   **The owner keeps the line.**
8. **Fail** at wide widths. At 800 and 500 the column fills the width less 24 px on each side, and the forms keep their width
   (560 Add a server, 400 Sign in) at its left. From the moment the column reaches 1064 px (window wider than about 1413 with the
   full pane, or 1400 with the rail), the title is centred correctly but the column is pushed right, by an amount that depends
   on the page. Maximised, the title starts at x 906: Settings' column starts at 1122 (+216), Add a server's form at 1203
   (+297), Sign in's at 1254 (+348) and Home's card at about 1361 (+455, 1050 px free on its left, 143 on its right). At 1420 to
   1600, Home's card runs past the window's right edge and is cut off; at 1400 with the rail, Settings' column also runs to the
   edge. The same in light. Narrator: each page title has UI Automation heading level 1 (Home, Settings, Add a server, Sign in),
   which Narrator reads; it was not listened to.
   Seen: the column is off-centre and clipped at wide widths. Expected: centred under the title, its left edge at the title's.
   Likely element: the `PageColumnStyle` grid (App.xaml, MaxWidth 1064, Margin 24) inside each page's `ScrollViewer`
   (Pages/*.xaml). The offset changes with the page's content, as if the ScrollViewer arranged its content wider than its
   viewport.

New findings from the owner during the run (not fixed here):
1. Seen: the caption buttons are 46 × 32 at the top of the 48 px title bar. The back and pane buttons look as tall as the title
   bar and touch the window's border (measured 40 × 44, 2 px from the top, 4 px from the left). Expected: caption buttons as tall
   as the title bar (Photos' are 48 × 48), and a small gap between the back and pane buttons and the window's border. Likely
   element: `AppTitleBar` (MainWindow.xaml) and `PreferredHeightOption` kept at Standard (MainWindow.xaml.cs); the TitleBar's
   back and pane buttons.
2. Seen: with the pane open over the page (800, 500), the title bar's Back and pane buttons show no hover or pressed state (the
   title bar stays #202020 under the pointer; with the pane closed the backplate is #2D2D2D). A click only light-dismisses the
   pane. Expected: both stay interactive, as with the pane closed. Likely element: the NavigationView's light-dismiss layer for
   the open pane covering the TitleBar's `PART_BackButton` and `PART_PaneToggleButton` (MainWindow.xaml).
3. Seen: after Windows switched from dark to light with the app running, the caption buttons hover #191919 on the #F3F3F3 title
   bar; after a restart in light they hover #E5E5E5 (Photos: #EAEAEA). Expected: the caption buttons follow a theme change while
   the app runs. Likely element: the caption button colours of the window's title bar, which are not refreshed on a theme change
   (MainWindow.xaml.cs handles no theme change).

### Windows run (2026-10-24, final)
Code at fix/windows-shell-defaults fe74762 in a local worktree, built with the four scripts: binding tests 34 passed, smoke run
passed (again before each commit). Windows 11 26H2, 10.0.26300.9711, x64, display scale 150% (3840 × 2160); `out/app/Current.exe`
with the saved server. Widths (effective px of the window frame): 1400, 800 and 500 in light and dark, maximised (2560) in dark,
and steps across 1008 (900 to 1100). Measured from UI Automation bounds and screen pixels of Current's window, side by side with
Photos and Windows Settings; the owner looked at every change and sent two screen recordings of Windows Settings and of Current.
1. **Pass.** Back's backplate 4 px inside the window's edge, as Windows Settings'.
2. **Pass.** As in the first run; the owner checked resizing and the open pane on the full-window layout.
3. **Pass.** Full pane 300 px, rail 48.
4. **Pass.** Light #F9F9F9 layer, #E5E5E5 stroke at y 48, #F3F3F3 title bar and pane; dark #272727, #1D1D1D, #202020, at 1400,
   800 and 500, with the navigation view under the title bar.
5. **Pass.** 48 px title bar, 500 × 500 minimum, dimmed Back at Home with no backplate, "All rights reserved." unchanged.
6. **Pass.** Title 24 px inside the layer's stroke at its top and left (x 325.3 at 1400 with the full pane, 73.3 with the rail;
   y 73.3), as in the first run.
7. **Pass.** Acrylic over the page, dark #2E2E2E; it now reaches the window's top behind the title bar's left 300 px. Its 1 px
   line runs up the pane's right edge (#303030-#353535 dark), its top row barely visible (#323232). **The owner keeps it as it
   is.**
8. **Pass** after the fix (the column in a `Border`, see "Page layout"). Maximised, Home's and Settings' columns start at x 906
   with the title and are 1064 wide; the owner checked every page, Add a server and Sign in included.
9. **Pass.** Caption buttons 48 × 48 (Photos 48 × 48); back and pane buttons 40 × 36 at 4, 6 (Windows Settings' back button
   40 × 36 at 4, 6).
10. **Pass.** With the navigation view under the title bar, the title bar's buttons get hover and clicks with the pane open
    (owner); Back enabled as the pane starts to open, as in the owner's recording of Windows Settings. Resizing across 1008:
    a traced build of the earlier code logged Back enabled for up to 16 ms on every Expanded to Compact crossing (3 of 3), the
    final code on none; the owner saw no flash.
11. **Pass.** Minimize hover #2D2D2D dark (Photos and Windows Settings #2D2D2D), #EAEAEA light (Photos #EAEAEA, Windows Settings
    #E9E9E9), after a live switch to light and after a restart in light; back to dark live: #2D2D2D. Pressed not measured.

Not verified: the pressed caption colour; a switch to high contrast while the app runs; Narrator read aloud (the titles have UI
Automation heading level 1). The owner considers the M1 window and page layout done (2026-10-24).

## App theme (M1)
The owner asked (2026-10-24) for a theme choice in Settings like the one in Windows' own Photos app: Settings → "Personalization" →
an "App theme" card, "Select which app theme to display", with a combo box: Light, Dark, Windows default.

- **The setting is the core's.** `AppTheme` (`System`, `Light`, `Dark`) is a field of the settings file (ADR 0013, `app_theme`,
  2026-10-24), saved at once by `CoreHost.SetAppThemeAsync` and observed with `CoreHost.ObserveAppTheme`. A value from a newer
  version reads as `System` and stays in the file until the theme is changed here. If it cannot be saved (a newer version's file, a
  failed write) the combo returns to the theme the core still has and a notice under the card says why.
- **The card.** "Guidelines for app settings" (learn.microsoft.com, ms.date 2026-04-08, read 2026-10-24): "If your app allows users
  to choose the app's color mode, present these options using a combo box inside a SettingsCard. The options should read: Light,
  Dark, Use system setting", and "When a user changes a setting, the app should immediately reflect the change". The header and
  description are the WinUI Gallery's ("App theme", "Select which app theme to display"; `WinUIGallery/Pages/SettingsPage.xaml`,
  github.com/microsoft/WinUI-Gallery, main, read 2026-10-24), and the section comes first, as the Gallery's "Appearance & behavior".
  **Labels:** Photos' "Windows default" for the third option, as the owner asked, not the guideline's "Use system setting" (which the
  Gallery uses; Task Manager too, per the manager, not verified). The page does not link to "Windows color settings"
  (`ms-settings:colors`), which the guideline says "you may also want to add"; the owner's example has none (not verified).
  Narrator: the card and the combo are both named "App theme" (`AutomationProperties.Name`).
  **Icon:** Photos' card has a palette icon, and the owner wanted ours to have one (2026-10-24): Segoe Fluent Icons E790 "Color"
  ("Segoe Fluent Icons font", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-24) as the card's `HeaderIcon`, a `FontIcon`
  with no font or size set, so `SettingsCard` gives it its own. It is the only card with an icon so far.
- **The whole window.** `MainWindow.FollowAppTheme` sets `Root.RequestedTheme` (Light, Dark, or Default for Windows default) and
  follows every change. "The RequestedTheme value you set on a FrameworkElement will inherit to any elements that are nested within
  the element", and it can change "at run-time", unlike `Application.RequestedTheme` (FrameworkElement.RequestedTheme,
  learn.microsoft.com, updated 2026-09-30, read 2026-10-24). The title bar, the pane and the pages are all inside `Root`. Windows
  default is `ElementTheme.Default`, as the WinUI Gallery maps "Use system setting" (`SettingsPage.xaml.cs`, main, read 2026-10-24).
  The same API page notes "On Windows, setting RequestedTheme to ElementTheme.Default will always result in "Dark" being the
  theme". That did not happen on the owner's PC: under Windows default the app followed Windows into light and back to dark
  while it ran (owner's run, 2026-10-24, below). High contrast ignores `RequestedTheme` (same page).
- **Caption buttons.** The system draws their glyphs for the system's theme, so in the other app theme they would be white on light
  or black on dark. `ApplyCaptionButtonColors`, which already runs on load and on every
  `ActualThemeChanged`, now also sets `ButtonForegroundColor`, `ButtonHoverForegroundColor` and `ButtonPressedForegroundColor`
  (white in dark, black in light, as the WinUI Gallery's `TitleBarHelper.ApplySystemThemeToCaptionButtons`, main, read
  2026-10-24) and `ButtonInactiveForegroundColor` (WinUI's `TextFillColorDisabled`, #5DFFFFFF dark, #5C000000 light; not
  measured against Windows' own apps). "If you set any title bar color, we recommend that you explicitly set all the colors"
  ("Title bar customization", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-24). A change of `Root.RequestedTheme` changes
  `ActualTheme` and so raises `ActualThemeChanged`; a live system change under Windows default did so in the owner's run of PR 38
  (item 11 above), which is how the hover colours followed.
- **At start.** The window is activated once the core is open (one small file) or after 300 ms; the theme is set on `Root` before
  `Activate`, so the first frame is in the saved theme. A slower open shows the system's theme first and switches when it is open.

### App theme: what the owner checks
1. Settings opens with "Personalization" first: the "App theme" card, "Select which app theme to display", the combo showing
   "Windows default" on a first run.
2. Light, then Dark, then Windows default: the whole window (title bar, pane, page, Mica) changes at once, without a restart, with
   Windows itself in light and in dark.
3. The caption buttons in each case: glyphs readable (black in Light, white in Dark), hover #EAEAEA light and #2D2D2D dark as in
   PR 38's item 11; also with the window inactive (dimmed glyphs).
4. Windows default: switch Windows between light and dark while the app runs; the window and the caption buttons follow. Confirm
   that `ElementTheme.Default` follows Windows in light mode (the API page's note says it would always be Dark).
5. Restart with Light or Dark chosen while Windows uses the other: the window opens in the chosen theme without a flash of the other.
6. Narrator on the combo: "App theme, combo box, Light" (or the current value) and the options read as their labels.
7. Not on the owner's run unless chosen: high contrast with a theme chosen (should look as without the setting).

### Windows run (2026-10-24, owner's PC, Windows 11 Pro 10.0.26300, Windows in dark mode at the start)
PR 39 at f1d6315 (04d79e8 plus the card's icon), built with the four scripts (C# tests 36 passed; the smoke run passed, with Dark
and Light reaching the window and the caption-button glyphs). The owner compared it with Photos' own App theme setting.
1. Personalization section and App theme card (placement, wording, icon; Narrator): **pass** after one change. Photos' card has a
   palette icon and ours had none; the owner chose to add it (Segoe Fluent Icons E790, above).
2. Light: the whole window light at once, without a restart (title bar, caption buttons with hover and pressed, pane, overlay
   pane, content layer, dialogs including Remove, InfoBars, the combo's drop-down): **pass**.
3. Dark: the same: **pass**.
4. Windows default, then Windows switched to light and back to dark while the app ran: the app followed live, so
   `ElementTheme.Default` did not mean "always Dark" here: **pass**.
5. Restart with a theme chosen: the chosen theme from the first frame, with no flash of the other: **pass**.
6. settings.json: Light writes `"app_theme": "light"`; Windows default removes the key (read from the file; only that key
   printed): **pass**.
7. High contrast and Narrator: the owner tried them: **pass**.

The owner accepted the app theme and considers M1 done (2026-10-24).
