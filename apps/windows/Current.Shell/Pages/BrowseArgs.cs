using Current.Bindings;

namespace Current.Shell.Pages;

// What the browse pages are navigated with. Kept in memory only: the app never saves its navigation state, so these need not be
// serialisable.

/// <summary>
/// A library's grid. The library itself, not only its id, so a grid can be opened before the navigation lists it (the smoke run);
/// the page shows the hub's newer copy when there is one. Compare by <see cref="Shows"/>: the record's artwork list compares by
/// reference. <c>Query</c>, when given, is what the grid opens with in place of the library's first query (a Home row's header).
/// </summary>
public sealed record LibraryArgs(Library Library, ItemQuery? Query = null)
{
    public bool Shows(object? other) => other is LibraryArgs args && args.Library.Item == Library.Item;
}

/// <summary>
/// A view across every library of <c>Server</c>, as a grid on the library page: the Favorites or the Recently added tab (owner,
/// 2026-10-26). Compares by value.
/// </summary>
public sealed record ViewArgs(ServerRef Server, string View)
{
    public const string Favourites = "favourites";
    public const string RecentlyAdded = "recently_added";

    /// <summary>The view's query from the core, narrowed to one media kind when given.</summary>
    public static ItemQuery Query(string view, string? kind) =>
        view == Favourites ? ItemQuery.Favourites(kind) : ItemQuery.RecentlyAddedEverywhere(kind);
}

/// <summary>An item's detail page.</summary>
public sealed record DetailsArgs(ItemRef Item);

/// <summary>The results for a search term, from every signed-in server.</summary>
public sealed record SearchArgs(string Term);
