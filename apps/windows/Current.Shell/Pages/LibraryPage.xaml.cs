using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// A library's grid (ADR 0018, section 3): a GridView over the core's slots, sized to the library's total from the first page,
/// loading the pages it shows (SlotCollection's IItemsRangeInfo). The grid's PagedItems lives in the hub for the whole session, so
/// coming back to the page binds to the slots already loaded: Back and Forward scroll to where the user left, opened from the
/// navigation it starts at the top. A refresh only replaces slots, so the scroll position stays.
/// </summary>
public sealed partial class LibraryPage : Page, IRefreshable
{
    /// <summary>The sorts and filters the core offers, in its order (ItemQuery.SortChoices and FilterChoices); the menus list them.</summary>
    private static readonly IReadOnlyList<SortChoice> Sorts = ItemQuery.SortChoices;

    private static readonly IReadOnlyList<string?> Filters = ItemQuery.FilterChoices;

    /// <summary>A sort's name in the menu; a key Current has no name for shows as the core gave it.</summary>
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

    // The menu items' icons, as Photos' sort menu has them (owner, 2026-10-26), from Segoe Fluent Icons ("Segoe Fluent Icons font",
    // learn.microsoft.com, ms.date 2026-10-13, read 2026-10-26): E8D2 Font, E787 Calendar, E8B2 Movies, E734 FavoriteStar, E74A Up,
    // E74B Down, E71D AllApps, E768 Play, E73E CheckMark and EB51 Heart (the tiles' played mark and the details page's favourite).
    // A key or filter Current has no icon for gets none.
    private static string? SortGlyph(string key) => key switch
    {
        "sort_name" => "\uE8D2",
        "date_created" => "\uE787",
        "premiere_date" => "\uE8B2",
        "community_rating" => "\uE734",
        _ => null,
    };

    private static string? FilterGlyph(string? filter) => filter switch
    {
        null => "\uE71D",
        "unplayed" => "\uE768",
        "played" => "\uE73E",
        "favourite" => "\uEB51",
        _ => null,
    };

    // The arrows fill their whole em while the other glyphs leave space round them, so Ascending and Descending get a margin in the
    // item's 16 x 16 icon box, which scales its content to fit (IconRoot, a Viewbox, in the menu items' template, generic.xaml of
    // Microsoft.WindowsAppSDK.WinUI 2.3.9): 2 on each side draws them at 16/20 of the size, as small as Photos' (owner's run,
    // 2026-10-26).
    private static RadioMenuFlyoutItem MenuItem(string text, string? glyph, string group, bool small = false) => new()
    {
        Text = text,
        GroupName = group,
        Icon = glyph is null ? null : new FontIcon { Glyph = glyph, Margin = small ? new Thickness(2) : default },
    };

    private LibraryGrid? _grid;
    private SlotCollection? _slots;
    private ValueSubscription<PagedState>? _state;

    // The menus' radio items, in the order of Sorts, SortOrder and Filters.
    private readonly List<RadioMenuFlyoutItem> _sortItems = [];
    private readonly List<RadioMenuFlyoutItem> _orderItems = [];
    private readonly List<RadioMenuFlyoutItem> _filterItems = [];

    public LibraryPage()
    {
        InitializeComponent();
        // A new field takes the order the core gives it (names A to Z, the newest first); Ascending or Descending then turns it.
        foreach (var sort in Sorts)
        {
            var item = MenuItem(SortName(sort.Key), SortGlyph(sort.Key), "sort");
            item.Click += (_, _) => Choose(query => query with { Sort = [sort.Key], Order = sort.Order });
            _sortItems.Add(item);
            SortMenu.Items.Add(item);
        }
        SortMenu.Items.Add(new MenuFlyoutSeparator());
        foreach (var (order, name, glyph) in new[] { (SortOrder.Ascending, "Ascending", "\uE74A"), (SortOrder.Descending, "Descending", "\uE74B") })
        {
            var item = MenuItem(name, glyph, "order", small: true);
            item.Click += (_, _) => Choose(query => query with { Order = order });
            _orderItems.Add(item);
            SortMenu.Items.Add(item);
        }
        foreach (var filter in Filters)
        {
            var item = MenuItem(FilterName(filter), FilterGlyph(filter), "filter");
            item.Click += (_, _) => Choose(query => query with { Filters = filter is null ? [] : [filter] });
            _filterItems.Add(item);
            FilterMenu.Items.Add(item);
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
        _library = library.Item;
        _hub = App.Browsing;
        _hub.Changed += OnLibrariesChanged;
        _grid = server.Grid(library);
        _grid.Bind();
        // Back and Forward return to where the grid was left, its sort and filter too; opening the library anew (its navigation
        // item) starts at the top with the first sort and filter (the owner, 2026-10-25). A changed query is a new query, which
        // resets the grid, as choosing a sort does.
        if (e.NavigationMode is not (NavigationMode.Back or NavigationMode.Forward))
        {
            _grid.FirstVisible = 0;
            // What the library opens with: a Home row's own query as it is, else the library's first query with the sort, filter and
            // view last chosen here, each while Settings remembers it (the core's rule).
            var remembered = args.Query is null ? App.Core?.LibraryChoicesFor(library.Item) : null;
            _grid.Wide = remembered?.View?.Tile == LibraryView.Wide;
            _grid.ShowsTitles = remembered?.View?.ShowTitles ?? true;
            var first = args.Query ?? ItemQuery.ForLibrary(library);
            if (remembered?.Sort is { } sort)
            {
                first = first with { Sort = [sort.Key], Order = sort.Order };
            }
            if (remembered?.Filters is { } filters)
            {
                first = first with { Filters = filters };
            }
            if (!SameChoices(_grid.Query, first))
            {
                _grid.Query = first;
                _grid.Items.SetQuery(first);
            }
        }
        ShowQuery(_grid.Query);
        ShowView();
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

    /// <summary>The hub whose libraries the title follows while the page is shown, and the library shown.</summary>
    private BrowseHub? _hub;

    private ItemRef? _library;

    /// <summary>The library was renamed on the server: the title follows the navigation's item.</summary>
    private void OnLibrariesChanged(object? sender, EventArgs e)
    {
        if (_library is { } shown && _hub?.FindLibrary(shown) is { } library && TitleText.Text != library.Name)
        {
            TitleText.Text = library.Name;
        }
    }

    private void Release()
    {
        if (_hub is not null)
        {
            _hub.Changed -= OnLibrariesChanged;
            _hub = null;
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
        // Last: a grid whose library went meanwhile is freed once nothing here uses it.
        _grid?.Unbind();
        _grid = null;
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

    /// <summary>Checks the menus' items for the query's field, order and filter.</summary>
    private void ShowQuery(ItemQuery query)
    {
        var sort = Math.Max(0, Sorts.ToList().FindIndex(sort => query.Sort.FirstOrDefault() == sort.Key));
        var filter = Math.Max(0, Filters.ToList().IndexOf(query.Filters.FirstOrDefault()));
        for (var i = 0; i < _sortItems.Count; i++)
        {
            _sortItems[i].IsChecked = i == sort;
        }
        _orderItems[0].IsChecked = query.Order == SortOrder.Ascending;
        _orderItems[1].IsChecked = query.Order == SortOrder.Descending;
        for (var i = 0; i < _filterItems.Count; i++)
        {
            _filterItems[i].IsChecked = i == filter;
        }
    }

    /// <summary>Whether two queries sort and filter alike (the records compare their lists by reference).</summary>
    private static bool SameChoices(ItemQuery a, ItemQuery b) =>
        a.Sort.SequenceEqual(b.Sort) && a.Order == b.Order && a.Filters.SequenceEqual(b.Filters);

    /// <summary>A new field, order or filter: a new query, which resets the grid to the top.</summary>
    private void Choose(Func<ItemQuery, ItemQuery> change)
    {
        if (_grid is null)
        {
            return;
        }
        var query = change(_grid.Query);
        ShowQuery(query);
        if (SameChoices(query, _grid.Query))
        {
            return;
        }
        _grid.Query = query;
        _grid.FirstVisible = 0;
        _grid.Items.SetQuery(query);
        Remember(new LibraryChoices(
            Sort: query.Sort.FirstOrDefault() is { } key ? new SortChoice(key, query.Order) : null,
            Filters: query.Filters));
    }

    /// <summary>
    /// Tells the core what the user chose here; it keeps each part only while Settings remembers it. A failed save loses nothing the
    /// grid shows, so it is only traced.
    /// </summary>
    private async void Remember(LibraryChoices choices)
    {
        if (_library is not { } library || App.Core is not { } core)
        {
            return;
        }
        try
        {
            await core.RememberLibraryChoicesAsync(library, choices);
        }
        catch (Exception error)
        {
            Trace.TraceWarning($"Current: a library's choices were not remembered: {error.GetType().Name}");
        }
    }

    /// <summary>The tiles' template and the View menu's checks for the grid's view.</summary>
    private void ShowView()
    {
        if (_grid is null)
        {
            return;
        }
        PosterItem.IsChecked = !_grid.Wide;
        WideItem.IsChecked = _grid.Wide;
        TitlesItem.IsChecked = _grid.ShowsTitles;
        var key = (_grid.Wide, _grid.ShowsTitles) switch
        {
            (false, true) => "PosterTileTemplate",
            (true, true) => "WideTileTemplate",
            (false, false) => "PosterPictureTemplate",
            (true, false) => "WidePictureTemplate",
        };
        var template = (DataTemplate)Application.Current.Resources[key];
        if (!ReferenceEquals(Items.ItemTemplate, template))
        {
            Items.ItemTemplate = template;
        }
    }

    private void OnViewChosen(object sender, RoutedEventArgs e)
    {
        if (_grid is null)
        {
            return;
        }
        _grid.Wide = WideItem.IsChecked;
        _grid.ShowsTitles = TitlesItem.IsChecked;
        ShowView();
        Remember(new LibraryChoices(View: new LibraryView(_grid.Wide ? LibraryView.Wide : LibraryView.Poster, _grid.ShowsTitles)));
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

    private void OnRefresh(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs e)
    {
        e.Handled = true;
        Refresh();
    }

    /// <summary>The grid's items again, in place.</summary>
    public void Refresh() => _grid?.Items.Refresh();

    private void OnRetry(object sender, RoutedEventArgs e) => _grid?.Items.Retry();
}
