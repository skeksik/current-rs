using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// The global search's filters (ADR 0018, "Global search filters"). The rules are the core's: which choices there are, their names, which
// is checked and which are unavailable and why. Media kinds are open values (movie, series, person), passed through unchanged.

/// <summary>Where a search looks.</summary>
public enum SearchSource
{
    /// <summary>Every source there is: the library, and the catalog once a Seerr server is set up.</summary>
    All,
    /// <summary>The signed-in Jellyfin servers' libraries.</summary>
    Library,
    /// <summary>What can be requested (Seerr).</summary>
    Catalog,
}

/// <summary>The search's filters. The default finds everything.</summary>
/// <param name="Kind">One type only (movie, series, person), or null for every type.</param>
public sealed record SearchFilter(string? Kind = null, SearchSource Source = SearchSource.All)
{
    /// <summary>
    /// The line under a search group of <paramref name="count"/> matches (the core decides): once it holds as many as the search asks
    /// for, that only the best matches show and more text narrows them; null otherwise.
    /// </summary>
    public string? Note(int count) => Ffi.CurrentFfiMethods.SearchNote(SearchMap.ToFfi(this), (uint)Math.Max(count, 0));
}

/// <summary>
/// One choice of a filter: its name, whether it is the one in effect, and whether it can be chosen; <c>Tooltip</c> says why not. An
/// unavailable choice names the reason in <c>Label</c> too: a disabled menu item shows no tooltip.
/// </summary>
public sealed record SearchChoice(string Label, bool Checked, bool Enabled, string? Tooltip);

/// <summary>One Type choice: <c>Kind</c> is null for "All".</summary>
public sealed record SearchTypeChoice(string? Kind, SearchChoice Choice);

/// <summary>One Source choice.</summary>
public sealed record SearchSourceChoice(SearchSource Source, SearchChoice Choice);

/// <summary>What the search page's Filter offers, in order: the Type choices, then the Source choices.</summary>
public sealed record SearchTools(IReadOnlyList<SearchTypeChoice> Types, IReadOnlyList<SearchSourceChoice> Sources);

/// <summary>
/// The search page's remembered choices, each null when there is none. From <see cref="CoreHost.SearchChoices"/>: nothing remembered,
/// or its switch is off, and the page opens with everything and its first view. To <see cref="CoreHost.RememberSearchChoicesAsync"/>:
/// left as it was.
/// </summary>
public sealed record SearchChoices(SearchFilter? Filter = null, LibraryView? View = null);

public sealed partial class CoreHost
{
    /// <summary>
    /// The search page's Filter choices for <paramref name="filter"/>, with the one in effect checked (the core decides): the catalog
    /// is listed, unavailable until a Seerr server is signed in. Ask again when the capabilities change.
    /// </summary>
    public SearchTools SearchToolsFor(SearchFilter filter)
    {
        ArgumentNullException.ThrowIfNull(filter);
        return SearchMap.From(_core.SearchTools(SearchMap.ToFfi(filter)));
    }

    /// <summary>What the search page opens with: its last filter and view, each only while its switch is on.</summary>
    public SearchChoices SearchChoices
    {
        get
        {
            var choices = _core.SearchChoices();
            return new(
                choices.Filter is { } filter ? SearchMap.From(filter) : null,
                choices.View is { } view ? new LibraryView(view.Tile, view.ShowTitles) : null);
        }
    }

    /// <summary>
    /// Remembers the search page's choices: each part given (not null) whose switch is on; the other stays as it was. Saved at once on
    /// the core's threads; nothing is written when nothing changed.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: nothing changed.</exception>
    public Task RememberSearchChoicesAsync(SearchChoices choices)
    {
        ArgumentNullException.ThrowIfNull(choices);
        var ffi = new Ffi.SearchChoices(
            choices.Filter is { } filter ? SearchMap.ToFfi(filter) : null,
            choices.View is { } view ? new Ffi.LibraryView(view.Tile, view.ShowTitles) : null);
        return SaveSettingsAsync(() => _core.RememberSearchChoices(ffi));
    }
}

internal static class SearchMap
{
    public static Ffi.SearchFilter ToFfi(SearchFilter filter) => new(
        filter.Kind,
        filter.Source switch
        {
            SearchSource.All => Ffi.SearchSource.All,
            SearchSource.Library => Ffi.SearchSource.Library,
            SearchSource.Catalog => Ffi.SearchSource.Catalog,
            _ => throw new ArgumentOutOfRangeException(nameof(filter), filter.Source, "unknown search source"),
        });

    public static SearchFilter From(Ffi.SearchFilter filter) => new(filter.Kind, From(filter.Source));

    private static SearchSource From(Ffi.SearchSource source) => source switch
    {
        Ffi.SearchSource.Library => SearchSource.Library,
        Ffi.SearchSource.Catalog => SearchSource.Catalog,
        _ => SearchSource.All,
    };

    private static SearchChoice From(Ffi.SearchChoice choice) => new(choice.Label, choice.Checked, choice.Enabled, choice.Tooltip);

    public static SearchTools From(Ffi.SearchTools tools) => new(
        Array.ConvertAll(tools.Types, choice => new SearchTypeChoice(choice.Kind, From(choice.Choice))),
        Array.ConvertAll(tools.Sources, choice => new SearchSourceChoice(From(choice.Source), From(choice.Choice))));
}
