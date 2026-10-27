using System.Collections.ObjectModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// The results of a search from the navigation's search box: every signed-in server's best matches, grouped by kind (ADR 0018). The
/// searches are the core's, kept with the page's history entry (BrowseHub.Search): Back shows the results the page had. The
/// Favourites tab (a <see cref="ViewArgs"/>) shows its rows the same way, a row per kind whose header opens that kind's whole list
/// (owner, 2026-10-27); they are kept with the server's browse, as Home's rows are. A search has Filter (type and source) and View
/// (owner, 2026-10-26 and 2026-10-27; ADR 0018, "Global search filters"): the choices are the core's, kept with the history entry
/// and, while Settings remembers filters or views, for the next search.
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
        _args = e.Parameter;
        _term = term;
        // A new search opens with the filters and view last chosen, each while Settings remembers it (the core's rule); one shown
        // again (Back) keeps its own.
        var remembered = App.Core?.SearchChoices;
        _kept = hub.Search(e.Parameter!, term, remembered?.Filter ?? new SearchFilter(),
            remembered?.View?.Tile == LibraryView.Wide, remembered?.View?.ShowTitles ?? true);
        foreach (var section in _kept.Sections)
        {
            Add(section);
        }
        Tools.Visibility = Visibility.Visible;
        ShowFilterMenu();
        ShowViewChecks();
        ShowStatus();
    }

    /// <summary>The search shown, kept with its history entry (<see cref="_args"/>); null for the Favourites tab.</summary>
    private KeptSearch? _kept;
    private object? _args;
    private string _term = "";

    // Glyphs from Segoe Fluent Icons ("Segoe Fluent Icons font", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-27): E8A9
    // "ViewAll", E8B2 "Movies" and E7F4 "TVMonitor" as the Recently added tab's types (LibraryPage), E716 "People", E8F1 "Library" and
    // E719 "Shop" for the catalog. Every item has one (ADR 0022, choice 1).
    private static string? TypeGlyph(string? kind) => kind switch
    {
        null => "\uE8A9",
        "movie" => "\uE8B2",
        "series" => "\uE7F4",
        "person" => "\uE716",
        _ => null,
    };

    private static string SourceGlyph(SearchSource source) => source switch
    {
        SearchSource.Library => "\uE8F1",
        SearchSource.Catalog => "\uE719",
        _ => "\uE8A9",
    };

    /// <summary>The capabilities can change while the page shows (a server signed in): the menu is the core's answer of now.</summary>
    private void OnFilterMenuOpening(object? sender, object e) => ShowFilterMenu();

    /// <summary>
    /// The Filter menu from the core's choices: the types, a separator, the sources. An unavailable choice (the catalog without a
    /// Seerr server) is disabled: "The control can't receive keyboard focus" and "It can't handle any input events on itself"
    /// (Control.IsEnabled, learn.microsoft.com, updated 2026-07-28, read 2026-10-27). Its name says why, as the tooltip opens on its
    /// owner's pointer and focus events (ToolTipService_Partial.cpp, microsoft-ui-xaml main, d8ab5708, read 2026-10-27), so it may not
    /// show on a disabled item, and how Narrator reads a disabled item (not verified). The tooltip is set anyway, for when it shows.
    /// </summary>
    private void ShowFilterMenu()
    {
        if (_kept is null || App.Core is not { } core)
        {
            return;
        }
        var current = _kept.Filter;
        var tools = core.SearchToolsFor(current);
        FilterMenu.Items.Clear();
        foreach (var type in tools.Types)
        {
            var kind = type.Kind;
            FilterMenu.Items.Add(FilterItem(type.Choice, TypeGlyph(kind), "type", () => Refilter(current with { Kind = kind })));
        }
        FilterMenu.Items.Add(new MenuFlyoutSeparator());
        foreach (var source in tools.Sources)
        {
            var chosen = source.Source;
            FilterMenu.Items.Add(FilterItem(source.Choice, SourceGlyph(chosen), "source",
                () => Refilter(current with { Source = chosen })));
        }
    }

    private static RadioMenuFlyoutItem FilterItem(SearchChoice choice, string? glyph, string group, Action chosen)
    {
        var item = new RadioMenuFlyoutItem
        {
            Text = choice.Label,
            GroupName = group,
            IsChecked = choice.Chosen,
            IsEnabled = choice.Enabled,
            Icon = glyph is null ? null : new FontIcon { Glyph = glyph },
        };
        if (choice.Tooltip is { } tip)
        {
            ToolTipService.SetToolTip(item, tip);
            AutomationProperties.SetHelpText(item, tip);
        }
        item.Click += (_, _) => chosen();
        return item;
    }

    /// <summary>The search again with <paramref name="filter"/> (a new type or source), remembered while Settings remembers filters.</summary>
    private void Refilter(SearchFilter filter)
    {
        if (_kept is null || _args is null || App.Browsing is not { } hub || filter == _kept.Filter)
        {
            return;
        }
        Release();
        _kept = hub.Refilter(_args, _term, filter);
        foreach (var section in _kept.Sections)
        {
            Add(section);
        }
        ShowFilterMenu();
        ShowStatus();
        Remember(new SearchChoices(Filter: filter));
    }

    /// <summary>The View menu's checks for the search's view.</summary>
    private void ShowViewChecks()
    {
        if (_kept is null)
        {
            return;
        }
        PosterItem.IsChecked = !_kept.Wide;
        WideItem.IsChecked = _kept.Wide;
        TitlesItem.IsChecked = _kept.Titles;
    }

    private void OnViewChosen(object sender, RoutedEventArgs e)
    {
        if (_kept is null)
        {
            return;
        }
        _kept.SetView(WideItem.IsChecked, TitlesItem.IsChecked);
        ShowViewChecks();
        Remember(new SearchChoices(View: new LibraryView(_kept.Wide ? LibraryView.Wide : LibraryView.Poster, _kept.Titles)));
    }

    /// <summary>Tells the core what the user chose; it keeps each part only while Settings remembers it. A failed save is only traced.</summary>
    private static async void Remember(SearchChoices choices)
    {
        if (App.Core is not { } core)
        {
            return;
        }
        try
        {
            await core.RememberSearchChoicesAsync(choices);
        }
        catch (Exception error)
        {
            Trace.TraceWarning($"Current: the search page's choices were not remembered: {error.GetType().Name}");
        }
    }

    /// <summary>Narrows the search to one type, as its Filter item does (the smoke run).</summary>
    internal void FilterTo(string? kind)
    {
        if (_kept is not null)
        {
            Refilter(_kept.Filter with { Kind = kind });
        }
    }

    /// <summary>The kinds of the rows shown now, in order (the smoke run).</summary>
    internal IReadOnlyList<string> RowKinds => [.. _sections.SelectMany(section => section.Rows).Select(row => row.Key)];

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

    /// <summary>The core's status line: "Searching…", then nothing once there are results, "No results", or why the search failed.</summary>
    private void ShowStatus()
    {
        var loading = _sections.Any(section => section.State.Loading);
        var found = _sections.Sum(section => section.Rows.Count);
        var failed = _sections.Select(section => section.State.Failed).FirstOrDefault(problem => problem is not null);
        LoadingRing.IsActive = loading && found == 0;
        var text = App.Core?.SearchStatus(found, loading, failed, favourites: _view is not null) ?? "";
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
            // A row scrolls sideways; a search narrowed to one type wraps its tiles, the page scrolling them (the panel is set before
            // the items, which StableSource sets).
            var shown = list.DataContext as TileRow;
            var wraps = shown is { Wraps: true };
            list.ItemsPanel = (ItemsPanelTemplate)Resources[wraps ? "WrapPanel" : "RowPanel"];
            ScrollViewer.SetHorizontalScrollMode(list, wraps ? ScrollMode.Disabled : ScrollMode.Enabled);
            ScrollViewer.SetHorizontalScrollBarVisibility(list, wraps ? ScrollBarVisibility.Disabled : ScrollBarVisibility.Auto);
            // What opening an item does and whether it has the item menu are the core's (a person opens their titles, with no menu).
            var opens = shown?.Opens ?? ItemOpens.Details;
            HomeSection.AttachOnce(list, item =>
            {
                if (opens == ItemOpens.PersonTitles)
                {
                    Frame.Navigate(typeof(LibraryPage), new PersonArgs(item.Item, item.Name));
                }
                else
                {
                    Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item));
                }
            }, shown?.ItemMenu ?? true);
            StableSource.Follow<TileRow>(list, row => (row.Items, row.ItemTemplate));
        }
    }
}
