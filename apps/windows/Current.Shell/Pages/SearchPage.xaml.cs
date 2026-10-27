using System.Collections.ObjectModel;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// The results of a search from the navigation's search box: every signed-in server's best matches, grouped by kind (ADR 0018). The
/// searches are the core's, kept with the page's history entry (BrowseHub.Search): Back shows the results the page had. The
/// Favourites tab (a <see cref="ViewArgs"/>) shows its rows the same way, a row per kind whose header opens that kind's whole list
/// (owner, 2026-10-27); they are kept with the server's browse, as Home's rows are.
/// </summary>
public sealed partial class SearchPage : Page, IRefreshable
{
    /// <summary>How many rows the page shows now (the smoke run).</summary>
    internal int RowCount => _sections.Sum(section => section.Rows.Count);

    /// <summary>The Favourites tab shown, or null for a search.</summary>
    private ViewArgs? _view;

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
        if (e.Parameter is ViewArgs view)
        {
            _view = view;
            TitleText.Text = Views.Title(view.View);
            if (App.Browsing?.Find(view.Server) is { } server)
            {
                Add(server.Favourites);
            }
            ShowStatus();
            return;
        }
        _view = null;
        var term = (e.Parameter as SearchArgs)?.Term ?? "";
        TitleText.Text = $"Results for “{term}”";
        if (App.Browsing is not { } hub || term.Length == 0)
        {
            return;
        }
        foreach (var section in hub.Search(e.Parameter!, term))
        {
            Add(section);
        }
        ShowStatus();
    }

    private void Add(SearchSection section)
    {
        section.PropertyChanged += OnSectionChanged;
        section.Rows.CollectionChanged += OnRowsChanged;
        _sections.Add(section);
    }

    private void OnSectionChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e) => ShowStatus();

    private void OnRowsChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e) => ShowStatus();

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

    /// <summary>Lets go of the sections; they stay with the history entry (BrowseHub.Search), which disposes them.</summary>
    private void Release()
    {
        foreach (var section in _sections)
        {
            section.PropertyChanged -= OnSectionChanged;
            section.Rows.CollectionChanged -= OnRowsChanged;
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
        var text = _view is not null
            ? (BrowseLabels.FavouritesStatus(found, loading, failed is not null) is not { } status ? ""
                : failed is not null ? $"{status} {BrowseText.Problem(failed)}" : status)
            : found > 0 ? ""
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

    /// <summary>F5 and Ctrl+R: every server's search (or the favourites) runs again, keeping its results until the answers arrive.</summary>
    private void OnRefreshAccelerator(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        Refresh();
        args.Handled = _sections.Count > 0;
    }

    /// <summary>The title bar's Refresh button, as F5.</summary>
    public void Refresh()
    {
        foreach (var section in _sections)
        {
            section.Refresh();
        }
    }

    /// <summary>A Favourites row's header: every item of its kind, a grid with its own view, sort and filter, favourites ticked.</summary>
    private void OnRowHeader(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is TileRow { View: { } view } row && _view is { } shown)
        {
            var args = new ViewArgs(shown.Server, view, row.Kind, row.Favourite);
            Frame.Navigate(args.Page, args);
        }
    }

    private void OnSectionRowsLoaded(object sender, RoutedEventArgs e) =>
        StableSource.Follow<SearchSection>((ItemsControl)sender, section => (section.Rows, null));

    private void OnRowLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is ListViewBase list)
        {
            HomeSection.AttachOnce(list, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
            StableSource.Follow<TileRow>(list, row => (row.Items, row.ItemTemplate));
        }
    }
}
