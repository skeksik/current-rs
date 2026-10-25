using System.Collections.ObjectModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// Home: with a signed-in Jellyfin server, its rows (ADR 0018, section 6: Continue watching, Next up, Latest per library; a row
/// shows once it has items); without one, the M1 states: an invitation to add a server, or the saved servers with their user. It
/// waits for the core to open (App.CoreReady); its subscriptions live from Loaded to Unloaded (ADR 0004). The feeds themselves stay
/// with their sessions (BrowseHub), so Home shows again without reloading, and refreshes when shown after five minutes.
/// </summary>
public sealed partial class HomePage : Page
{
    private static readonly TimeSpan OpeningDelay = TimeSpan.FromMilliseconds(500);
    private static readonly TimeSpan RefreshAfter = TimeSpan.FromMinutes(5);
    private bool _loaded;
    private readonly ObservableCollection<HomeSection> _sections = [];

    public HomePage()
    {
        InitializeComponent();
        Sections.ItemsSource = _sections;
        // Shown again with a session: the feed and its rows from the first frame, not the M1 content first.
        if (App.Browsing is { } hub)
        {
            ShowFeed(hub, refreshStale: false);
        }
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    public ServersViewModel? ViewModel { get; private set; }

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
            if (App.Browsing is { } hub)
            {
                hub.Changed += OnBrowseChanged;
                ShowFeed(hub, refreshStale: true);
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
        // The sections stay: the page is kept (NavigationCacheMode Required), and so are its rows.
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
        var wanted = servers.Select(server => server.HomeSection).ToList();
        foreach (var server in servers)
        {
            server.HomeSection.Header = servers.Count > 1 ? hub.ServerName(server.Server) : "";
        }
        if (!_sections.SequenceEqual(wanted))
        {
            ClearSections();
            foreach (var section in wanted)
            {
                _sections.Add(section);
            }
        }
        foreach (var server in servers)
        {
            if (refreshStale && DateTime.UtcNow - server.HomeLastShown > RefreshAfter)
            {
                server.Home.Refresh();
            }
            server.HomeLastShown = DateTime.UtcNow;
        }
    }

    /// <summary>Lets go of the sections; they stay with their sessions (ServerBrowse.HomeSection).</summary>
    private void ClearSections() => _sections.Clear();

    private void OnRowLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is ListViewBase list)
        {
            HomeSection.AttachOnce(list, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item, item)));
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
        void OnChanged(object? s, System.ComponentModel.PropertyChangedEventArgs args) => Show();
        section.PropertyChanged += OnChanged;
        // The section outlives the page: its handler goes with the bar.
        void OnUnloaded(object s, RoutedEventArgs args)
        {
            section.PropertyChanged -= OnChanged;
            info.Unloaded -= OnUnloaded;
        }
        info.Unloaded += OnUnloaded;
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
