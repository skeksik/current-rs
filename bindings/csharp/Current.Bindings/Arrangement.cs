using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// The user's arrangement of the pane and Home (ADR 0024): the core keeps the order and the hidden state per server and user and
// publishes the pane's groups as arranged; the UI shows them and passes the user's moves, hides and resets back.

/// <summary>A group the user arranges.</summary>
public enum NavGroup
{
    /// <summary>The Movies and Series tabs, keyed by the kind each shows (<c>movie</c>, <c>series</c>).</summary>
    Tabs,
    /// <summary>Favourites and Recently added, keyed <see cref="PaneEntry.Favourites"/> and <see cref="PaneEntry.RecentlyAdded"/>.</summary>
    Views,
    /// <summary>The libraries, keyed by their item id.</summary>
    Libraries,
    /// <summary>Home's rows, keyed as <see cref="HomeRow.Key"/>.</summary>
    HomeRows,
}

/// <summary>A tab or a view in the pane.</summary>
public sealed record PaneEntry(string Key, bool Hidden)
{
    public const string Favourites = "favourites";
    public const string RecentlyAdded = "recently_added";
}

/// <summary>A library in the pane. <c>HiddenOnServer</c>: the server keeps it out of "My media", so it starts hidden.</summary>
public sealed record PaneLibrary(Library Library, bool Hidden, bool HiddenOnServer);

/// <summary>One server's pane groups as the user arranged them, hidden items included (the pane leaves them out).</summary>
public sealed record PaneLayout(IReadOnlyList<PaneEntry> Tabs, IReadOnlyList<PaneEntry> Views, IReadOnlyList<PaneLibrary> Libraries);

/// <summary>One of Home's rows for the editor, also one without items now.</summary>
public sealed record HomeRowSetting(HomeRow Row, bool Hidden);

public sealed partial class Browse
{
    /// <summary>
    /// The pane's groups as the user arranged them, mirrored on the UI thread: null until the libraries first loaded. It follows the
    /// libraries and every change saved on this device. Dispose it with the session.
    /// </summary>
    public ValueSubscription<PaneLayout?> ObservePane(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<PaneLayout?>(null);
        subscription.Attach(_browse.SubscribePane(new PaneForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Home's rows as the user arranged them, hidden ones and those without items now included, for the editor.</summary>
    public IReadOnlyList<HomeRowSetting> HomeLayout() =>
        Array.ConvertAll(_browse.HomeLayout(), setting => new HomeRowSetting(MapBrowse.From(setting.Row), setting.Hidden));

    /// <summary>
    /// Moves <paramref name="key"/> to <paramref name="index"/> in <paramref name="group"/> (an index in the group as arranged, hidden
    /// items included) and saves it. The pane's subscription and Home follow. Not cancellable: it is one small write.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved; nothing changed.</exception>
    public Task MoveAsync(NavGroup group, string key, int index) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await _browse.MoveItem(ToFfi(group), key, (uint)Math.Max(0, index)).ConfigureAwait(false);
            return true;
        });

    /// <summary>Hides or shows <paramref name="key"/> in <paramref name="group"/>, as <see cref="MoveAsync"/>.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved; nothing changed.</exception>
    public Task SetHiddenAsync(NavGroup group, string key, bool hidden) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await _browse.SetHidden(ToFfi(group), key, hidden).ConfigureAwait(false);
            return true;
        });

    /// <summary>Forgets the user's arrangement of <paramref name="group"/>: back to the server's or the app's order.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved; nothing changed.</exception>
    public Task ResetAsync(NavGroup group) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await _browse.ResetGroup(ToFfi(group)).ConfigureAwait(false);
            return true;
        });

    private static Ffi.NavGroup ToFfi(NavGroup group) => group switch
    {
        NavGroup.Tabs => Ffi.NavGroup.Tabs,
        NavGroup.Views => Ffi.NavGroup.Views,
        NavGroup.Libraries => Ffi.NavGroup.Libraries,
        _ => Ffi.NavGroup.HomeRows,
    };

    internal static PaneLayout From(Ffi.PaneLayout layout) => new(
        Array.ConvertAll(layout.Tabs, entry => new PaneEntry(entry.Key, entry.Hidden)),
        Array.ConvertAll(layout.Views, entry => new PaneEntry(entry.Key, entry.Hidden)),
        Array.ConvertAll(layout.Libraries, entry => new PaneLibrary(MapBrowse.From(entry.Library), entry.Hidden, entry.HiddenOnServer)));
}

internal sealed class PaneForwarder(ValueSubscription<PaneLayout?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.PaneLayout?, PaneLayout?>(target, ui, layout => layout is null ? null : Browse.From(layout)),
      Ffi.PaneListener
{
    public void OnValue(Ffi.PaneLayout? value) => Forward(value);
}
