using System.Collections.ObjectModel;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// The results of a search from the navigation's search box: every signed-in server's best matches, grouped by kind (ADR 0018). The
/// searches are the core's; they are opened when the page is shown and closed when it is left.
/// </summary>
public sealed partial class SearchPage : Page
{
    private readonly ObservableCollection<SearchSection> _sections = [];

    public SearchPage()
    {
        InitializeComponent();
        Sections.ItemsSource = _sections;
        // Released once the page has left the window: during its exit transition it still shows its rows.
        Loaded += (_, _) => _wasLoaded = true;
        Unloaded += (_, _) => Release();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        var term = (e.Parameter as SearchArgs)?.Term ?? "";
        TitleText.Text = $"Results for “{term}”";
        if (App.Browsing is not { } hub || term.Length == 0)
        {
            return;
        }
        var ui = new DispatcherQueueUi(DispatcherQueue);
        var servers = hub.Servers;
        foreach (var server in servers)
        {
            var header = servers.Count > 1 ? hub.ServerName(server.Server) : "";
            var section = new SearchSection(header, server.Session.Search(term), ui);
            section.PropertyChanged += (_, _) => ShowStatus();
            section.Rows.CollectionChanged += (_, _) => ShowStatus();
            _sections.Add(section);
        }
        ShowStatus();
    }

    /// <summary>The page reached the window: its lists go on Unloaded. One navigated from before it loaded has no Unloaded.</summary>
    private bool _wasLoaded;

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (!_wasLoaded)
        {
            // Left before it ever loaded (navigated past at once): no Unloaded will come.
            DispatcherQueue.TryEnqueue(Release);
        }
    }

    private void Release()
    {
        foreach (var section in _sections)
        {
            section.Dispose();
        }
        _sections.Clear();
    }

    /// <summary>"Searching…", then nothing once there are results, "No results", or why the search failed.</summary>
    private void ShowStatus()
    {
        var loading = _sections.Any(section => section.State.Loading);
        var found = _sections.Sum(section => section.Rows.Count);
        var failed = _sections.Select(section => section.State.Failed).FirstOrDefault(problem => problem is not null);
        LoadingRing.IsActive = loading && found == 0;
        var text = found > 0 ? ""
            : loading ? "Searching…"
            : failed is not null ? $"Couldn't search. {BrowseText.Problem(failed)}"
            : "No results";
        if (StatusText.Text != text)
        {
            StatusText.Text = text;
            StatusText.Visibility = text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
            // A live region is read only when its peer raises LiveRegionChanged (Announce.Shown): "No results" was not heard
            // (the owner's run, 2026-10-25).
            if (text.Length > 0)
            {
                Announce.Shown(StatusText);
            }
        }
    }

    private void OnRowLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is ListViewBase list)
        {
            HomeSection.AttachOnce(list, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
        }
    }
}
