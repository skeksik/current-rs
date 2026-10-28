using System.Diagnostics;
using System.Runtime.InteropServices;
using Current.Bindings;
using Current.Shell.Browsing;
using Current.Shell.Pages;
using Microsoft.UI.Input;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Navigation;
using Windows.System;
using Windows.UI;
using Windows.UI.ViewManagement;

namespace Current.Shell;

/// <summary>
/// The main window: Mica, the content extended into the title bar with the system caption buttons, and a NavigationView over one
/// Frame. Navigation follows the NavigationView page's example (learn.microsoft.com, ms.date 2026-10-02, read 2026-10-22): navigate
/// on ItemInvoked without duplicate back stack entries, go back through the Frame, and keep the selection in step on Navigated.
/// </summary>
public sealed partial class MainWindow : Window, Player.IPlayerHost
{
    private static readonly Dictionary<string, Type> Pages = new() { ["home"] = typeof(HomePage) };

    /// <summary>Pages reached from Home (adding a server, signing in): Home stays selected, and the page's title names the step.</summary>
    private static readonly HashSet<Type> HomeSteps = [typeof(AddServerPage), typeof(SignInPage)];

    /// <summary>
    /// The smallest window, in effective pixels: the narrowest page column (Sign in, 400 plus 24 on each side) and the title bar with
    /// its back and pane buttons, title and caption buttons fit (manager, 2026-10-24; the WinUI Gallery uses
    /// 640 by 500, github.com/microsoft/WinUI-Gallery MainWindow.xaml.cs, main at 14061ee, read 2026-10-24).
    /// </summary>
    private const double MinWindowWidth = 500;

    private const double MinWindowHeight = 500;

    private bool _wasDeactivated;

    /// <summary>Whether the core was last told the window is minimized (<see cref="ReportMinimized"/>).</summary>
    private bool _reportedMinimized;

    private FrameworkElement? _paneToggleButton;

    /// <summary>The navigation view's content layer, read by the smoke run: ContentGrid, the content of the part RootSplitView.</summary>
    private Grid? _contentLayer;

    private readonly TaskCompletionSource _shown = new(TaskCreationOptions.RunContinuationsAsynchronously);

    private readonly AccessibilitySettings _accessibility = new();

    /// <summary>The core's app theme, followed from <see cref="FollowAppTheme"/> until the window closes.</summary>
    private ValueSubscription<AppTheme>? _appTheme;

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        // Ours again after the title bar sets its own click-through areas, which it does as it resizes or a button changes
        // (QueueTitleBarRegions); not on every layout pass in the window, which a scrolling grid makes many of (review, 2026-10-27).
        AppTitleBar.SizeChanged += (_, _) => QueueTitleBarRegions();
        AppTitleBar.Loaded += (_, _) =>
        {
            if (AppTitleBar.XamlRoot is { } root)
            {
                root.Changed += (_, _) => QueueTitleBarRegions();
            }
            QueueTitleBarRegions();
        };
        // Caption buttons as tall as the 48 px title bar (owner's finding, 2026-10-24; Photos' are 48 × 48, measured on the owner's PC,
        // 2026-10-24): "Set PreferredHeightOption to Tall so the caption buttons are the same height as the title bar" ("Title bar
        // customization", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-24), as the WinUI Gallery does after
        // ExtendsContentIntoTitleBar (MainWindow.xaml.cs, github.com/microsoft/WinUI-Gallery, main at 14061ee, read 2026-10-24).
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;

        // XAML cannot parse "GoBack" as a VirtualKey (XamlParseException in CI), so this one is added here.
        var goBack = new KeyboardAccelerator { Key = VirtualKey.GoBack };
        goBack.Invoked += OnBackAccelerator;
        Root.KeyboardAccelerators.Add(goBack);

        // handledEventsToo: a control under the pointer may already have handled the press.
        Root.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler(OnPointerPressed), true);

        Activated += OnActivated;
        Closed += (_, _) =>
        {
            _closed = true;
            _appTheme?.Dispose();
            _appTheme = null;
        };
        Player.Attach(this, this);
        // handledEventsToo: the player still shows its controls on a key a control took; its shortcuts skip handled keys.
        Root.AddHandler(UIElement.KeyDownEvent, new KeyEventHandler((_, e) =>
        {
            if (Player.IsOpen)
            {
                Player.HandleKey(e);
            }
        }), true);
        AppWindow.Changed += OnAppWindowChanged;
        AppWindow.Closing += OnClosing;
        Root.Loaded += OnRootLoaded;

        NavView.SelectedItem = HomeItem;
        Navigate(typeof(HomePage), new EntranceNavigationTransitionInfo());
    }

    /// <summary>Adds a notice above every page; the user closes it. Its text is read out when it opens (InfoBar page,
    /// learn.microsoft.com, read 2026-10-23).</summary>
    internal void ShowNotice(InfoBarSeverity severity, string title, string message)
    {
        var notice = new InfoBar { Severity = severity, Title = title, Message = message, IsOpen = true, Margin = new Thickness(0, 8, 0, 0) };
        notice.Closed += (_, _) => Notices.Children.Remove(notice);
        Notices.Children.Add(notice);
    }

    /// <summary>
    /// Shows the core's app theme now and follows every change (Settings, Personalization). Called once the core is open, before the
    /// window is first shown when the core opens in time (App.OnLaunched).
    /// </summary>
    internal void FollowAppTheme(CoreHost core)
    {
        _appTheme?.Dispose();
        var theme = core.ObserveAppTheme(new DispatcherQueueUi(DispatcherQueue));
        theme.PropertyChanged += (_, _) => ApplyAppTheme(theme.Value);
        _appTheme = theme;
        ApplyAppTheme(theme.Value);
    }

    /// <summary>
    /// The theme on the window's root element, which the title bar, the pane and the pages inherit: "The RequestedTheme value you set on
    /// a FrameworkElement will inherit to any elements that are nested within the element", and unlike Application.RequestedTheme it
    /// can change "at run-time" (FrameworkElement.RequestedTheme, learn.microsoft.com, updated 2026-09-30, read 2026-10-24). "Windows
    /// default" is ElementTheme.Default, as the WinUI Gallery's "Use system setting" is (SettingsPage.xaml.cs, main, read 2026-10-24),
    /// so the window takes the app's theme, which follows the system while App.xaml sets no RequestedTheme. The same page notes "On
    /// Windows, setting RequestedTheme to ElementTheme.Default will always result in "Dark" being the theme"; that and a live system
    /// change are on the owner's check (docs/notes/windows-shell.md). The change raises ActualThemeChanged, which recolours the caption
    /// buttons (ApplyCaptionButtonColors). High contrast ignores RequestedTheme (same page).
    /// </summary>
    private void ApplyAppTheme(AppTheme theme) => Root.RequestedTheme = theme switch
    {
        AppTheme.Light => ElementTheme.Light,
        AppTheme.Dark => ElementTheme.Dark,
        _ => ElementTheme.Default,
    };

    /// <summary>The theme the window shows now (the smoke run).</summary>
    internal ElementTheme ActualTheme => Root.ActualTheme;

    /// <summary>The caption buttons' glyph colour now (the smoke run).</summary>
    internal Color? CaptionButtonForeground => AppWindow.TitleBar.ButtonForegroundColor;

    /// <summary>Completes when the window's content has loaded and the navigation view's template is applied (the smoke run).</summary>
    internal Task Shown => _shown.Task;

    /// <summary>The content layer's background now, or null if its template part was not found (the smoke run).</summary>
    internal Brush? ContentLayerBackground => _contentLayer?.Background;

    /// <summary>Opens a library's grid, as its item in the pane does (the smoke run).</summary>
    internal void ShowLibrary(Library library) =>
        Navigate(typeof(LibraryPage), new SuppressNavigationTransitionInfo(), new LibraryArgs(library));

    /// <summary>Opens a view tab, as its item in the pane does (the smoke run).</summary>
    internal void ShowView(ViewArgs view) => Navigate(view.Page, new SuppressNavigationTransitionInfo(), view);

    /// <summary>Searches every signed-in server, as the title bar's box does (the smoke run).</summary>
    internal void ShowSearch(string term) => Navigate(typeof(SearchPage), new SuppressNavigationTransitionInfo(), new SearchArgs(term));

    /// <summary>The page shown now (the smoke run).</summary>
    internal Page? ShownPage => ContentFrame.Content as Page;

    /// <summary>The parameter of the page shown now (the smoke run).</summary>
    internal object? ShownParameter => _shownParameter;

    /// <summary>Goes back, as the title bar's Back button does (the smoke run).</summary>
    internal bool GoBack() => TryGoBack();

    /// <summary>The titles of the notices above the pages now (the smoke run).</summary>
    internal IReadOnlyList<string> NoticeTitles => [.. Notices.Children.OfType<InfoBar>().Select(notice => notice.Title)];

    /// <summary>The navigation's library items now, servers' items left out (the smoke run).</summary>
    internal int LibraryItemCount => _libraryItems.OfType<NavigationViewItem>().Count(item => item.Tag is LibraryArgs);

    /// <summary>Opens Settings, as its item in the pane does (the smoke run).</summary>
    internal void ShowSettings() => Navigate(typeof(SettingsPage), new SuppressNavigationTransitionInfo());

    /// <summary>Opens Settings > Supported formats, as its card in Settings does (the smoke run).</summary>
    internal void ShowSupportedFormats() => Navigate(typeof(SupportedFormatsPage), new SuppressNavigationTransitionInfo());

    private void OnRootLoaded(object sender, RoutedEventArgs e)
    {
        Root.Loaded -= OnRootLoaded;
        ApplyCaptionButtonColors();
        Root.ActualThemeChanged += (_, _) => ApplyCaptionButtonColors();
        // The template is applied by now. The template's items name parts "ContentGrid" too, so the layer is taken from the SplitView
        // it is the content of.
        _contentLayer = (FindDescendant(NavView, "RootSplitView") as SplitView)?.Content as Grid;
        if (_contentLayer is null)
        {
            Trace.TraceWarning("Current: NavigationView template part RootSplitView not found; the smoke run cannot read its layer");
        }
        DispatcherQueue.TryEnqueue(() =>
        {
            NamePaneToggleButton();
            if (_paneToggleButton is null)
            {
                Trace.TraceWarning("Current: TitleBar template part PART_PaneToggleButton not found; the pane button keeps its name");
            }
        });
        ApplyMinimumSize();
        // The scale changes when the window moves to a display with another scale or the user changes it.
        Root.XamlRoot.Changed += (_, _) => ApplyMinimumSize();
        _shown.TrySetResult();
    }

    /// <summary>
    /// Sets the window's minimum size through its presenter (OverlappedPresenter.PreferredMinimumWidth and PreferredMinimumHeight,
    /// Windows App SDK 1.7 and later, learn.microsoft.com, read 2026-10-24). The API page gives no unit; the WinUI Gallery's
    /// WindowHelper.SetWindowMinSize multiplies by XamlRoot.RasterizationScale, so these are physical pixels (not verified otherwise).
    /// </summary>
    private void ApplyMinimumSize()
    {
        if (AppWindow.Presenter is not OverlappedPresenter presenter || Root.XamlRoot is null)
        {
            return;
        }
        var scale = Root.XamlRoot.RasterizationScale;
        presenter.PreferredMinimumWidth = (int)Math.Ceiling(MinWindowWidth * scale);
        presenter.PreferredMinimumHeight = (int)Math.Ceiling(MinWindowHeight * scale);
    }

    /// <summary>
    /// The caption buttons' glyphs, and their hover and pressed backgrounds, in the app's theme, set again when it changes.
    /// Glyphs: the system draws them for its own theme, so with the app in the other theme (Settings, Personalization) they would be
    /// white on light or black on dark. They follow the window's theme, white in dark and black in light, as the WinUI Gallery sets
    /// ButtonForegroundColor and ButtonHoverForegroundColor in its app-theme setting (TitleBarHelper.ApplySystemThemeToCaptionButtons,
    /// github.com/microsoft/WinUI-Gallery, main, read 2026-10-24). Pressed takes the same colour; an inactive window's glyphs take
    /// WinUI's TextFillColorDisabled (#5DFFFFFF dark, #5C000000 light, Common_themeresources_any.xaml, same tag as below; not measured
    /// against Windows' own apps). "If you set any title bar color, we recommend that you explicitly set all the colors" ("Title bar
    /// customization", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-24).
    /// Backgrounds: WinUI sets only their
    /// background and inactive background (transparent) and leaves these unset, and the system then drew a light theme's hover in
    /// dark mode, #191919 on the #202020 title bar, and kept dark colours after a switch to light while the app ran (owner's new
    /// finding 3; measured on the owner's PC, 2026-10-24). Photos and Windows Settings hover at #2D2D2D dark and #EAEAEA light,
    /// which is WinUI's SubtleFillColorSecondary over the title bar; pressed takes SubtleFillColorTertiary (not measured). Values
    /// from Common_themeresources_any.xaml (microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24). The WinUI Gallery
    /// re-applies its caption colours on ActualThemeChanged in the same way, "as AppWindow TitleBar doesn't update caption button
    /// colors correctly when changed while app is running" (TitleBarHelper.cs, main at 14061ee, read 2026-10-24). High contrast
    /// keeps the system's own colours; AccessibilitySettings.HighContrastChanged cannot be subscribed to in a desktop window
    /// (0x80070490 in the smoke run), so a switch to high contrast while the app runs is caught only if it changes ActualTheme (not
    /// verified). Setting PreferredTheme (UseDefaultAppMode or Dark) did not change the hover (owner's PC, 2026-10-24).
    /// </summary>
    private void ApplyCaptionButtonColors()
    {
        var titleBar = AppWindow.TitleBar;
        if (_accessibility.HighContrast)
        {
            titleBar.ButtonForegroundColor = null;
            titleBar.ButtonHoverForegroundColor = null;
            titleBar.ButtonPressedForegroundColor = null;
            titleBar.ButtonInactiveForegroundColor = null;
            titleBar.ButtonHoverBackgroundColor = null;
            titleBar.ButtonPressedBackgroundColor = null;
            return;
        }
        var dark = Root.ActualTheme == ElementTheme.Dark;
        var glyph = dark ? Microsoft.UI.Colors.White : Microsoft.UI.Colors.Black;
        titleBar.ButtonForegroundColor = glyph;
        titleBar.ButtonHoverForegroundColor = glyph;
        titleBar.ButtonPressedForegroundColor = glyph;
        titleBar.ButtonInactiveForegroundColor = dark ? Color.FromArgb(0x5D, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x5C, 0x00, 0x00, 0x00);
        titleBar.ButtonHoverBackgroundColor = dark ? Color.FromArgb(0x0F, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x09, 0x00, 0x00, 0x00);
        titleBar.ButtonPressedBackgroundColor = dark ? Color.FromArgb(0x0A, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x06, 0x00, 0x00, 0x00);
    }

    /// <summary>Also on PaneOpening: Back is enabled as the pane starts to open over the page, as Windows Settings' back button is
    /// (owner's recording, 2026-10-24), not once its animation has ended.</summary>
    private void OnPaneOpenedOrClosed(NavigationView sender, object args) => QueuePaneStateUpdate();

    private void OnDisplayModeChanged(NavigationView sender, NavigationViewDisplayModeChangedEventArgs args)
    {
        QueuePaneStateUpdate();
        // The title bar's box gives way to its Search button when the pane does to the icon rail (owner, 2026-10-26).
        var narrow = args.DisplayMode != NavigationViewDisplayMode.Expanded;
        if (narrow != _narrow)
        {
            _narrow = narrow;
            PlaceSearch();
        }
    }

    /// <summary>
    /// Updates the pane button's name and Back once the navigation view has finished the change. Crossing 1008 px, it raises
    /// DisplayModeChanged while the full pane is still open and closes or reopens the pane after; read at once, the pane counted as
    /// over the page for that moment and Back flashed enabled (owner's recording, 2026-10-24).
    /// </summary>
    private void QueuePaneStateUpdate() => DispatcherQueue.TryEnqueue(() =>
    {
        NamePaneToggleButton();
        UpdateBackButton();
        // The pencil shows only while the pane is open, and closing the pane leaves edit mode.
        ShowEditNavigation();
    });

    /// <summary>
    /// The pane open over the page (Compact; Minimal is never reached, CompactModeThresholdWidth is 0): it counts as a back step,
    /// which back closes. The full pane inline on a wide window does not (owner, 2026-10-24).
    /// </summary>
    private bool PaneOverlaysPage => NavView.IsPaneOpen && NavView.DisplayMode == NavigationViewDisplayMode.Compact;

    /// <summary>The title bar's Back button is enabled while there is a page to go back to, the pane overlays the page, or the player is
    /// open (Back closes it).</summary>
    private void UpdateBackButton()
    {
        AppTitleBar.IsBackButtonEnabled = ContentFrame.CanGoBack || PaneOverlaysPage || Player.IsOpen;
        QueueTitleBarRegions();
    }

    // The player ----------------------------------------------------------------------------------------------------------------------

    /// <summary>
    /// Plays <paramref name="target"/>'s item over the pages, from its resume point or, with <paramref name="fromStart"/>, from its
    /// beginning (ADR 0019: the details screen owns that choice). The pages and the pane leave the tab order and Narrator's view while it
    /// plays: hidden, not removed, so the page comes back as it was.
    /// </summary>
    internal void Play(PlayTarget target, bool fromStart)
    {
        if (App.Core is not { } core)
        {
            return;
        }
        NavView.Visibility = Visibility.Collapsed;
        AppTitleBar.IsPaneToggleButtonVisible = false;
        ShowPageCommands(false);
        _ = Player.PlayAsync(core, target.Item.Item, fromStart ? TimeSpan.Zero : target.Start);
        UpdateBackButton();
    }

    /// <summary>
    /// The title bar's search and Refresh belong to the pages, so they go while the player is open and come back with the page
    /// (owner, 2026-10-28; ADR 0022 items 3 and 4). Hidden, not removed: the search keeps its text and Refresh its state.
    /// </summary>
    private void ShowPageCommands(bool show)
    {
        var visibility = show ? Visibility.Visible : Visibility.Collapsed;
        RefreshButton.Visibility = visibility;
        SearchHost.Visibility = visibility;
        if (!show)
        {
            CloseNarrowSearch();
        }
        QueueTitleBarRegions();
    }

    public bool IsFullScreen => AppWindow.Presenter.Kind == AppWindowPresenterKind.FullScreen;

    /// <summary>Full screen through the AppWindow's full-screen presenter, on whichever monitor the window is (ADR 0007 item 3).</summary>
    public void SetFullScreen(bool on)
    {
        if (on == IsFullScreen)
        {
            return;
        }
        if (on)
        {
            // The one-row gap (ADR 0007, "Limits"): with the content extended into the title bar, WinUI places the content's window
            // (Microsoft.UI.Content.DesktopChildSiteBridge) one row below the top of the client area, so it stays a row short of the
            // full-screen presenter's (GetWindowRect of the child windows, and Playback info: content 3840×2159 of a 3840×2160 client,
            // owner's PC, 2026-10-25). Not extended, before the presenter changes, the content fills it.
            ExtendsContentIntoTitleBar = false;
            AppWindow.SetPresenter(AppWindowPresenterKind.FullScreen);
        }
        else
        {
            AppWindow.SetPresenter(AppWindowPresenterKind.Overlapped);
            ExtendsContentIntoTitleBar = true;
            SetTitleBar(AppTitleBar);
            AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
            ApplyCaptionButtonColors();
        }
    }

    /// <summary>The title bar's subtitle names what plays, as Media Player shows the title of what it plays.</summary>
    public void ShowPlaying(string? title) => AppTitleBar.Subtitle = title ?? "";

    public void PlayerClosed()
    {
        NavView.Visibility = Visibility.Visible;
        AppTitleBar.IsPaneToggleButtonVisible = true;
        ShowPageCommands(true);
        UpdateBackButton();
        DispatcherQueue.TryEnqueue(() => ContentFrame.Focus(FocusState.Programmatic));
    }

    /// <summary>
    /// The presenter changed (the player's button or keys, or Windows): in full screen the title bar goes and the player takes the whole
    /// window, its row included.
    /// </summary>
    private void OnAppWindowChanged(AppWindow sender, AppWindowChangedEventArgs args)
    {
        ReportMinimized();
        if (!args.DidPresenterChange)
        {
            return;
        }
        var full = IsFullScreen;
        AppTitleBar.Visibility = full ? Visibility.Collapsed : Visibility.Visible;
        Grid.SetRow(Player, full ? 0 : 1);
        Grid.SetRowSpan(Player, full ? 2 : 1);
        Player.FullScreenChanged(full);
    }

    /// <summary>
    /// Tells the core when the window is minimized or restored: the live updates' socket closes after a minute minimized and opens
    /// again on the restore (ADR 0023, decision 1); playback goes on. A window behind others or not focused is not minimized. The
    /// state is read on every <c>AppWindow.Changed</c> and every activation or deactivation, and reported only when it differs: the docs
    /// say <c>DidPresenterChange</c> is set "When a new presenter is applied (the AppWindow.Presenter property changes)" ("Manage app
    /// windows", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-28), and name no flag for a minimize or a restore, so which
    /// event comes first is not verified. <c>OverlappedPresenter.State</c> is "the state of the presenter" (Minimized, Maximized or
    /// Restored; learn.microsoft.com, updated 2026-07-28, read 2026-10-28). The player's full screen uses the full-screen presenter,
    /// which has no state: there <c>IsIconic</c> answers, which "Determines whether the specified window is minimized (iconic)"
    /// (learn.microsoft.com, updated 2025-07-01, read 2026-10-28). A restore after a minimize also activates the window, so
    /// <see cref="OnActivated"/> reports the return to the foreground as well: the socket reopens from this report, the lists
    /// refresh from that one.
    /// </summary>
    private void ReportMinimized()
    {
        var minimized = AppWindow.Presenter is OverlappedPresenter overlapped
            ? overlapped.State == OverlappedPresenterState.Minimized
            : IsIconic(Microsoft.UI.Win32Interop.GetWindowFromWindowId(AppWindow.Id));
        if (minimized == _reportedMinimized)
        {
            return;
        }
        _reportedMinimized = minimized;
        App.WindowMinimized(minimized);
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsIconic(IntPtr window);

    /// <summary>Whether the window waits for the player before it closes, and whether that wait is over.</summary>
    private bool _closingPlayerFirst, _playerClosedForWindow;

    /// <summary>Closing the window while the player is open, or its engine still opening or closing (after Back), closes the player
    /// first and waits until no engine is left (its stop is reported, the engine shut down), then the window, so the core is not
    /// disposed under it. A second close while it waits is held back. async void, as event handlers are: it catches everything
    /// itself.</summary>
    private async void OnClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (_playerClosedForWindow)
        {
            return;
        }
        if (_closingPlayerFirst)
        {
            args.Cancel = true;
            return;
        }
        if (!Player.IsBusy)
        {
            return;
        }
        args.Cancel = true;
        _closingPlayerFirst = true;
        try
        {
            await Player.CloseAndWaitAsync();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: closing the player with the window failed: {error.GetType().Name}");
        }
        _playerClosedForWindow = true;
        Close();
    }

    /// <summary>
    /// Names the title bar's pane button as the navigation view names its own: "Open Navigation" while the pane is closed, "Close
    /// Navigation" while it is open (NavigationButtonClosedName and NavigationButtonOpenName, NavigationView's Resources.resw), with
    /// the same tooltip. The TitleBar names it "Toggle Navigation" whatever the state and has no property for it, so its template part
    /// PART_PaneToggleButton is looked up (TitleBar.h and TitleBar.cpp, LoadPaneToggleButton; microsoft-ui-xaml, tag
    /// winui3/release/2.5.1, read 2026-10-24).
    /// </summary>
    private void NamePaneToggleButton()
    {
        _paneToggleButton ??= FindDescendant(AppTitleBar, "PART_PaneToggleButton");
        if (_paneToggleButton is null)
        {
            return;
        }
        var name = NavView.IsPaneOpen ? "Close Navigation" : "Open Navigation";
        AutomationProperties.SetName(_paneToggleButton, name);
        ToolTipService.SetToolTip(_paneToggleButton, name);
    }

    private static FrameworkElement? FindDescendant(DependencyObject parent, string name)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is FrameworkElement element && element.Name == name)
            {
                return element;
            }
            if (FindDescendant(child, name) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>
    /// After the window was in the background, activating it again tells the core, so unreachable servers try again at once (ADR 0014).
    /// Restoring from minimised and a redirected second launch both activate the window.
    /// </summary>
    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
        ReportMinimized();
        if (args.WindowActivationState == WindowActivationState.Deactivated)
        {
            _wasDeactivated = true;
            return;
        }
        if (_wasDeactivated)
        {
            _wasDeactivated = false;
            App.ReturnedToForeground();
        }
    }

    private void OnItemInvoked(NavigationView sender, NavigationViewItemInvokedEventArgs args)
    {
        if (ReferenceEquals(args.InvokedItemContainer, EditNavItem))
        {
            EnterEditMode();
            return;
        }
        // Choosing a page leaves edit mode (MainWindow.Arrange.cs).
        LeaveEditMode();
        if (args.IsSettingsInvoked)
        {
            Navigate(typeof(SettingsPage), args.RecommendedNavigationTransitionInfo);
        }
        else if (args.InvokedItemContainer?.Tag is ViewArgs view)
        {
            Navigate(view.Page, args.RecommendedNavigationTransitionInfo, view);
        }
        else if (args.InvokedItemContainer?.Tag is LibraryArgs library)
        {
            Navigate(typeof(LibraryPage), args.RecommendedNavigationTransitionInfo, library);
        }
        else if (args.InvokedItemContainer?.Tag is string tag && Pages.TryGetValue(tag, out var page))
        {
            Navigate(page, args.RecommendedNavigationTransitionInfo);
        }
    }

    /// <summary>The parameter of the page shown now (a library's), so invoking its item again adds no history.</summary>
    private object? _shownParameter;

    private void Navigate(Type page, NavigationTransitionInfo transition, object? parameter = null)
    {
        // Only navigate if the page is not already shown, so the back stack gets no duplicates.
        var same = parameter is LibraryArgs library ? library.Shows(_shownParameter) : Equals(_shownParameter, parameter);
        if (ContentFrame.CurrentSourcePageType != page || !same)
        {
            ContentFrame.Navigate(page, parameter, transition);
        }
    }

    /// <summary>
    /// Follows the window's browse sessions (App.Browsing): the search box and one navigation item per library show while a Jellyfin
    /// server is signed in, and pages of a server or a library that is gone leave the history (a page shown now goes back to Home).
    /// Called once the core is open.
    /// </summary>
    internal void FollowBrowsing(BrowseHub hub)
    {
        hub.Changed += (_, _) => ShowBrowsing(hub);
        ShowBrowsing(hub);
    }

    private readonly List<object> _libraryItems = [];

    private void ShowBrowsing(BrowseHub hub)
    {
        _searchAvailable = hub.IsAvailable;
        PlaceSearch();
        ShowLibraries(hub);
        DropGoneHistory(hub);
        SelectShownPage();
    }

    /// <summary>
    /// The pane's items follow the core's arrangement (ADR 0024) in place: an item still shown is kept (and renamed), a new one is
    /// inserted where the user's order puts it, one gone or hidden is removed, and the rest stay as they are, so the selection and
    /// keyboard focus stay on what they were on. The libraries sit inside the Libraries item, inside an item per server when more
    /// than one is signed in; with one server its Movies and Series sit under Home and its views above Libraries, each in the user's
    /// order.
    /// </summary>
    private void ShowLibraries(BrowseHub hub)
    {
        var servers = hub.Servers;
        var grouped = servers.Count > 1;
        // Each container's items as they should be, the Libraries item's first; an item that moves (one server more or less) is taken
        // out of every container before any is filled, as an item can have one parent only.
        var wanted = new List<(IList<object> Items, List<object> Want)>();
        var top = new List<object>();
        wanted.Add((LibrariesItem.MenuItems, top));
        var all = new List<object>();
        // The views across every library: above Libraries with one server, first inside each server's item with more.
        var single = servers.Count == 1 ? servers[0].Server : null;
        var singlePane = servers.Count == 1 ? servers[0].Pane : null;
        // Movies and Series under Home with one server, each while the core lists it (a library of its type is listed) and the user
        // has not hidden it, in the user's order.
        var tabOrder = (singlePane?.Tabs ?? []).Select(tab => tab.Key).ToList();
        foreach (var (item, kind) in KindTabItems)
        {
            var shown = singlePane?.Tabs.Any(tab => tab.Key == kind && !tab.Hidden) ?? false;
            item.Tag = shown ? new ViewArgs(single!, BrowseView.Everything, kind) : null;
            item.Content = Views.Title(BrowseView.Everything, kind);
            item.Visibility = shown ? Visibility.Visible : Visibility.Collapsed;
        }
        var viewOrder = (singlePane?.Views ?? []).Select(view => view.Key).ToList();
        foreach (var (item, key) in ViewItems)
        {
            var shown = singlePane?.Views.Any(view => view.Key == key && !view.Hidden) ?? false;
            item.Tag = shown ? new ViewArgs(single!, PaneRows.ViewOf(key)) : null;
            item.Visibility = shown ? Visibility.Visible : Visibility.Collapsed;
        }
        // The top level in the user's order: Home, the tabs, the separator, the views, Libraries. Collapsed items keep a place.
        static int Rank(List<string> order, string key)
        {
            var at = order.IndexOf(key);
            return at >= 0 ? at : int.MaxValue;
        }
        var topLevel = new List<object> { HomeItem };
        topLevel.AddRange(KindTabItems.OrderBy(tab => Rank(tabOrder, tab.Kind)).Select(tab => (object)tab.Item));
        topLevel.Add(LibrariesSeparator);
        topLevel.AddRange(ViewItems.OrderBy(view => Rank(viewOrder, view.Key)).Select(view => (object)view.Item));
        topLevel.Add(LibrariesItem);
        foreach (var server in servers)
        {
            var into = top;
            if (grouped)
            {
                var group = _libraryItems.OfType<NavigationViewItem>().FirstOrDefault(known => Equals(known.Tag, server.Server))
                    ?? new NavigationViewItem { Tag = server.Server, SelectsOnInvoked = false, IsExpanded = true };
                var name = hub.ServerName(server.Server);
                if (!Equals(group.Content, name))
                {
                    group.Content = name;
                }
                top.Add(group);
                all.Add(group);
                into = [];
                wanted.Add((group.MenuItems, into));
                // The server's Movies and Series first, as under Home with one server, then its Favourites and Recently added, each in
                // the user's order and without those the user hid.
                var pane = server.Pane;
                var views = (pane?.Tabs ?? []).Where(tab => !tab.Hidden)
                    .Select(tab => (Tag: new ViewArgs(server.Server, BrowseView.Everything, tab.Key), Glyph: KindTabGlyph(tab.Key)))
                    .Concat((pane?.Views ?? []).Where(view => !view.Hidden)
                        .Select(view => (Tag: new ViewArgs(server.Server, PaneRows.ViewOf(view.Key)), Glyph: PaneRows.ViewGlyph(view.Key))));
                foreach (var (tag, glyph) in views)
                {
                    var item = _libraryItems.OfType<NavigationViewItem>().FirstOrDefault(known => Equals(known.Tag, tag))
                        ?? new NavigationViewItem { Tag = tag, Content = Views.Title(tag.View, tag.Kind), Icon = new FontIcon { Glyph = glyph } };
                    into.Add(item);
                    all.Add(item);
                }
            }
            // The libraries the user has not hidden, in the user's order.
            foreach (var library in (server.Pane?.Libraries ?? []).Where(entry => !entry.Hidden).Select(entry => entry.Library))
            {
                var item = _libraryItems.OfType<NavigationViewItem>()
                    .FirstOrDefault(known => known.Tag is LibraryArgs args && args.Library.Item == library.Item);
                if (item is null)
                {
                    item = new NavigationViewItem { Content = library.Name, Icon = new FontIcon { Glyph = LibraryGlyph(library.Collection) } };
                }
                else
                {
                    var shown = ((LibraryArgs)item.Tag).Library;
                    if (shown.Name != library.Name)
                    {
                        item.Content = library.Name;
                    }
                    if (shown.Collection != library.Collection)
                    {
                        item.Icon = new FontIcon { Glyph = LibraryGlyph(library.Collection) };
                    }
                }
                item.Tag = new LibraryArgs(library);
                into.Add(item);
                all.Add(item);
            }
        }
        // A server's item no longer shown lets go of its libraries too.
        foreach (var gone in _libraryItems.OfType<NavigationViewItem>()
            .Where(item => item.Tag is not (LibraryArgs or ViewArgs) && !all.Contains(item)))
        {
            gone.MenuItems.Clear();
        }
        // The top level last: its items never change parent.
        wanted.Add((NavView.MenuItems, topLevel));
        foreach (var (items, want) in wanted)
        {
            for (var i = items.Count - 1; i >= 0; i--)
            {
                if (!want.Contains(items[i]))
                {
                    items.RemoveAt(i);
                }
            }
        }
        foreach (var (items, want) in wanted)
        {
            for (var i = 0; i < want.Count; i++)
            {
                if (i < items.Count && ReferenceEquals(items[i], want[i]))
                {
                    continue;
                }
                // Moved (the user's order changed) or new: every item before it is already in place.
                items.Remove(want[i]);
                items.Insert(i, want[i]);
            }
        }
        _libraryItems.Clear();
        _libraryItems.AddRange(all);
        LibrariesItem.Visibility = top.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        // Between Home's items and the rest, while anything shows below it.
        LibrariesSeparator.Visibility = top.Count > 0 || ViewItems.Any(view => view.Item.Visibility == Visibility.Visible)
            ? Visibility.Visible
            : Visibility.Collapsed;
        ShowEditNavigation();
    }

    /// <summary>The Favourites and Recently added items above Libraries, by the key the core's Views group gives each.</summary>
    private (NavigationViewItem Item, string Key)[] ViewItems => [(FavouritesItem, PaneEntry.Favourites), (RecentItem, PaneEntry.RecentlyAdded)];

    /// <summary>The Movies and Series items under Home, by the kind each shows (the pane layout's tab keys).</summary>
    private (NavigationViewItem Item, string Kind)[] KindTabItems => [(MoviesItem, "movie"), (SeriesItem, "series")];

    /// <summary>A kind tab's icon (PaneRows.KindTabGlyph).</summary>
    private static string KindTabGlyph(string kind) => PaneRows.KindTabGlyph(kind);

    /// <summary>A library's icon by what it holds (PaneRows.LibraryGlyph).</summary>
    private static string LibraryGlyph(string? collection) => PaneRows.LibraryGlyph(collection);

    private static ServerRef? ServerOf(object? parameter) => parameter switch
    {
        LibraryArgs library => library.Library.Item.Server,
        ViewArgs view => view.Server,
        DetailsArgs details => details.Item.Server,
        PersonArgs person => person.Person.Server,
        _ => null,
    };

    /// <summary>Detail views kept for history entries that are no longer in the history (a new page dropped the forward stack).</summary>
    private void PruneKeptDetails()
    {
        if (App.Browsing is not { } hub)
        {
            return;
        }
        var live = new HashSet<object>(ReferenceEqualityComparer.Instance);
        foreach (var entry in ContentFrame.BackStack.Concat(ContentFrame.ForwardStack))
        {
            if (entry.Parameter is { } parameter)
            {
                live.Add(parameter);
            }
        }
        if (_shownParameter is { } shown)
        {
            live.Add(shown);
        }
        foreach (var server in hub.Servers)
        {
            server.PruneDetails(live);
        }
        hub.PruneSearches(live);
    }

    /// <summary>
    /// Whether a history entry shows what is gone: a page of a server whose session ended, search results while no server is signed in,
    /// or a library that left its server's list (the core no longer lists it: removed on the server, or hidden from the user).
    /// </summary>
    private static bool IsGone(BrowseHub hub, Type? page, object? parameter) =>
        (ServerOf(parameter) is { } server && hub.Find(server) is null)
        || (page == typeof(SearchPage) && !hub.IsAvailable)
        || (parameter is LibraryArgs args && hub.IsGone(args.Library.Item));

    /// <summary>Drops the back and forward entries that show what is gone (<see cref="IsGone"/>).</summary>
    private void PruneGoneHistory(BrowseHub hub)
    {
        foreach (var stack in new[] { ContentFrame.BackStack, ContentFrame.ForwardStack })
        {
            for (var i = stack.Count - 1; i >= 0; i--)
            {
                if (IsGone(hub, stack[i].SourcePageType, stack[i].Parameter))
                {
                    stack.RemoveAt(i);
                }
            }
        }
    }

    /// <summary>
    /// Pages of what is gone leave the back and forward stacks, and the page shown now, if it is one, goes back to Home: for a gone
    /// library with a short notice, which is also how opening a stale entry ends once the core has found the library gone. Navigating
    /// to Home pushes the page it leaves onto the back stack ("Whenever a page is loaded into the frame, that page is added as a
    /// PageStackEntry to the BackStack or ForwardStack", "Navigate between two pages", learn.microsoft.com, read 2026-10-25), so the
    /// stacks are pruned again after it, and on every navigation (OnNavigated): Back from Home never reopens it (review, 2026-10-25).
    /// </summary>
    private void DropGoneHistory(BrowseHub hub)
    {
        PruneGoneHistory(hub);
        if (IsGone(hub, ContentFrame.SourcePageType, _shownParameter))
        {
            var library = _shownParameter is LibraryArgs args && hub.IsGone(args.Library.Item);
            ContentFrame.Navigate(typeof(HomePage), null, new SuppressNavigationTransitionInfo());
            PruneGoneHistory(hub);
            if (library)
            {
                ShowLibraryGoneNotice();
            }
        }
        UpdateBackButton();
    }

    /// <summary>One "no longer available" notice at a time: a newer one replaces one still open, however many libraries went.</summary>
    private void ShowLibraryGoneNotice()
    {
        foreach (var open in Notices.Children.OfType<InfoBar>().Where(notice => notice.Title == BrowseLabels.LibraryGoneTitle).ToList())
        {
            Notices.Children.Remove(open);
        }
        ShowNotice(InfoBarSeverity.Informational, BrowseLabels.LibraryGoneTitle, BrowseLabels.LibraryGoneMessage);
    }

    private void OnSearchSubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        var term = args.QueryText.Trim();
        if (term.Length > 0)
        {
            Navigate(typeof(SearchPage), new SuppressNavigationTransitionInfo(), new SearchArgs(term));
        }
    }

    private void OnSearchAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        // No search over the player: its box is hidden while it plays (ShowPageCommands).
        if (Player.IsOpen)
        {
            return;
        }
        if (SearchBox.Visibility == Visibility.Visible)
        {
            args.Handled = SearchBox.Focus(FocusState.Keyboard);
        }
        else if (SearchButton.Visibility == Visibility.Visible || NarrowSearchPlate.Visibility == Visibility.Visible)
        {
            OpenNarrowSearch(FocusState.Keyboard);
            args.Handled = true;
        }
    }

    /// <summary>Whether a Jellyfin server is signed in, so search has something to search.</summary>
    private bool _searchAvailable;

    /// <summary>Whether the pane is not expanded, so the title bar's box gives way to the Search button (OnDisplayModeChanged).</summary>
    private bool _narrow;

    // Windows Settings' title bar search, measured on the owner's PC at 150 % (2026-10-26): 500 wide at most, centred on the window,
    // narrowing with the window while keeping the room on its left mirrored on its right.
    private const double SearchMaxWidth = 500;

    private void OnSearchHostSizeChanged(object sender, SizeChangedEventArgs e) => LayoutSearch();

    private bool _titleBarRegionsQueued;

    /// <summary>
    /// Sets the title bar's click-through areas (the non-client "passthrough" regions) to its buttons and search as they show,
    /// once the title bar has laid out and set its own. Everything else there is caption: a double-click on it maximizes the window.
    /// The title bar's own list (UpdateInteractableElementsList and GetBounds in TitleBar.cpp, microsoft-ui-xaml, tag
    /// winui3/release/2.5.1, read 2026-10-26) leaves out a disabled Back button and takes the search box where layout put it, not
    /// where its render transform moves it (LayoutSearch); a double-click on either maximized the window (owner's run, 2026-10-26).
    /// The title bar only sets its regions again when its own list's rects change, so these stay until then, and are set again after
    /// its layout passes.
    /// </summary>
    private void QueueTitleBarRegions()
    {
        if (_titleBarRegionsQueued || _closed)
        {
            return;
        }
        _titleBarRegionsQueued = true;
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
        {
            _titleBarRegionsQueued = false;
            if (_closed)
            {
                return;
            }
            try
            {
                SetTitleBarRegions();
            }
            catch (Exception error)
            {
                // A layout pass as the window closes can queue this after its AppWindow is gone: an exception here, unhandled on the
                // UI thread, ended the process with 0xC000027B after a passing smoke run (CI's install test, 2026-10-26).
                Trace.TraceWarning($"Current: the title bar's click-through areas were not set: {error.GetType().Name}");
            }
        });
    }

    /// <summary>The window has closed: nothing touches its AppWindow any more.</summary>
    private bool _closed;

    private void SetTitleBarRegions()
    {
        if (AppTitleBar.XamlRoot is not { } root)
        {
            return;
        }
        var scale = root.RasterizationScale;
        var rects = new List<Windows.Graphics.RectInt32>();
        void Add(FrameworkElement? element)
        {
            if (element is null || element.Visibility != Visibility.Visible || element.ActualWidth <= 0 || !IsShown(element))
            {
                return;
            }
            var bounds = element.TransformToVisual(null).TransformBounds(
                new Windows.Foundation.Rect(0, 0, element.ActualWidth, element.ActualHeight));
            rects.Add(new Windows.Graphics.RectInt32(
                (int)Math.Round(bounds.X * scale), (int)Math.Round(bounds.Y * scale),
                (int)Math.Round(bounds.Width * scale), (int)Math.Round(bounds.Height * scale)));
        }
        _backPart ??= FindNamed(AppTitleBar, "PART_BackButton");
        _panePart ??= FindNamed(AppTitleBar, "PART_PaneToggleButton");
        Add(_backPart);
        Add(_panePart);
        Add(RefreshButton);
        Add(SearchBox);
        Add(SearchButton);
        // The small-window search box's top reaches 4 into the title bar: a click there is the box's, not a drag (review, 2026-10-27).
        Add(NarrowSearchPlate);
        // Only when they differ from what is set: setting them again on every layout pass (pressing the search box lays it out again)
        // turned the text cursor over it into the arrow while the button was held (owner's run, 2026-10-26). Compared with what the
        // window has, not with our last value: the title bar sets its own when Back is enabled, and ours must replace them then
        // (review, 2026-10-27).
        var source = Microsoft.UI.Input.InputNonClientPointerSource.GetForWindowId(AppWindow.Id);
        if (source.GetRegionRects(Microsoft.UI.Input.NonClientRegionKind.Passthrough).SequenceEqual(rects))
        {
            return;
        }
        source.SetRegionRects(Microsoft.UI.Input.NonClientRegionKind.Passthrough, [.. rects]);
    }

    /// <summary>The title bar's own Back and pane buttons, parts of its template, found once.</summary>
    private FrameworkElement? _backPart;

    private FrameworkElement? _panePart;

    /// <summary>Whether every element from <paramref name="element"/> up is visible: a collapsed parent hides it too.</summary>
    private static bool IsShown(FrameworkElement element)
    {
        for (DependencyObject? at = element; at is not null; at = VisualTreeHelper.GetParent(at))
        {
            if (at is UIElement { Visibility: Visibility.Collapsed })
            {
                return false;
            }
        }
        return true;
    }

    /// <summary>A named part of a control's template, found by walking its visual tree.</summary>
    private static FrameworkElement? FindNamed(DependencyObject parent, string name)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is FrameworkElement element && element.Name == name)
            {
                return element;
            }
            if (FindNamed(child, name) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>
    /// The title bar's box, as Settings sizes and places it: at most <see cref="SearchMaxWidth"/> wide and centred on the window, with
    /// the room the back, pane and Refresh buttons and the title take on its left (where the title bar's content column, SearchHost,
    /// starts) kept on its right too. It also stays inside that column, which ends well before the caption buttons: the title bar
    /// keeps a 48 wide drag region (TitleBarMinDragRegionWidth) and pads for the caption buttons (RightPaddingColumn); at 150 % that
    /// padding measured 216, the caption buttons' inset in physical pixels, where the buttons take 144 (owner's PC, 2026-10-26). So a
    /// box that would cross into it moves left, or narrows.
    /// </summary>
    private void LayoutSearch()
    {
        var window = Root.ActualWidth;
        var host = SearchHost.ActualWidth;
        if (host <= 0 || window <= 0)
        {
            return;
        }
        var left = SearchHost.TransformToVisual(Root).TransformPoint(new Windows.Foundation.Point(0, 0)).X;
        var width = Math.Max(0, Math.Min(Math.Min(SearchMaxWidth, window - 2 * left), host));
        // Moved by a render transform, not a margin: a margin adds to what the content asks of the title bar, which then hid its title
        // to make room (owner's run, 2026-10-26).
        SearchBox.Width = width;
        SearchBox.RenderTransform = new TranslateTransform { X = Math.Min((window - width) / 2 - left, host - width) };
        QueueTitleBarRegions();
    }

    /// <summary>The title bar's box in a wider window, its Search button in a small one, nothing without a server.</summary>
    private void PlaceSearch()
    {
        _narrow = NavView.DisplayMode != NavigationViewDisplayMode.Expanded;
        SearchBox.Visibility = _searchAvailable && !_narrow ? Visibility.Visible : Visibility.Collapsed;
        SearchButton.Visibility = _searchAvailable && _narrow ? Visibility.Visible : Visibility.Collapsed;
        QueueTitleBarRegions();
        if (!_searchAvailable || !_narrow)
        {
            CloseNarrowSearch();
        }
    }

    private void OnSearchButtonClick(object sender, RoutedEventArgs e) => OpenNarrowSearch(FocusState.Programmatic);

    private void OpenNarrowSearch(FocusState focus)
    {
        NarrowSearchBox.Text = SearchBox.Text;
        SearchDismissLayer.Visibility = Visibility.Visible;
        NarrowSearchPlate.Visibility = Visibility.Visible;
        QueueTitleBarRegions();
        // Once it has been laid out: focused in the same pass as it is shown, it did not take the focus (owner's run, 2026-10-26).
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () => NarrowSearchBox.Focus(focus));
    }

    private void CloseNarrowSearch()
    {
        if (NarrowSearchPlate.Visibility == Visibility.Visible)
        {
            SearchBox.Text = NarrowSearchBox.Text;
            NarrowSearchPlate.Visibility = Visibility.Collapsed;
            SearchDismissLayer.Visibility = Visibility.Collapsed;
            QueueTitleBarRegions();
        }
    }

    /// <summary>A click outside the open small-window search box only closes it.</summary>
    private void OnSearchDismissPressed(object sender, PointerRoutedEventArgs e)
    {
        CloseNarrowSearch();
        e.Handled = true;
    }

    private void OnNarrowSearchSubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        OnSearchSubmitted(sender, args);
        CloseNarrowSearch();
    }

    /// <summary>Esc closes the box. Seen before the box's text field, which takes Esc itself (KeyDown never came; owner's run, 2026-10-26).</summary>
    private void OnNarrowSearchKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Escape)
        {
            CloseNarrowSearch();
            SearchButton.Focus(FocusState.Keyboard);
            e.Handled = true;
        }
    }

    /// <summary>Focus went elsewhere in the window: the box closes, as Settings' does. Inside it (its text box) it stays.</summary>
    private void OnNarrowSearchLostFocus(object sender, RoutedEventArgs e)
    {
        var focused = FocusManager.GetFocusedElement(Root.XamlRoot) as DependencyObject;
        for (var element = focused; element is not null; element = VisualTreeHelper.GetParent(element))
        {
            if (ReferenceEquals(element, NarrowSearchBox))
            {
                return;
            }
        }
        CloseNarrowSearch();
    }

    /// <summary>Announces from the navigation view, which every page is shown in, so a page that was just left can still be heard.</summary>
    internal void Announce(string text, string activityId) => Shell.Announce.Say(NavView, text, activityId);

    /// <summary>Drops the back and forward history, after a finished sign-in, and keeps the title bar's back button in step.</summary>
    internal void ClearHistory()
    {
        ContentFrame.BackStack.Clear();
        ContentFrame.ForwardStack.Clear();
        UpdateBackButton();
    }

    /// <summary>
    /// A sign-in page owns the flow it was opened with and disposes it when left, so it is never returned to: its entries leave the back
    /// and forward stacks as soon as it is left. Back from a sign-in goes to the page before it; starting again opens a new one.
    /// </summary>
    private void DropSignInHistory()
    {
        foreach (var stack in new[] { ContentFrame.BackStack, ContentFrame.ForwardStack })
        {
            for (var i = stack.Count - 1; i >= 0; i--)
            {
                if (stack[i].SourcePageType == typeof(SignInPage))
                {
                    stack.RemoveAt(i);
                }
            }
        }
    }

    private void OnNavigated(object sender, NavigationEventArgs e)
    {
        LeaveEditMode();
        _shownParameter = e.Parameter;
        PruneKeptDetails();
        DropSignInHistory();
        if (App.Browsing is { } hub)
        {
            PruneGoneHistory(hub);
        }
        UpdateBackButton();
        SelectShownPage();
        RefreshButton.IsEnabled = ContentFrame.Content is IRefreshable;
    }

    private void OnRefreshClick(object sender, RoutedEventArgs e) => (ContentFrame.Content as IRefreshable)?.Refresh();

    /// <summary>
    /// The navigation's selection follows the page shown: its item, Home for Home's steps, none for search results; a detail page
    /// keeps the item it was reached from.
    /// </summary>
    private void SelectShownPage()
    {
        if (ContentFrame.SourcePageType == typeof(DetailsPage))
        {
            return;
        }
        // A view's page selects its tab: the Favourites tab's rows (on the search page), a kind's page that a Favourites row's
        // header opened under Favourites (review 2, S2), and a kind's page opened from its tab (Movies, Series) that tab.
        if (_shownParameter is ViewArgs shownView)
        {
            var tab = shownView switch
            {
                { View: BrowseView.Everything, Favourite: true } => new ViewArgs(shownView.Server, BrowseView.Favourites),
                { View: BrowseView.Everything } => shownView,
                _ => shownView with { Kind = null },
            };
            NavView.SelectedItem = new[] { MoviesItem, SeriesItem, FavouritesItem, RecentItem }.Concat(_libraryItems.OfType<NavigationViewItem>())
                .FirstOrDefault(item => item.Visibility == Visibility.Visible && Equals(item.Tag, tab));
            return;
        }
        if (ContentFrame.SourcePageType == typeof(SearchPage))
        {
            NavView.SelectedItem = null;
            return;
        }
        if (ContentFrame.SourcePageType == typeof(LibraryPage))
        {
            NavView.SelectedItem = _libraryItems.OfType<NavigationViewItem>()
                .FirstOrDefault(item => item.Tag is LibraryArgs library && library.Shows(_shownParameter));
            return;
        }
        // Each page shows its own title (App.xaml, PageTitleTextBlockStyle); the navigation view's header is not used.
        if (ContentFrame.SourcePageType == typeof(SettingsPage) || ContentFrame.SourcePageType == typeof(SupportedFormatsPage))
        {
            // The settings item is not in MenuItems and has no tag.
            NavView.SelectedItem = NavView.SettingsItem;
            return;
        }
        if (HomeSteps.Contains(ContentFrame.SourcePageType))
        {
            NavView.SelectedItem = HomeItem;
            return;
        }
        var tag = Pages.FirstOrDefault(entry => entry.Value == ContentFrame.SourcePageType).Key;
        NavView.SelectedItem = NavView.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => Equals(i.Tag, tag));
    }

    private bool TryGoBack()
    {
        // Back from the player closes it and shows the page it was played from (ADR 0007's closing order, PlayerView.CloseAsync).
        if (Player.IsOpen)
        {
            _ = Player.CloseAsync();
            return true;
        }
        // With the pane open over the page, back only closes the pane, from the title bar's Back button, Alt+Left, the GoBack key and
        // the mouse Back button alike (owner, 2026-10-24), as the navigation view's own back button does then ("clicking the back
        // button will close the Pane and raise the PaneClosing event instead", NavigationView page, learn.microsoft.com, ms.date
        // 2026-10-02, read 2026-10-24).
        if (PaneOverlaysPage)
        {
            NavView.IsPaneOpen = false;
            return true;
        }
        if (!ContentFrame.CanGoBack)
        {
            return false;
        }
        ContentFrame.GoBack();
        return true;
    }

    // The Frame drops its forward stack itself when a new page is navigated to.
    private bool TryGoForward()
    {
        if (!ContentFrame.CanGoForward)
        {
            return false;
        }
        ContentFrame.GoForward();
        return true;
    }

    private void OnTitleBarBackRequested(TitleBar sender, object args) => TryGoBack();

    private void OnTitleBarPaneToggleRequested(TitleBar sender, object args) => NavView.IsPaneOpen = !NavView.IsPaneOpen;

    private void OnBackAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args) =>
        args.Handled = TryGoBack();

    private void OnForwardAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args) =>
        args.Handled = TryGoForward();

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (e.Pointer.PointerDeviceType != PointerDeviceType.Mouse)
        {
            return;
        }
        var buttons = e.GetCurrentPoint(Root).Properties;
        if (buttons.IsXButton1Pressed)
        {
            e.Handled = TryGoBack();
        }
        else if (buttons.IsXButton2Pressed)
        {
            e.Handled = TryGoForward();
        }
    }
}
