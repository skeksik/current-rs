using System.ComponentModel;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// A library's grid (ADR 0018, section 3): a GridView over the core's slots, sized to the library's total from the first page,
/// loading the pages it shows (SlotCollection's IItemsRangeInfo). The grid's PagedItems lives in the hub for the whole session, so
/// coming back to the page binds to the slots already loaded and scrolls to where the user left; a refresh only replaces slots, so
/// the scroll position stays.
/// </summary>
public sealed partial class LibraryPage : Page
{
    /// <summary>"The page is shown again after five minutes" refreshes (ADR 0018, section 3).</summary>
    private static readonly TimeSpan RefreshAfter = TimeSpan.FromMinutes(5);

    /// <summary>The sort choices in SortBox's order: the key and its order (names A to Z, the newest and best first).</summary>
    private static readonly (string Key, SortOrder Order)[] Sorts =
    [
        ("sort_name", SortOrder.Ascending),
        ("date_created", SortOrder.Descending),
        ("premiere_date", SortOrder.Descending),
        ("community_rating", SortOrder.Descending),
    ];

    /// <summary>The filter choices in FilterBox's order.</summary>
    private static readonly string?[] Filters = [null, "unplayed", "played", "favourite"];

    private LibraryGrid? _grid;
    private SlotCollection? _slots;
    private ValueSubscription<PagedState>? _state;
    private bool _showingQuery;

    public LibraryPage()
    {
        InitializeComponent();
        ItemActions.Attach(Items, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is not LibraryArgs args
            || App.Browsing?.Find(args.Library.Server) is not { } server
            || App.Browsing.FindLibrary(args.Library) is not { } library)
        {
            // The server was signed out of while this page was in the history.
            EmptyText.Visibility = Visibility.Visible;
            return;
        }
        TitleText.Text = library.Name;
        _grid = server.Grid(library);
        ShowQuery(_grid.Query);
        var ui = new DispatcherQueueUi(DispatcherQueue);
        _state = _grid.Items.ObserveState(ui);
        _state.PropertyChanged += OnStateChanged;
        _slots = SlotCollection.Open(_grid.Items, ui);
        _slots.CollectionChanged += OnFirstSlots;
        Items.ItemsSource = _slots;
        ShowState();
        if (DateTime.UtcNow - _grid.LastShown > RefreshAfter)
        {
            _grid.Items.Refresh();
        }
        _grid.LastShown = DateTime.UtcNow;
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (_grid is not null)
        {
            _grid.FirstVisible = (Items.ItemsPanelRoot as ItemsWrapGrid)?.FirstVisibleIndex ?? 0;
            _grid.LastShown = DateTime.UtcNow;
        }
        if (_state is not null)
        {
            _state.PropertyChanged -= OnStateChanged;
            _state.Dispose();
            _state = null;
        }
        Items.ItemsSource = null;
        _slots?.Dispose();
        _slots = null;
    }

    /// <summary>Once the slots first arrive (one Reset, with every loaded page), back to the item the user left the grid at.</summary>
    private void OnFirstSlots(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
    {
        if (_slots is not { } slots || slots.Count == 0)
        {
            return;
        }
        slots.CollectionChanged -= OnFirstSlots;
        var index = Math.Min(_grid?.FirstVisible ?? 0, slots.Count - 1);
        if (index > 0)
        {
            Items.ScrollIntoView(slots[index], ScrollIntoViewAlignment.Leading);
        }
    }

    private void OnStateChanged(object? sender, PropertyChangedEventArgs e) => ShowState();

    private void ShowState()
    {
        if (_state?.Value is not { } state)
        {
            return;
        }
        LoadingRing.IsActive = state.Loading && state.Total == 0;
        EmptyText.Visibility = !state.Loading && state.Total == 0 && state.Failed is null ? Visibility.Visible : Visibility.Collapsed;
        if (state.Failed is { } failed)
        {
            var title = state.Total == 0 ? "Couldn't load this library." : "Some items couldn't be loaded.";
            var message = BrowseText.Problem(failed);
            // Opened again for a new message, so Narrator reads it: "If you would like to inform users that use screen readers of
            // the InfoBar's updated content we recommend you close and re-open the control" (InfoBar page, learn.microsoft.com,
            // ms.date 2025-02-26, read 2026-10-25).
            if (!StateInfo.IsOpen || StateInfo.Title != title || StateInfo.Message != message)
            {
                StateInfo.IsOpen = false;
                StateInfo.Severity = InfoBarSeverity.Error;
                StateInfo.Title = title;
                StateInfo.Message = message;
                StateInfo.IsOpen = true;
            }
        }
        else
        {
            StateInfo.IsOpen = false;
        }
    }

    private void ShowQuery(ItemQuery query)
    {
        _showingQuery = true;
        SortBox.SelectedIndex = Math.Max(0, Array.FindIndex(Sorts, sort => query.Sort.FirstOrDefault() == sort.Key));
        FilterBox.SelectedIndex = Math.Max(0, Array.IndexOf(Filters, query.Filters.FirstOrDefault()));
        _showingQuery = false;
    }

    /// <summary>A new sort or filter: a new query, which resets the grid to the top.</summary>
    private void OnQueryChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_showingQuery || _grid is null || SortBox.SelectedIndex < 0 || FilterBox.SelectedIndex < 0)
        {
            return;
        }
        var (key, order) = Sorts[SortBox.SelectedIndex];
        var filter = Filters[FilterBox.SelectedIndex];
        var query = _grid.Query with { Sort = [key], Order = order, Filters = filter is null ? [] : [filter] };
        if (query.Sort.SequenceEqual(_grid.Query.Sort) && query.Order == _grid.Query.Order
            && query.Filters.SequenceEqual(_grid.Query.Filters))
        {
            return;
        }
        _grid.Query = query;
        _grid.FirstVisible = 0;
        _grid.Items.SetQuery(query);
    }

    private void OnRefresh(object sender, RoutedEventArgs e) => _grid?.Items.Refresh();

    private void OnRetry(object sender, RoutedEventArgs e) => _grid?.Items.Retry();
}
