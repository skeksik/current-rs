using System.Collections.ObjectModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// Home: with a signed-in Jellyfin server, its rows (ADR 0018, section 6: Continue watching, Next up, Latest per library; a row
/// shows once it has items); without one, the M1 states: an invitation to add a server, or the saved servers with their user. It
/// waits for the core to open (App.CoreReady); its subscriptions live from Loaded to Unloaded (ADR 0004). The feeds themselves stay
/// with their sessions (BrowseHub), so Home shows again without reloading, and refreshes when shown after five minutes. Back and
/// Forward return to where Home was left, its vertical offset and each row's sideways one, as a library's grid returns to its place;
/// opened from the navigation it starts at the top.
/// </summary>
public sealed partial class HomePage : Page
{
    private static readonly TimeSpan OpeningDelay = TimeSpan.FromMilliseconds(500);
    private static readonly TimeSpan RefreshAfter = TimeSpan.FromMinutes(5);
    private bool _loaded;
    private readonly ObservableCollection<HomeSection> _sections = [];

    /// <summary>The rows' lists now in the window, to read their sideways offsets when Home is left.</summary>
    private readonly List<ListViewBase> _rowLists = [];

    /// <summary>Reached by Back or Forward: return to the saved offsets.</summary>
    private bool _restore;

    /// <summary>How long a return to a saved offset waits for the rows to be laid out far enough.</summary>
    private static readonly TimeSpan RestoreWait = TimeSpan.FromSeconds(1);

    public HomePage()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    public ServersViewModel? ViewModel { get; private set; }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        _restore = e.NavigationMode is NavigationMode.Back or NavigationMode.Forward;
        if (!_restore && App.Browsing is { } hub)
        {
            hub.HomeOffset = 0;
            hub.HomeRowOffsets.Clear();
        }
    }

    /// <summary>Before the exit transition, while the rows are still in place: where Home is left.</summary>
    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (App.Browsing is not { } hub)
        {
            return;
        }
        hub.HomeOffset = Feed.VerticalOffset;
        hub.HomeRowOffsets.Clear();
        foreach (var list in _rowLists)
        {
            if (RowKey(list) is { } key && ScrollerOf(list) is { } scroller)
            {
                hub.HomeRowOffsets[key] = scroller.HorizontalOffset;
            }
        }
    }

    /// <summary>A row's key across servers: its server's place on Home and the core's key for the row.</summary>
    private string? RowKey(ListViewBase list)
    {
        if (list.DataContext is not TileRow row)
        {
            return null;
        }
        var section = _sections.IndexOf(_sections.FirstOrDefault(s => s.Rows.Contains(row))!);
        return section < 0 ? null : $"{section}/{row.Key}";
    }

    /// <summary>The list's own ScrollViewer, the template part it scrolls with.</summary>
    private static ScrollViewer? ScrollerOf(DependencyObject parent)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is ScrollViewer scroller)
            {
                return scroller;
            }
            if (ScrollerOf(child) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>
    /// Scrolls to <paramref name="offset"/> once the content reaches it: the rows arrive and lay out a moment after the page is
    /// shown, so it tries again each frame, for at most <see cref="RestoreWait"/>. Without animation, as a return to a place is.
    /// </summary>
    private async Task ScrollWhenLaidOutAsync(ScrollViewer scroller, double offset, bool horizontal)
    {
        var deadline = DateTime.UtcNow + RestoreWait;
        while (_loaded && DateTime.UtcNow < deadline)
        {
            var reach = horizontal ? scroller.ScrollableWidth : scroller.ScrollableHeight;
            if (reach >= offset - 0.5)
            {
                if (horizontal)
                {
                    scroller.ChangeView(offset, null, null, true);
                }
                else
                {
                    scroller.ChangeView(null, offset, null, true);
                }
                return;
            }
            await Task.Delay(16);
        }
    }

    // async void, as event handlers are: it catches everything itself.
    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        _loaded = true;
        try
        {
            var ready = App.CoreReady;
            if (!ready.IsCompleted)
            {
                // A ring only if opening takes a moment, so a fast start does not flash one.
                if (await Task.WhenAny(ready, Task.Delay(OpeningDelay)) != ready)
                {
                    Opening.IsActive = true;
                }
            }
            var core = await ready;
            Opening.IsActive = false;
            if (!_loaded)
            {
                return;
            }
            if (core is null)
            {
                CoreFailed.Visibility = Visibility.Visible;
                return;
            }
            ViewModel ??= new ServersViewModel(core, new DispatcherQueueUi(DispatcherQueue));
            ViewModel.Start();
            Bindings.Update();
            Sections.ItemsSource = _sections;
            if (App.Browsing is { } hub)
            {
                hub.Changed += OnBrowseChanged;
                ShowFeed(hub, refreshStale: true);
                if (_restore && hub.HomeOffset > 0)
                {
                    _ = ScrollWhenLaidOutAsync(Feed, hub.HomeOffset, horizontal: false);
                }
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the home page failed to load: {error.GetType().Name}");
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        _loaded = false;
        ViewModel?.Stop();
        if (App.Browsing is { } hub)
        {
            hub.Changed -= OnBrowseChanged;
            foreach (var server in hub.Servers)
            {
                server.HomeLastShown = DateTime.UtcNow;
            }
        }
        ClearSections();
    }

    private void OnBrowseChanged(object? sender, EventArgs e)
    {
        if (_loaded && App.Browsing is { } hub)
        {
            ShowFeed(hub, refreshStale: false);
        }
    }

    /// <summary>One section per signed-in server, kept while its session lasts; the M1 content while there is none.</summary>
    private void ShowFeed(BrowseHub hub, bool refreshStale)
    {
        var servers = hub.Servers;
        Feed.Visibility = hub.IsAvailable ? Visibility.Visible : Visibility.Collapsed;
        M1Content.Visibility = hub.IsAvailable ? Visibility.Collapsed : Visibility.Visible;
        TitleText.Style = (Style)Application.Current.Resources[
            hub.IsAvailable ? "WidePageTitleTextBlockStyle" : "PageTitleTextBlockStyle"];
        var feeds = servers.Select(server => server.Home).ToList();
        if (_sections.Count == feeds.Count && _sections.Select(section => section.Feed).SequenceEqual(feeds))
        {
            return;
        }
        ClearSections();
        var ui = new DispatcherQueueUi(DispatcherQueue);
        foreach (var server in servers)
        {
            var header = servers.Count > 1 ? hub.ServerName(server.Server) : "";
            _sections.Add(new HomeSection(header, server.Home, ui));
            if (refreshStale && DateTime.UtcNow - server.HomeLastShown > RefreshAfter)
            {
                server.Home.Refresh();
            }
            server.HomeLastShown = DateTime.UtcNow;
        }
    }

    private void ClearSections()
    {
        foreach (var section in _sections)
        {
            section.Dispose();
        }
        _sections.Clear();
    }

    private void OnRowLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is not ListViewBase list)
        {
            return;
        }
        HomeSection.AttachOnce(list, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
        if (!_rowLists.Contains(list))
        {
            _rowLists.Add(list);
            list.Unloaded += (_, _) => _rowLists.Remove(list);
        }
        if (_restore && App.Browsing is { } hub && RowKey(list) is { } key
            && hub.HomeRowOffsets.TryGetValue(key, out var offset) && offset > 0 && ScrollerOf(list) is { } scroller)
        {
            _ = ScrollWhenLaidOutAsync(scroller, offset, horizontal: true);
        }
    }

    /// <summary>A section's message: rows that failed (with Retry), or the server unreachable (its last rows stay).</summary>
    private void OnSectionInfoLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is not InfoBar { Tag: HomeSection section } info)
        {
            return;
        }
        void Show()
        {
            var state = section.State;
            if (state.Offline)
            {
                SetInfo(info, InfoBarSeverity.Informational, "Can't reach the server.", "Showing what was loaded last.", retry: true);
            }
            else if (state.Failed is { } failed)
            {
                SetInfo(info, InfoBarSeverity.Error, "Some rows couldn't be loaded.", BrowseText.Problem(failed), retry: true);
            }
            else
            {
                info.IsOpen = false;
            }
        }
        section.PropertyChanged += (_, _) => Show();
        Show();
    }

    /// <summary>Opened again for a new message, so Narrator reads it (InfoBar page, learn.microsoft.com, ms.date 2025-02-26).</summary>
    private static void SetInfo(InfoBar info, InfoBarSeverity severity, string title, string message, bool retry)
    {
        if (info.IsOpen && info.Title == title && info.Message == message)
        {
            return;
        }
        info.IsOpen = false;
        info.Severity = severity;
        info.Title = title;
        info.Message = message;
        info.ActionButton.Visibility = retry ? Visibility.Visible : Visibility.Collapsed;
        info.IsOpen = true;
    }

    private void OnRefreshAccelerator(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        foreach (var section in _sections)
        {
            section.Feed.Refresh();
        }
        args.Handled = _sections.Count > 0;
    }

    private void OnRetrySection(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is HomeSection section)
        {
            section.Feed.Retry();
        }
    }

    private void OnAddServer(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(AddServerPage));

    private void OnReload(object sender, RoutedEventArgs e) => ViewModel?.Reload();

    private void OnSignInAgain(object sender, RoutedEventArgs e)
    {
        if (App.Core is { } core && (sender as FrameworkElement)?.DataContext is ServerRow row)
        {
            SignInRequest.SignInAgain(Frame, core, row);
        }
    }
}
