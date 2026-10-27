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
/// A view across every library of <c>Server</c> (owner, 2026-10-26): the Recently added tab, a grid on the library page; the
/// Favourites tab, rows of a media kind each (owner, 2026-10-27); and a kind's page, every movie, series or episode, a grid on the
/// library page that a Favourites row's header opens with the favourites tick on (<c>Favourite</c>). Compares by value.
/// </summary>
public sealed record ViewArgs(ServerRef Server, BrowseView View, string? Kind = null, bool Favourite = false)
{
    /// <summary>The page that shows it, by the core's layout: the Favourites tab's rows on the search page's layout, any other view a
    /// grid.</summary>
    public Type Page => Views.Layout(View, Kind) == ViewLayout.Rows ? typeof(SearchPage) : typeof(LibraryPage);
}

/// <summary>An item's detail page.</summary>
public sealed record DetailsArgs(ItemRef Item);

/// <summary>The results for a search term, from every signed-in server.</summary>
public sealed record SearchArgs(string Term);
