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
/// coming back to the page binds to the slots already loaded: Back and Forward scroll to where the user left, opened from the
/// navigation it starts at the top. A refresh only replaces slots, so the scroll position stays.
/// </summary>
public sealed partial class LibraryPage : Page
{
    /// <summary>The sorts and filters the core offers, in its order (ItemQuery.SortChoices and FilterChoices); the boxes list them.</summary>
    private static readonly IReadOnlyList<SortChoice> Sorts = ItemQuery.SortChoices;

    private static readonly IReadOnlyList<string?> Filters = ItemQuery.FilterChoices;

    /// <summary>A sort's name in the box; a key Current has no name for shows as the core gave it.</summary>
    private static string SortName(string key) => key switch
    {
        "sort_name" => "Name",
        "date_created" => "Date added",
        "premiere_date" => "Release date",
        "community_rating" => "Rating",
        _ => key,
    };

    private static string FilterName(string? filter) => filter switch
    {
        null => "All",
        "unplayed" => "Unplayed",
        "played" => "Played",
        "favourite" => "Favourites",
        _ => filter,
    };

    private LibraryGrid? _grid;
    private SlotCollection? _slots;
    private ValueSubscription<PagedState>? _state;
    private bool _showingQuery;

    public LibraryPage()
    {
        InitializeComponent();
        foreach (var sort in Sorts)
        {
            SortBox.Items.Add(SortName(sort.Key));
        }
        foreach (var filter in Filters)
        {
            FilterBox.Items.Add(FilterName(filter));
        }
        ItemActions.Attach(Items, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
        Loaded += (_, _) => _wasLoaded = true;
        Unloaded += (_, _) => Release();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is not LibraryArgs args || App.Browsing?.Find(args.Library.Item.Server) is not { } server)
        {
            // The server was signed out of while this page was in the history.
            EmptyText.Visibility = Visibility.Visible;
            return;
        }
        var library = App.Browsing.FindLibrary(args.Library.Item) ?? args.Library;
        TitleText.Text = library.Name;
        _grid = server.Grid(library);
        // Back and Forward return to where the grid was left; opening the library anew (its navigation item) starts at the top
        // (the owner, 2026-10-25).
        if (e.NavigationMode is not (NavigationMode.Back or NavigationMode.Forward))
        {
            _grid.FirstVisible = 0;
        }
        ShowQuery(_grid.Query);
        var ui = new DispatcherQueueUi(DispatcherQueue);
        _state = _grid.Items.ObserveState(ui);
        _state.PropertyChanged += OnStateChanged;
        _slots = SlotCollection.Open(_grid.Items, ui);
        _slots.CollectionChanged += OnFirstSlots;
        Items.ItemsSource = _slots;
        ShowState();
        // The core decides whether the grid is stale: shown again five minutes or more after it was left (ADR 0018, section 3).
        _grid.Items.Shown();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (!_wasLoaded)
        {
            // Left before it ever loaded (navigated past at once): no Unloaded will come.
            DispatcherQueue.TryEnqueue(Release);
        }
        if (_grid is not null)
        {
            _grid.FirstVisible = (Items.ItemsPanelRoot as ItemsWrapGrid)?.FirstVisibleIndex ?? 0;
            _grid.Items.Hidden();
        }
    }

    /// <summary>
    /// Lets go of the slots once the page has left the window, not when it is navigated from: then its exit transition still
    /// shows it, and an emptied grid showed its top for a moment (the owner's run, 2026-10-25).
    /// </summary>
    /// <summary>The page reached the window: its lists go on Unloaded. One navigated from before it loaded has no Unloaded.</summary>
    private bool _wasLoaded;

    private void Release()
    {
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
            // Queued: this handler runs before the grid's own, which has not taken the items in yet, and a scroll asked for now did
            // nothing (owner's run, 2026-10-25: Back did not return to the place).
            DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
            {
                if (ReferenceEquals(_slots, slots) && index < slots.Count)
                {
                    Items.ScrollIntoView(slots[index], ScrollIntoViewAlignment.Leading);
                }
            });
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
        CountText.Text = state.Total > 0
            ? (state.Total == 1 ? "1 item" : $"{state.Total.ToString("N0", System.Globalization.CultureInfo.CurrentCulture)} items")
            : "";
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
        SortBox.SelectedIndex = Math.Max(0, Sorts.ToList().FindIndex(sort => query.Sort.FirstOrDefault() == sort.Key));
        FilterBox.SelectedIndex = Math.Max(0, Filters.ToList().IndexOf(query.Filters.FirstOrDefault()));
        _showingQuery = false;
    }

    /// <summary>A new sort or filter: a new query, which resets the grid to the top.</summary>
    private void OnQueryChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_showingQuery || _grid is null || SortBox.SelectedIndex < 0 || FilterBox.SelectedIndex < 0)
        {
            return;
        }
        var sort = Sorts[SortBox.SelectedIndex];
        var filter = Filters[FilterBox.SelectedIndex];
        var query = _grid.Query with { Sort = [sort.Key], Order = sort.Order, Filters = filter is null ? [] : [filter] };
        if (query.Sort.SequenceEqual(_grid.Query.Sort) && query.Order == _grid.Query.Order
            && query.Filters.SequenceEqual(_grid.Query.Filters))
        {
            return;
        }
        _grid.Query = query;
        _grid.FirstVisible = 0;
        _grid.Items.SetQuery(query);
    }

    /// <summary>The grid's slots now (the smoke run).</summary>
    internal SlotCollection? Slots => _slots;

    /// <summary>Scrolls the grid to a slot, as the scroll bar would (the smoke run).</summary>
    internal void ScrollTo(int index)
    {
        if (_slots is { } slots && index < slots.Count)
        {
            Items.ScrollIntoView(slots[index], ScrollIntoViewAlignment.Leading);
        }
    }

    private void OnRefresh(object sender, RoutedEventArgs e) => _grid?.Items.Refresh();

    private void OnRetry(object sender, RoutedEventArgs e) => _grid?.Items.Retry();
}
