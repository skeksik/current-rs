using System.Diagnostics;
using Current.Shell.Pages;
using Microsoft.UI.Input;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Navigation;
using Windows.System;

namespace Current.Shell;

/// <summary>
/// The main window: Mica, the content extended into the title bar with the system caption buttons, and a NavigationView over one
/// Frame. Navigation follows the NavigationView page's example (learn.microsoft.com, ms.date 2026-10-02, read 2026-10-22): navigate
/// on ItemInvoked without duplicate back stack entries, go back through the Frame, and keep the selection in step on Navigated.
/// </summary>
public sealed partial class MainWindow : Window
{
    private static readonly Dictionary<string, Type> Pages = new() { ["home"] = typeof(HomePage) };

    /// <summary>Pages reached from Home (adding a server, signing in): Home stays selected, the header names the step.</summary>
    private static readonly Dictionary<Type, string> HomeSteps = new()
    {
        [typeof(AddServerPage)] = "Add a server",
        [typeof(SignInPage)] = "Sign in",
    };

    /// <summary>
    /// The smallest window, in effective pixels: the narrowest page column (Sign in, 400 plus 24 on each side) and the title bar with
    /// its back and pane buttons, title and caption buttons fit (manager, 2026-10-24; the WinUI Gallery uses
    /// 640 by 500, github.com/microsoft/WinUI-Gallery MainWindow.xaml.cs, main at 14061ee, read 2026-10-24).
    /// </summary>
    private const double MinWindowWidth = 500;

    private const double MinWindowHeight = 500;

    private bool _wasDeactivated;

    private FrameworkElement? _paneToggleButton;

    /// <summary>The navigation view's content layer: ContentGrid, the content of its template part RootSplitView.</summary>
    private Grid? _contentLayer;

    private readonly SolidColorBrush _transparent = new(Microsoft.UI.Colors.Transparent);

    private readonly TaskCompletionSource _shown = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        // The caption buttons keep their standard height (PreferredHeightOption stays Standard): Windows Settings' are 46 × 32 px in
        // its 48 px title bar (owner's run, 2026-10-24).

        // XAML cannot parse "GoBack" as a VirtualKey (XamlParseException in CI), so this one is added here.
        var goBack = new KeyboardAccelerator { Key = VirtualKey.GoBack };
        goBack.Invoked += OnBackAccelerator;
        Root.KeyboardAccelerators.Add(goBack);

        // handledEventsToo: a control under the pointer may already have handled the press.
        Root.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler(OnPointerPressed), true);

        Activated += OnActivated;
        Root.Loaded += OnRootLoaded;
        // A handler of its own, so the one that keeps the selection and header in step stays as it is.
        ContentFrame.Navigated += (_, _) => ApplyContentLayer();

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

    /// <summary>Completes when the window's content has loaded and the navigation view's template is applied (the smoke run).</summary>
    internal Task Shown => _shown.Task;

    /// <summary>The content layer's background now, or null if its template part was not found (the smoke run).</summary>
    internal Brush? ContentLayerBackground => _contentLayer?.Background;

    /// <summary>Opens Settings, as its item in the pane does (the smoke run).</summary>
    internal void ShowSettings() => Navigate(typeof(SettingsPage), new SuppressNavigationTransitionInfo());

    private void OnRootLoaded(object sender, RoutedEventArgs e)
    {
        Root.Loaded -= OnRootLoaded;
        // The template is applied by now: a missing part is only traced, and the page keeps the control's default look.
        ApplyContentLayer();
        if (_contentLayer is null)
        {
            Trace.TraceWarning("Current: NavigationView template part RootSplitView not found; every page keeps the content layer");
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
    /// Shows the content layer behind content pages and Mica behind pages of settings cards (PageLayout.HasContentLayer), and places
    /// the page's title, notices and column for it. The layer is the template's ContentGrid, whose Background and BorderBrush come
    /// from ThemeResources (NavigationViewContentBackground, NavigationViewContentGridBorderBrush; NavigationView.xaml,
    /// microsoft-ui-xaml, tag winui3/release/2.5.1, read 2026-10-24) and no visual state sets either. Values set by a template are
    /// local values in WinUI's property system, which knows only default, style and local sources (GetBaseValueSource,
    /// PropertySystem.cpp, same tag), so ClearValue would leave no brush at all: that hid the layer on every page in the owner's run
    /// (2026-10-24). Content pages bind both to ContentLayerBrushes (MainWindow.xaml) instead, which follows the theme.
    /// </summary>
    private void ApplyContentLayer()
    {
        if (ContentFrame.Content is not Page page)
        {
            return;
        }
        var hasLayer = PageLayout.GetHasContentLayer(page);
        PageLayout.SetContentLayer(hasLayer);
        Notices.Margin = PageLayout.NoticesMargin;

        // The template's items name parts "ContentGrid" too, so the layer is taken from the SplitView it is the content of.
        _contentLayer ??= (FindDescendant(NavView, "RootSplitView") as SplitView)?.Content as Grid;
        if (_contentLayer is null)
        {
            return;
        }
        if (hasLayer)
        {
            _contentLayer.SetBinding(Panel.BackgroundProperty, new Binding { Source = ContentLayerBrushes, Path = new PropertyPath("Background") });
            _contentLayer.SetBinding(Grid.BorderBrushProperty, new Binding { Source = ContentLayerBrushes, Path = new PropertyPath("BorderBrush") });
        }
        else
        {
            _contentLayer.Background = _transparent;
            _contentLayer.BorderBrush = _transparent;
        }
    }

    private void OnPaneOpenedOrClosed(NavigationView sender, object args) => NamePaneToggleButton();

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
        else if (args.InvokedItemContainer?.Tag is string tag && Pages.TryGetValue(tag, out var page))
        {
            Navigate(page, args.RecommendedNavigationTransitionInfo);
        }
    }

    private void Navigate(Type page, NavigationTransitionInfo transition)
    {
        // Only navigate if the page is not already shown, so the back stack gets no duplicates.
        if (ContentFrame.CurrentSourcePageType != page)
        {
            ContentFrame.Navigate(page, null, transition);
        }
    }

    /// <summary>Drops the back and forward history, after a finished sign-in, and keeps the title bar's back button in step.</summary>
    internal void ClearHistory()
    {
        ContentFrame.BackStack.Clear();
        ContentFrame.ForwardStack.Clear();
        AppTitleBar.IsBackButtonEnabled = ContentFrame.CanGoBack;
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
        DropSignInHistory();
        AppTitleBar.IsBackButtonEnabled = ContentFrame.CanGoBack;
        if (ContentFrame.SourcePageType == typeof(SettingsPage))
        {
            // The settings item is not in MenuItems and has no tag.
            NavView.SelectedItem = NavView.SettingsItem;
            NavView.Header = "Settings";
            return;
        }
        if (HomeSteps.TryGetValue(ContentFrame.SourcePageType, out var step))
        {
            NavView.SelectedItem = HomeItem;
            NavView.Header = step;
            return;
        }
        var tag = Pages.FirstOrDefault(entry => entry.Value == ContentFrame.SourcePageType).Key;
        var item = NavView.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => Equals(i.Tag, tag));
        NavView.SelectedItem = item;
        NavView.Header = item?.Content;
    }

    private bool TryGoBack()
    {
        if (!ContentFrame.CanGoBack)
        {
            return false;
        }
        // With the pane open over the page, back closes the pane and goes back (owner, 2026-10-24). The NavigationView page's example
        // does not go back then, and the control's own back button only closes the pane ("clicking the back button will close the
        // Pane and raise the PaneClosing event instead", learn.microsoft.com, ms.date 2026-10-02, read 2026-10-24); that blocked the
        // title bar's Back button and Alt+Left here.
        if (NavView.IsPaneOpen && NavView.DisplayMode is NavigationViewDisplayMode.Compact or NavigationViewDisplayMode.Minimal)
        {
            NavView.IsPaneOpen = false;
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
