using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// Settings' "Remember sort", "Remember filter" and "Remember view", and the choices the core remembers per library. The rules are the
// core's: a part is saved and given back only while its switch is on, and turning a switch off forgets that part for every library.
// Sort keys, filters and tile shapes are open values, passed through unchanged.

/// <summary>Settings' three switches, all off by default.</summary>
public sealed record LibraryMemorySettings(bool RememberSort, bool RememberFilter, bool RememberView);

/// <summary>A library grid's view.</summary>
/// <param name="Tile">The tiles' shape: <see cref="Poster"/> or <see cref="Wide"/>, or one a later version names.</param>
/// <param name="ShowTitles">Whether the tiles show their titles.</param>
public sealed record LibraryView(string Tile, bool ShowTitles)
{
    public const string Poster = "poster";
    public const string Wide = "wide";
}

/// <summary>
/// A library's choices, each part null when there is none. From <see cref="CoreHost.LibraryChoicesFor"/>: nothing remembered, or its
/// switch is off, and the grid opens with its first choice. To <see cref="CoreHost.RememberLibraryChoicesAsync"/>: left as it was.
/// <c>Filters</c> is empty for "All". Compares its list by reference (see Capabilities).
/// </summary>
public sealed record LibraryChoices(SortChoice? Sort = null, IReadOnlyList<string>? Filters = null, LibraryView? View = null);

public sealed partial class CoreHost
{
    /// <summary>Settings' "Remember sort", "Remember filter" and "Remember view" now.</summary>
    public LibraryMemorySettings LibraryMemory => LibraryMap.From(_core.LibraryMemory());

    /// <summary>
    /// The Settings switch "Remember sort". Saved at once on the core's threads; off forgets every library's sort in the same save,
    /// so on again starts afresh.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: nothing changed.</exception>
    public Task SetRememberSortAsync(bool on) => SaveSettingsAsync(() => _core.SetRememberSort(on));

    /// <summary>The Settings switch "Remember filter", as <see cref="SetRememberSortAsync"/>.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: nothing changed.</exception>
    public Task SetRememberFilterAsync(bool on) => SaveSettingsAsync(() => _core.SetRememberFilter(on));

    /// <summary>The Settings switch "Remember view", as <see cref="SetRememberSortAsync"/>.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: nothing changed.</exception>
    public Task SetRememberViewAsync(bool on) => SaveSettingsAsync(() => _core.SetRememberView(on));

    /// <summary>What <paramref name="library"/> opens with: the last sort, filters and view chosen there, each only while its switch is on.</summary>
    public LibraryChoices LibraryChoicesFor(ItemRef library)
    {
        ArgumentNullException.ThrowIfNull(library);
        return LibraryMap.From(_core.LibraryChoices(MapBrowse.ToFfi(library)));
    }

    /// <summary>
    /// Remembers the user's choices in <paramref name="library"/>: each part given (not null) whose switch is on; the others stay as
    /// they were. Pass a part each time the user changes it (all of them is fine too). Saved at once on the core's threads; nothing is
    /// written when nothing changed.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: nothing changed.</exception>
    public Task RememberLibraryChoicesAsync(ItemRef library, LibraryChoices choices)
    {
        ArgumentNullException.ThrowIfNull(library);
        ArgumentNullException.ThrowIfNull(choices);
        var ffiLibrary = MapBrowse.ToFfi(library);
        var ffiChoices = LibraryMap.ToFfi(choices);
        return SaveSettingsAsync(() => _core.RememberLibraryChoices(ffiLibrary, ffiChoices));
    }

    private static Task SaveSettingsAsync(Func<Task> call) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await call().ConfigureAwait(false);
            return true;
        });
}

internal static class LibraryMap
{
    public static LibraryMemorySettings From(Ffi.LibraryMemorySettings settings) =>
        new(settings.RememberSort, settings.RememberFilter, settings.RememberView);

    public static LibraryChoices From(Ffi.LibraryChoices choices) => new(
        choices.Sort is { } sort
            ? new SortChoice(sort.Key, sort.Order == Ffi.SortOrder.Descending ? SortOrder.Descending : SortOrder.Ascending)
            : null,
        choices.Filters,
        choices.View is { } view ? new LibraryView(view.Tile, view.ShowTitles) : null);

    public static Ffi.LibraryChoices ToFfi(LibraryChoices choices) => new(
        choices.Sort is { } sort
            ? new Ffi.SortChoice(
                sort.Key,
                sort.Order switch
                {
                    SortOrder.Ascending => Ffi.SortOrder.Ascending,
                    SortOrder.Descending => Ffi.SortOrder.Descending,
                    _ => throw new ArgumentOutOfRangeException(nameof(choices), sort.Order, "unknown sort order"),
                })
            : null,
        choices.Filters?.ToArray(),
        choices.View is { } view ? new Ffi.LibraryView(view.Tile, view.ShowTitles) : null);
}
