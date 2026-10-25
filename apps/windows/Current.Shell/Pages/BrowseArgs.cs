using Current.Bindings;

namespace Current.Shell.Pages;

// What the browse pages are navigated with. Kept in memory only: the app never saves its navigation state, so these need not be
// serialisable.

/// <summary>A library's grid.</summary>
public sealed record LibraryArgs(ItemRef Library);

/// <summary>An item's detail page.</summary>
public sealed record DetailsArgs(ItemRef Item);

/// <summary>The results for a search term, from every signed-in server.</summary>
public sealed record SearchArgs(string Term);
