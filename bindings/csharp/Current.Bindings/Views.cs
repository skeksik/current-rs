using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// The views across every library, what a grid opens with, its Filter menu, how Home's and the search page's rows show and link, and
// the browse screens' names and times: the core's rules and words. The app lays them out with its own icons. Kinds, sort keys and
// filters are open values, passed through unchanged.

/// <summary>A view across every library of a server: the Favourites tab, the Recently added tab, or every item of a kind.</summary>
public enum BrowseView
{
    Favourites,
    RecentlyAdded,
    Everything,
}

/// <summary>How a view is laid out: titled rows as search results (the Favourites tab), or a grid as a library's.</summary>
public enum ViewLayout
{
    Rows,
    Grid,
}

/// <summary>What a grid opens with when it is opened anew (not by Back or Forward): its query, and its view.</summary>
public sealed record GridOpening(ItemQuery Query, bool Wide, bool ShowTitles);

/// <summary>
/// What a grid's Filter menu lists besides its filters: the media types at its top, every type (null) first, empty for none; and
/// whether the played choice and the favourites tick show.
/// </summary>
public sealed record FilterMenu(IReadOnlyList<string?> Kinds, bool Filters);

/// <summary>Where a row's header goes.</summary>
public abstract record RowTarget
{
    private RowTarget() { }

    /// <summary>The library's grid, the newest added first (<see cref="ItemQuery.RecentlyAdded"/>).</summary>
    public sealed record OpenLibrary(ItemRef Library) : RowTarget;

    /// <summary>A view across every library: <see cref="Views.Query"/> with these.</summary>
    public sealed record OpenView(BrowseView View, string? Kind, bool Favourite) : RowTarget;
}

/// <summary>A row header that is a link: where it goes and its tooltip.</summary>
public sealed record RowLink(RowTarget Target, string Tooltip);

/// <summary>How a row of tiles shows: its title, whether its tiles are wide (16:9) rather than posters (2:3), and its header's link
/// (null: plain text).</summary>
public sealed record RowDisplay(string Title, bool Wide, RowLink? Link);

/// <summary>The views across every library and how rows show (the core decides).</summary>
public static class Views
{
    /// <summary>The query of <paramref name="view"/>, narrowed to <paramref name="kind"/> when given; with
    /// <paramref name="favourite"/>, a kind's page with its favourites tick on, as a Favourites row's header opens it.</summary>
    public static ItemQuery Query(BrowseView view, string? kind = null, bool favourite = false) =>
        MapBrowse.From(Ffi.CurrentFfiMethods.ViewQuery(MapViews.ToFfi(view), kind, favourite));

    /// <summary>How <paramref name="view"/> narrowed to <paramref name="kind"/> is laid out.</summary>
    public static ViewLayout Layout(BrowseView view, string? kind = null) =>
        Ffi.CurrentFfiMethods.ViewLayout(MapViews.ToFfi(view), kind) == Ffi.ViewLayout.Rows ? ViewLayout.Rows : ViewLayout.Grid;

    /// <summary>What a view's grid opens with when opened anew: its query, posters with titles; nothing is remembered for a view.</summary>
    public static GridOpening Opening(BrowseView view, string? kind = null, bool favourite = false) =>
        MapViews.From(Ffi.CurrentFfiMethods.ViewOpening(MapViews.ToFfi(view), kind, favourite));

    /// <summary>The Filter menu of a grid showing <paramref name="view"/> (null: a library) narrowed to <paramref name="kind"/>.</summary>
    public static FilterMenu FilterMenu(BrowseView? view, string? kind = null)
    {
        var menu = Ffi.CurrentFfiMethods.GridFilterMenu(view is { } shown ? (Ffi.BrowseView?)MapViews.ToFfi(shown) : null, kind);
        return new FilterMenu(menu.Kinds, menu.Filters);
    }

    /// <summary>The view's title, as its tab or page names it: "Favourites", "Recently added", or a kind's page by its kind.</summary>
    public static string Title(BrowseView view, string? kind = null) => Ffi.CurrentFfiMethods.ViewTitle(MapViews.ToFfi(view), kind);

    /// <summary>The tooltip of a row header that opens <paramref name="view"/> narrowed to <paramref name="kind"/>.</summary>
    public static string LinkTooltip(BrowseView view, string? kind = null) =>
        Ffi.CurrentFfiMethods.ViewLinkTooltip(MapViews.ToFfi(view), kind);

    /// <summary>How a Home row shows: its title ("Recently added in" a library), its tiles' shape and its header's link.</summary>
    public static RowDisplay ForHomeRow(HomeRow row)
    {
        ArgumentNullException.ThrowIfNull(row);
        return MapViews.From(Ffi.CurrentFfiMethods.HomeRowDisplay(MapViews.ToFfi(row)));
    }

    /// <summary>How a row of <paramref name="kind"/> on the search page shows: a search group, or with <paramref name="link"/> a
    /// Favourites tab's row, whose header opens that view of its kind with the favourites tick on.</summary>
    public static RowDisplay ForSearchGroup(string kind, BrowseView? link = null)
    {
        ArgumentNullException.ThrowIfNull(kind);
        return MapViews.From(Ffi.CurrentFfiMethods.SearchGroupDisplay(kind, link is { } view ? (Ffi.BrowseView?)MapViews.ToFfi(view) : null));
    }
}

/// <summary>What the browse screens call things (the core's words).</summary>
public static class BrowseLabels
{
    /// <summary>A media kind's group name ("Movies"); one Current has no name for shows as the server named it, title-cased.</summary>
    public static string KindGroup(string kind)
    {
        ArgumentNullException.ThrowIfNull(kind);
        return Ffi.CurrentFfiMethods.KindGroupName(kind);
    }

    /// <summary>A media type in a view's Filter menu: "All types" for every type (null), else its name.</summary>
    public static string KindChoice(string? kind) => Ffi.CurrentFfiMethods.KindChoiceName(kind);

    /// <summary>A sort key's name in a library grid's Sort menu.</summary>
    public static string SortName(string key)
    {
        ArgumentNullException.ThrowIfNull(key);
        return Ffi.CurrentFfiMethods.LibrarySortName(key);
    }

    /// <summary>A filter's name in a library grid's Filter menu: "All" for no filter (null).</summary>
    public static string FilterName(string? filter) => Ffi.CurrentFfiMethods.LibraryFilterName(filter);

    /// <summary>An item's runtime: "2 h 12 min", "45 min".</summary>
    public static string Runtime(TimeSpan runtime) => Ffi.CurrentFfiMethods.RuntimeLabel(MapViews.Millis(runtime));

    /// <summary>The time left on a tile, short: "1h 12m", "45m"; under a minute counts as one.</summary>
    public static string TimeLeftShort(TimeSpan left) => Ffi.CurrentFfiMethods.TimeLeftShort(MapViews.Millis(left));

    /// <summary>The time left in words, as Narrator reads a tile: "1 h 12 min left"; under a minute counts as one.</summary>
    public static string TimeLeft(TimeSpan left) => Ffi.CurrentFfiMethods.TimeLeftLabel(MapViews.Millis(left));

    /// <summary>
    /// The Favourites tab's line above its <paramref name="rows"/> rows: null while it shows rows or is loading; else "Couldn't load
    /// favourites." when it failed (add why after it), else "No favourites yet".
    /// </summary>
    public static string? FavouritesStatus(int rows, bool loading, bool failed) =>
        Ffi.CurrentFfiMethods.FavouritesStatus((uint)Math.Max(rows, 0), loading, failed);
}

public sealed partial class CoreHost
{
    /// <summary>
    /// What <paramref name="library"/>'s grid opens with when opened anew: <paramref name="homeQuery"/> as it is when a Home row's
    /// header opened it, posters with titles, nothing remembered; else the library's first query with its remembered sort, filters
    /// (only if the Filter menu still offers every one) and view, each while Settings remembers it.
    /// </summary>
    public GridOpening LibraryOpening(Library library, ItemQuery? homeQuery = null)
    {
        ArgumentNullException.ThrowIfNull(library);
        return MapViews.From(_core.LibraryOpening(MapBrowse.ToFfi(library), homeQuery is { } query ? MapBrowse.ToFfi(query) : null));
    }
}

internal static class MapViews
{
    public static Ffi.BrowseView ToFfi(BrowseView view) => view switch
    {
        BrowseView.Favourites => Ffi.BrowseView.Favourites,
        BrowseView.RecentlyAdded => Ffi.BrowseView.RecentlyAdded,
        BrowseView.Everything => Ffi.BrowseView.Everything,
        _ => throw new ArgumentOutOfRangeException(nameof(view), view, "unknown view"),
    };

    public static BrowseView From(Ffi.BrowseView view) => view switch
    {
        Ffi.BrowseView.Favourites => BrowseView.Favourites,
        Ffi.BrowseView.RecentlyAdded => BrowseView.RecentlyAdded,
        _ => BrowseView.Everything,
    };

    public static GridOpening From(Ffi.GridOpening opening) => new(MapBrowse.From(opening.Query), opening.Wide, opening.ShowTitles);

    public static RowDisplay From(Ffi.RowDisplay display) => new(
        display.Title,
        display.Shape == Ffi.TileShape.Wide,
        display.Link is { } link
            ? new RowLink(
                link.Target switch
                {
                    Ffi.RowTarget.OpenLibrary library => new RowTarget.OpenLibrary(MapBrowse.From(library.Library)),
                    Ffi.RowTarget.OpenView view => new RowTarget.OpenView(From(view.View), view.Kind, view.Favourite),
                    _ => throw new ArgumentException($"unknown row target {link.Target.GetType().Name}", nameof(display)),
                },
                link.Tooltip)
            : null);

    public static Ffi.HomeRow ToFfi(HomeRow row) => new(
        row.Key,
        row.Kind switch
        {
            HomeRowKind.ContinueWatching => Ffi.HomeRowKind.ContinueWatching,
            HomeRowKind.NextUp => Ffi.HomeRowKind.NextUp,
            HomeRowKind.RecentlyAdded => Ffi.HomeRowKind.RecentlyAdded,
            HomeRowKind.Favourites => Ffi.HomeRowKind.Favourites,
            HomeRowKind.SeenRecently => Ffi.HomeRowKind.SeenRecently,
            _ => Ffi.HomeRowKind.Latest,
        },
        row.Title,
        row.Library is { } library ? MapBrowse.ToFfi(library) : null);

    public static ulong Millis(TimeSpan time) => (ulong)Math.Max(0, time.TotalMilliseconds);
}
