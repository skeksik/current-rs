using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
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

    /// <summary>The filters ticked on or off beside that choice (ItemQuery.ToggleFilterChoices): favourites.</summary>
    private static readonly IReadOnlyList<string> Toggles = ItemQuery.ToggleFilterChoices;

    /// <summary>A sort's and a filter's name in the menus, as the core names them.</summary>
    private static string SortName(string key) => BrowseLabels.SortName(key);

    private static string FilterName(string? filter) => BrowseLabels.FilterName(filter);

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

    /// <summary>The view across every library shown (<see cref="ViewArgs"/>), or null for a library.</summary>
    private BrowseView? _view;


    /// <summary>
    /// The Filter menu's media types, for a view across every library (owner, 2026-10-26): the core's (Views.FilterMenu), every type
    /// first, before the menu's filters and a separator. Glyphs: Segoe Fluent Icons E8A9 "ViewAll", E8B2 "Movies" and E7F4
    /// "TVMonitor", the films and TV libraries' ("Segoe Fluent Icons font", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-26).
    /// </summary>
    private readonly List<MenuFlyoutItemBase> _kindEntries = [];
    private readonly List<(RadioMenuFlyoutItem Item, string? Kind)> _kindItems = [];

    private static string KindName(string? kind) => BrowseLabels.KindChoice(kind);

    private static string? KindGlyph(string? kind) => kind switch
    {
        null => "\uE8A9",
        "movie" => "\uE8B2",
        "series" => "\uE7F4",
        _ => null,
    };

    /// <summary>The Filter menu as the core lays it out for <paramref name="view"/> (null: a library) narrowed to
    /// <paramref name="kind"/>.</summary>
    private void ShowFilterMenu(BrowseView? view, string? kind)
    {
        var menu = Views.FilterMenu(view, kind);
        ShowKindItems(menu.Kinds);
        ShowFilterItems(menu.Filters);
    }

    /// <summary>Puts the media types at the top of the Filter menu, or takes them out (none given).</summary>
    private void ShowKindItems(IReadOnlyList<string?> kinds)
    {
        foreach (var entry in _kindEntries)
        {
            FilterMenu.Items.Remove(entry);
        }
        if (kinds.Count == 0)
        {
            return;
        }
        if (_kindEntries.Count == 0)
        {
            foreach (var kind in kinds)
            {
                var item = MenuItem(KindName(kind), KindGlyph(kind), "kind");
                var chosen = kind;
                item.Click += (_, _) => ChooseKind(chosen);
                _kindItems.Add((item, kind));
                _kindEntries.Add(item);
            }
            _kindEntries.Add(new MenuFlyoutSeparator());
        }
        for (var i = 0; i < _kindEntries.Count; i++)
        {
            FilterMenu.Items.Insert(i, _kindEntries[i]);
        }
    }

    /// <summary>Shows the Filter menu's played choice and favourites tick, or hides them with the separator before them.</summary>
    private void ShowFilterItems(bool show)
    {
        var visibility = show ? Visibility.Visible : Visibility.Collapsed;
        foreach (var item in _filterItems.Concat<MenuFlyoutItemBase>(_toggleItems.Select(toggle => toggle.Item)).Append(_toggleSeparator))
        {
            item.Visibility = visibility;
        }
        if (_kindEntries.LastOrDefault() is MenuFlyoutSeparator separator)
        {
            separator.Visibility = visibility;
        }
    }

    /// <summary>The view narrowed to one media kind (or every kind), keeping its sort, filters and search.</summary>
    private void ChooseKind(string? kind)
    {
        if (_view is not { } view)
        {
            return;
        }
        var narrowed = Views.Query(view, kind);
        Choose(query => query with { Kinds = narrowed.Kinds });
    }

    // The menus' radio items, in the order of Sorts, SortOrder and Filters.
    private readonly List<RadioMenuFlyoutItem> _sortItems = [];
    private readonly List<RadioMenuFlyoutItem> _orderItems = [];
    private readonly List<RadioMenuFlyoutItem> _filterItems = [];

    /// <summary>The Filter menu's ticks after the played choice (owner, 2026-10-27), and the separator between them.</summary>
    private readonly List<(ToggleMenuFlyoutItem Item, string Filter)> _toggleItems = [];
    private readonly MenuFlyoutSeparator _toggleSeparator = new();

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
            item.Click += (_, _) => Choose(query => query.WithFilter(filter));
            _filterItems.Add(item);
            FilterMenu.Items.Add(item);
        }
        // Favourites as a tick of their own, on or off whatever the choice above: a ToggleMenuFlyoutItem, "Switching an option on or
        // off" ("Menu flyout and menu bar", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-27).
        FilterMenu.Items.Add(_toggleSeparator);
        foreach (var toggle in Toggles)
        {
            var item = new ToggleMenuFlyoutItem
            {
                Text = FilterName(toggle),
                Icon = FilterGlyph(toggle) is { } glyph ? new FontIcon { Glyph = glyph } : null,
            };
            item.Click += (_, _) => Choose(query => query.WithToggle(toggle, item.IsChecked));
            _toggleItems.Add((item, toggle));
            FilterMenu.Items.Add(item);
        }
        ItemActions.Attach(Items, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
        Loaded += (_, _) => _wasLoaded = true;
        Unloaded += (_, _) => Release();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is ViewArgs view)
        {
            ShowView(view, e.NavigationMode);
            return;
        }
        if (e.Parameter is PersonArgs person)
        {
            ShowPerson(person, e.NavigationMode);
            return;
        }
        if (e.Parameter is not LibraryArgs args || App.Browsing?.Find(args.Library.Item.Server) is not { } server)
        {
            // The server was signed out of while this page was in the history.
            EmptyText.Visibility = Visibility.Visible;
            return;
        }
        var library = App.Browsing.FindLibrary(args.Library.Item) ?? args.Library;
        _view = null;
        ShowFilterMenu(null, null);
        TitleText.Text = library.Name;
        ShowSearchName(library.Name);
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
            // What the library opens with, the core's rule: a Home row's own query as it is, else the library's first query with the
            // sort, filter and view last chosen here, each while Settings remembers it, filters the menu no longer offers dropped.
            var opening = App.Core?.LibraryOpening(library, args.Query)
                ?? new GridOpening(args.Query ?? ItemQuery.ForLibrary(library), Wide: false, ShowTitles: true);
            _grid.Wide = opening.Wide;
            _grid.ShowsTitles = opening.ShowTitles;
            var first = opening.Query;
            // Opened anew: a search left in this grid is over, so a later one starts from the query shown now (review 2, S1).
            _grid.Browsing = null;
            if (!SameChoices(_grid.Query, first))
            {
                _grid.Query = first;
                _grid.Items.SetQuery(first);
            }
        }
        Attach();
    }

    /// <summary>Shows the grid's query and view and binds its slots and state; the grid is stale-checked as it is shown again.</summary>
    private void Attach()
    {
        if (_grid is null)
        {
            return;
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

    /// <summary>
    /// A view across every library (the Favorites or Recently added tab) in the same grid: its own query from the core, nothing
    /// remembered, and Back and Forward returning to where it was left, as a library's.
    /// </summary>
    private void ShowView(ViewArgs view, NavigationMode mode)
    {
        if (App.Browsing?.Find(view.Server) is not { } server)
        {
            EmptyText.Visibility = Visibility.Visible;
            return;
        }
        var title = Views.Title(view.View, view.Kind);
        TitleText.Text = title;
        _view = view.View;
        // A Favourites row's header opens its kind's page with the favourites tick on (owner, 2026-10-27); nothing is remembered for
        // a view (the core's opening).
        var opening = Views.Opening(view.View, view.Kind, view.Favourite);
        var first = opening.Query;
        // Recently added narrows by media type in its Filter menu; a kind's page is one type already (the core's menu).
        ShowFilterMenu(view.View, view.Kind);
        ShowSearchName(title);
        _grid = server.View(view.View, view.Kind);
        _grid.Bind();
        if (mode is not (NavigationMode.Back or NavigationMode.Forward))
        {
            _grid.FirstVisible = 0;
            _grid.Wide = opening.Wide;
            _grid.ShowsTitles = opening.ShowTitles;
            _grid.Browsing = null;
            if (!SameChoices(_grid.Query, first))
            {
                _grid.Query = first;
                _grid.Items.SetQuery(first);
            }
        }
        Attach();
    }

    /// <summary>
    /// A person's titles (owner, 2026-10-27) in the same grid: their films and series, the newest first (the core's query), titled with
    /// their name, nothing remembered, and Back and Forward returning to where it was left, as a view's.
    /// </summary>
    private void ShowPerson(PersonArgs person, NavigationMode mode)
    {
        if (App.Browsing?.Find(person.Person.Server) is not { } server)
        {
            EmptyText.Visibility = Visibility.Visible;
            return;
        }
        TitleText.Text = person.Name;
        _view = null;
        // One person's titles: the Filter menu a library has (the core's menu).
        ShowFilterMenu(null, null);
        ShowSearchName(person.Name);
        _grid = server.Person(person.Person);
        _grid.Bind();
        if (mode is not (NavigationMode.Back or NavigationMode.Forward))
        {
            var first = ItemQuery.ForPerson(person.Person);
            _grid.FirstVisible = 0;
            _grid.Wide = false;
            _grid.ShowsTitles = true;
            _grid.Browsing = null;
            if (!SameChoices(_grid.Query, first))
            {
                _grid.Query = first;
                _grid.Items.SetQuery(first);
            }
        }
        Attach();
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
            ShowSearchName(library.Name);
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

    /// <summary>A search was just started: the state shown is still the previous query's until the grid's next state arrives.</summary>
    private bool _stateBeforeSearch;

    private void OnStateChanged(object? sender, PropertyChangedEventArgs e)
    {
        _stateBeforeSearch = false;
        ShowState();
    }

    private void ShowState()
    {
        if (_state?.Value is not { } state)
        {
            return;
        }
        LoadingRing.IsActive = state.Loading && state.Total == 0;
        // A search that fills the server's window says so instead of a count that would only be the window's size (the core decides).
        // Only from the searched query's own state: the previous query's total (a big library's) would flash the note (review 2).
        var note = _stateBeforeSearch ? null : _grid?.Query.SearchNote(state.Total);
        CountText.Text = note ?? App.Core?.ItemCount(state.Total) ?? "";
        EmptyText.Visibility = !state.Loading && state.Total == 0 && state.Failed is null ? Visibility.Visible : Visibility.Collapsed;
        if (state.Failed is { } failed)
        {
            var title = state.Total == 0 ? "Couldn't load this library." : "Some items couldn't be loaded.";
            var message = App.Core?.ProblemText(failed) ?? ProblemTexts.Unexpected;
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

    /// <summary>The search box names the library it searches.</summary>
    private void ShowSearchName(string name)
    {
        LibrarySearch.PlaceholderText = $"Search {name}";
        AutomationProperties.SetName(LibrarySearch, $"Search {name}");
    }

    /// <summary>Checks the menus' items for the query's field, order and filter, and shows its search.</summary>
    private void ShowQuery(ItemQuery query)
    {
        if (LibrarySearch.Text != (query.SearchTerm ?? "") && FocusManager.GetFocusedElement(XamlRoot) is not TextBox)
        {
            LibrarySearch.Text = query.SearchTerm ?? "";
        }
        var sort = Math.Max(0, Sorts.ToList().FindIndex(sort => query.Sort.FirstOrDefault() == sort.Key));
        var filter = Math.Max(0, Filters.ToList().IndexOf(query.Filters.FirstOrDefault(Filters.Contains)));
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
        foreach (var (item, toggle) in _toggleItems)
        {
            item.IsChecked = query.Filters.Contains(toggle);
        }
        // A view narrowed to one kind lists that kind only; otherwise it is every type.
        var single = query.Kinds.Count == 1 ? query.Kinds[0] : null;
        foreach (var (item, kind) in _kindItems)
        {
            item.IsChecked = kind == single;
        }
        // While it searches the results come by relevance, unfiltered: Sort and Filter are off, and say why (the core decides).
        var tools = query.Tools();
        ShowTool(SortButton, SortTip, tools.SortAvailable, tools.SortTooltip ?? "Sort");
        ShowTool(FilterButton, FilterTip, tools.FilterAvailable, tools.FilterTooltip ?? "Filter");
    }

    /// <summary>
    /// A command button on or off, its tooltip on the area around it: a disabled control gets no pointer input, so its own tooltip would
    /// not show ("It can't handle any input events on itself", Control.IsEnabled, learn.microsoft.com, updated 2026-07-28, read
    /// 2026-10-27; seen in the app, 2026-10-27). Narrator reads the reason as the button's help text.
    /// </summary>
    private static void ShowTool(Control button, FrameworkElement area, bool available, string tip)
    {
        button.IsEnabled = available;
        ToolTipService.SetToolTip(area, tip);
        AutomationProperties.SetHelpText(button, available ? "" : tip);
    }

    /// <summary>Whether two queries sort and filter alike (the records compare their lists by reference).</summary>
    private static bool SameChoices(ItemQuery a, ItemQuery b) =>
        a.Kinds.SequenceEqual(b.Kinds) && a.Sort.SequenceEqual(b.Sort) && a.Order == b.Order && a.Filters.SequenceEqual(b.Filters)
        && a.SearchTerm == b.SearchTerm;

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
        _grid.Browsing = null;
        _grid.FirstVisible = 0;
        _grid.Items.SetQuery(query);
        Remember(new LibraryChoices(
            Sort: query.Sort.FirstOrDefault() is { } key ? new SortChoice(key, query.Order) : null,
            Filters: query.Filters));
    }

    /// <summary>Waits for a pause in typing before searching, so each key does not start a query.</summary>
    private DispatcherQueueTimer? _searchTimer;

    /// <summary>How long after the last key the box searches; not from a guideline (not verified).</summary>
    private static readonly TimeSpan SearchPause = TimeSpan.FromMilliseconds(400);

    private void OnSearchTextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
    {
        if (args.Reason != AutoSuggestionBoxTextChangeReason.UserInput)
        {
            return;
        }
        if (_searchTimer is null)
        {
            _searchTimer = DispatcherQueue.CreateTimer();
            _searchTimer.IsRepeating = false;
            _searchTimer.Tick += (_, _) => Search(LibrarySearch.Text);
        }
        _searchTimer.Interval = SearchPause;
        _searchTimer.Start();
    }

    private void OnSearchSubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        _searchTimer?.Stop();
        Search(args.QueryText);
    }

    /// <summary>
    /// The grid searching for <paramref name="text"/> from the query it showed before searching (the core's rule: trimmed, by relevance,
    /// no filter; that query back when blank), from the top.
    /// </summary>
    private void Search(string text)
    {
        if (_grid is null)
        {
            return;
        }
        var browsing = _grid.Browsing ?? _grid.Query;
        var query = browsing.Searching(text);
        _grid.Browsing = query.SearchTerm is null ? null : browsing;
        if (SameChoices(query, _grid.Query))
        {
            return;
        }
        _grid.Query = query;
        _grid.FirstVisible = 0;
        _grid.Items.SetQuery(query);
        _stateBeforeSearch = true;
        ShowQuery(query);
        ShowState();
    }

    private void OnFindAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs e) =>
        e.Handled = LibrarySearch.Focus(FocusState.Keyboard);

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

    /// <summary>Types <paramref name="text"/> into the library's search box and searches at once, as Enter does (the smoke run).</summary>
    internal void SearchFor(string text)
    {
        LibrarySearch.Text = text;
        Search(text);
    }

    /// <summary>Whether the Sort and Filter buttons are on now (the smoke run).</summary>
    internal bool SortAndFilterAvailable => SortButton.IsEnabled && FilterButton.IsEnabled;

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
