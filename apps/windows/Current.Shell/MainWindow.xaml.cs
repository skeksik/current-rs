using System.Diagnostics;
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
public sealed partial class MainWindow : Window
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
            _appTheme?.Dispose();
            _appTheme = null;
        };
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

    /// <summary>The page shown now (the smoke run).</summary>
    internal Page? ShownPage => ContentFrame.Content as Page;

    /// <summary>The navigation's library items now, headers left out (the smoke run).</summary>
    internal int LibraryItemCount => _libraryItems.OfType<NavigationViewItem>().Count();

    /// <summary>Opens Settings, as its item in the pane does (the smoke run).</summary>
    internal void ShowSettings() => Navigate(typeof(SettingsPage), new SuppressNavigationTransitionInfo());

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

    private void OnDisplayModeChanged(NavigationView sender, NavigationViewDisplayModeChangedEventArgs args) => QueuePaneStateUpdate();

    /// <summary>
    /// Updates the pane button's name and Back once the navigation view has finished the change. Crossing 1008 px, it raises
    /// DisplayModeChanged while the full pane is still open and closes or reopens the pane after; read at once, the pane counted as
    /// over the page for that moment and Back flashed enabled (owner's recording, 2026-10-24).
    /// </summary>
    private void QueuePaneStateUpdate() => DispatcherQueue.TryEnqueue(() =>
    {
        NamePaneToggleButton();
        UpdateBackButton();
    });

    /// <summary>
    /// The pane open over the page (Compact; Minimal is never reached, CompactModeThresholdWidth is 0): it counts as a back step,
    /// which back closes. The full pane inline on a wide window does not (owner, 2026-10-24).
    /// </summary>
    private bool PaneOverlaysPage => NavView.IsPaneOpen && NavView.DisplayMode == NavigationViewDisplayMode.Compact;

    /// <summary>The title bar's Back button is enabled while there is a page to go back to or the pane overlays the page.</summary>
    private void UpdateBackButton() => AppTitleBar.IsBackButtonEnabled = ContentFrame.CanGoBack || PaneOverlaysPage;

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
        if (args.IsSettingsInvoked)
        {
            Navigate(typeof(SettingsPage), args.RecommendedNavigationTransitionInfo);
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
    /// server is signed in, and pages of a server that is gone leave the history (a page shown now goes back to Home). Called once
    /// the core is open.
    /// </summary>
    internal void FollowBrowsing(BrowseHub hub)
    {
        hub.Changed += (_, _) => ShowBrowsing(hub);
        ShowBrowsing(hub);
    }

    private readonly List<object> _libraryItems = [];

    private void ShowBrowsing(BrowseHub hub)
    {
        SearchBox.Visibility = hub.IsAvailable ? Visibility.Visible : Visibility.Collapsed;
        foreach (var item in _libraryItems)
        {
            NavView.MenuItems.Remove(item);
        }
        _libraryItems.Clear();
        var servers = hub.Servers;
        foreach (var server in servers)
        {
            if (servers.Count > 1)
            {
                _libraryItems.Add(new NavigationViewItemHeader { Content = hub.ServerName(server.Server) });
            }
            foreach (var library in server.Libraries)
            {
                _libraryItems.Add(new NavigationViewItem
                {
                    Content = library.Name,
                    Tag = new LibraryArgs(library),
                    Icon = new FontIcon { Glyph = LibraryGlyph(library.Collection) },
                });
            }
        }
        foreach (var item in _libraryItems)
        {
            NavView.MenuItems.Add(item);
        }
        DropHistoryOfGoneServers(hub);
        SelectShownPage();
    }

    /// <summary>
    /// A library's icon by what it holds, from Segoe Fluent Icons ("Segoe Fluent Icons font", learn.microsoft.com, ms.date
    /// 2026-10-13, read 2026-10-25): E8B2 Movies, E7F4 TVMonitor, E8D6 Audio, E714 Video, E91B Photo, E82D Dictionary, E8F1
    /// Library, E8B7 Folder.
    /// </summary>
    private static string LibraryGlyph(string? collection) => collection switch
    {
        "movies" => "\uE8B2",
        "tv_shows" => "\uE7F4",
        "music" => "\uE8D6",
        "music_videos" or "home_videos" => "\uE714",
        "photos" => "\uE91B",
        "books" => "\uE82D",
        "box_sets" or "playlists" => "\uE8F1",
        _ => "\uE8B7",
    };

    private static ServerRef? ServerOf(object? parameter) => parameter switch
    {
        LibraryArgs library => library.Library.Item.Server,
        DetailsArgs details => details.Item.Server,
        _ => null,
    };

    /// <summary>Pages of a server whose session ended leave the back and forward stacks; the page shown now goes to Home.</summary>
    private void DropHistoryOfGoneServers(BrowseHub hub)
    {
        bool Gone(Type? page, object? parameter) =>
            (ServerOf(parameter) is { } server && hub.Find(server) is null) || (page == typeof(SearchPage) && !hub.IsAvailable);
        foreach (var stack in new[] { ContentFrame.BackStack, ContentFrame.ForwardStack })
        {
            for (var i = stack.Count - 1; i >= 0; i--)
            {
                if (Gone(stack[i].SourcePageType, stack[i].Parameter))
                {
                    stack.RemoveAt(i);
                }
            }
        }
        if (Gone(ContentFrame.SourcePageType, _shownParameter))
        {
            ContentFrame.Navigate(typeof(HomePage), null, new SuppressNavigationTransitionInfo());
        }
        UpdateBackButton();
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
        if (SearchBox.Visibility == Visibility.Visible)
        {
            // In the icon rail the box shows as a search button; whether focusing it there opens the pane is not verified.
            args.Handled = SearchBox.Focus(FocusState.Keyboard);
        }
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
        _shownParameter = e.Parameter;
        DropSignInHistory();
        UpdateBackButton();
        SelectShownPage();
    }

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
        if (ContentFrame.SourcePageType == typeof(SettingsPage))
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
