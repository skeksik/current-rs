using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// Browsing (ADR 0018, section 7): CoreHost.Browse gives a Browse for a signed-in Jellyfin server; its grids, Home, detail pages and
// searches are mirrored on the UI thread as ListSubscription and ValueSubscription, as the M1 flows are. Each object owns its native
// handle: dispose it, and every subscription it gave, when its page goes away.

public sealed partial class CoreHost
{
    /// <summary>
    /// Browsing <paramref name="server"/>. The Browse belongs to the server's current session: after a sign-out, a removal or a new
    /// sign-in its lists stop loading and its requests fail with <see cref="BrowseProblem.NotSignedIn"/>; ask for a new one then.
    /// </summary>
    /// <exception cref="UnknownServerException">No such server is saved.</exception>
    /// <exception cref="NotSignedInException">It is not a Jellyfin server signed in with a live session.</exception>
    public Browse Browse(ServerRef server) => new(CoreCall.Run(() => _core.Browse(Map.ToFfi(server))));

    /// <summary>
    /// The latest browse notice (a played or favourite change a server did not take, rolled back), mirrored on the UI thread. Null
    /// until the first. Show each new <see cref="BrowseNotice.Seq"/> once, as a message. Dispose it when the window goes away.
    /// </summary>
    public ValueSubscription<BrowseNotice?> ObserveBrowseNotices(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<BrowseNotice?>(null);
        subscription.Attach(_core.SubscribeBrowseNotices(new BrowseNoticeForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>
    /// An image's encoded bytes: JPEG or PNG from a server, BMP for a BlurHash. Runs on the core's threads; identical requests share one
    /// fetch, and cancelling <paramref name="ct"/> (a recycled tile) stops the fetch once no request waits for it.
    /// </summary>
    /// <exception cref="ImageLoadException">Not signed in, the server's or the network's failure, not an image, an empty box.</exception>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled first.</exception>
    public Task<byte[]> LoadImageAsync(ImageRequest request, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(request);
        var native = MapBrowse.ToFfi(request);
        return CoreCall.RunAsync(token => _core.LoadImage(native, token), ct);
    }

    /// <summary>
    /// The image cache's size on disk in bytes, every server's images together: what Settings → Storage shows. 0 for a core that
    /// stores nothing (<see cref="Create"/>).
    /// </summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled first.</exception>
    public async Task<long> ImageCacheSizeAsync(CancellationToken ct = default)
    {
        var size = await CoreCall.RunAsync(token => _core.ImageCacheSize(token), ct).ConfigureAwait(false);
        return (long)Math.Min(size, long.MaxValue);
    }

    /// <summary>
    /// Deletes every cached image of every server, in memory and on disk (Settings → Storage, "Clear"); they load again from the
    /// servers when next shown. Safe while images load. Not cancellable: it runs to its end once started.
    /// </summary>
    public Task ClearImageCacheAsync() =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await _core.ClearImageCache().ConfigureAwait(false);
            return true;
        });
}

/// <summary>Browsing one session of a signed-in Jellyfin server. Dispose it when no page uses it.</summary>
public sealed class Browse : IDisposable
{
    private readonly Ffi.Browse _browse;

    internal Browse(Ffi.Browse browse)
    {
        _browse = browse;
    }

    public ServerRef Server => Map.From(_browse.Server());

    /// <summary>
    /// The user's libraries, in the user's order, as a load that starts now reads them; <see cref="ObserveLibraries"/> hears them too.
    /// </summary>
    /// <exception cref="BrowseRequestException">The session ended, or the server's or the network's failure.</exception>
    public async Task<IReadOnlyList<Library>> LibrariesAsync(CancellationToken ct = default)
    {
        var libraries = await CoreCall.RunAsync(token => _browse.Libraries(token), ct).ConfigureAwait(false);
        return Array.ConvertAll(libraries, library => MapBrowse.From(library));
    }

    /// <summary>
    /// The user's libraries for the navigation, in the user's order, mirrored on the UI thread: null until first loaded, then the last
    /// list loaded. The core reads them again on a refresh, on return to the foreground, when Home reloads, after a library request
    /// the server refused, and when the server is reached again after a failed load; a library the user can no longer open leaves
    /// the list. Dispose it with the session.
    /// </summary>
    public ValueSubscription<IReadOnlyList<Library>?> ObserveLibraries(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<IReadOnlyList<Library>?>(null);
        subscription.Attach(_browse.SubscribeLibraries(new LibrariesForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>
    /// A grid of <paramref name="library"/>'s items, sorted and filtered by <paramref name="query"/>; without one, what
    /// <see cref="ItemQuery.ForLibrary"/> gives. Starts loading at once. When a request fails as it does for a library that is gone,
    /// the core reads the libraries again; if the library left them, the state's failure is <see cref="BrowseProblem.NotAvailable"/>.
    /// </summary>
    public PagedItems OpenLibrary(Library library, ItemQuery? query = null)
    {
        ArgumentNullException.ThrowIfNull(library);
        return new(_browse.OpenLibrary(MapBrowse.ToFfi(library), MapBrowse.ToFfi(query ?? ItemQuery.ForLibrary(library))));
    }

    /// <summary>A grid of items from every library (a Favourites row's whole list, the Recently added tab): <paramref name="query"/>, with no parent.
    /// Starts loading at once.</summary>
    public PagedItems OpenItems(ItemQuery query)
    {
        ArgumentNullException.ThrowIfNull(query);
        return new(_browse.OpenItems(MapBrowse.ToFfi(query)));
    }

    /// <summary>Home's rows. Starts loading at once.</summary>
    public HomeFeed Home() => new(_browse.Home());

    /// <summary><paramref name="item"/>'s detail page. Starts loading at once.</summary>
    public DetailsView Details(ItemRef item) => new(_browse.Details(MapBrowse.ToFfi(item)));

    /// <summary>The best matches for <paramref name="term"/>, one group per kind. Starts loading at once.</summary>
    public SearchResults Search(string term) => new(_browse.Search(term));

    /// <summary>
    /// The Favourites tab: the user's favourite films, series and episodes, a group each, in that order, each only while it has items.
    /// They read again a moment after a mark changes. A group's header opens <see cref="ItemQuery.Favourites"/> with its kind in
    /// <see cref="OpenItems"/>. Starts loading at once.
    /// </summary>
    public SearchResults Favourites() => new(_browse.Favourites());

    /// <summary>
    /// Marks <paramref name="item"/> played or unplayed: every live list shows it at once, the server is told, and on failure the
    /// change rolls back with a notice (<see cref="CoreHost.ObserveBrowseNotices"/>).
    /// </summary>
    public void SetPlayed(ItemRef item, bool played) => _browse.SetPlayed(MapBrowse.ToFfi(item), played);

    /// <summary>Marks <paramref name="item"/> a favourite or not, as <see cref="SetPlayed"/> does.</summary>
    public void SetFavourite(ItemRef item, bool favourite) => _browse.SetFavourite(MapBrowse.ToFfi(item), favourite);

    /// <summary><paramref name="item"/>'s marks as the lists show them; null unless a list loaded it.</summary>
    public UserState? GetUserState(ItemRef item) =>
        _browse.UserState(MapBrowse.ToFfi(item)) is { } state ? MapBrowse.From(state) : null;

    /// <summary>Reads every live list of this session again, without resetting them.</summary>
    public void Refresh() => _browse.Refresh();

    public void Dispose() => _browse.Dispose();
}

/// <summary>
/// A library grid: a list of slots sized to the query's total, filled a page at a time around what the UI shows (ADR 0018, section 3).
/// Bind a <see cref="PagedCollection{T}"/> from <see cref="Collect{T}"/> to the grid.
/// </summary>
public sealed class PagedItems : IDisposable
{
    private readonly Ffi.PagedItems _items;

    internal PagedItems(Ffi.PagedItems items)
    {
        _items = items;
    }

    public PagedState State => MapBrowse.From(_items.State());

    /// <summary>The state (total, loading, the failure for one Retry message), mirrored on the UI thread.</summary>
    public ValueSubscription<PagedState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<PagedState>(State);
        subscription.Attach(_items.SubscribeState(new PagedStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>The slots as they are, mirrored on the UI thread: a placeholder record for each slot not loaded.</summary>
    public PagedCollection<Slot> Collect(IUiDispatcher ui) => Collect(ui, slot => slot);

    /// <summary>
    /// The slots mirrored on the UI thread, each mapped (on a core thread) to what a tile binds to. <paramref name="map"/> must never
    /// return null, for a placeholder slot either: a null fails the batch, as a mapping that throws does.
    /// </summary>
    /// <param name="items">The collection to fill, empty: a subclass with a platform interface (the Windows app adds
    /// IItemsRangeInfo). A new one when null.</param>
    public PagedCollection<T> Collect<T>(IUiDispatcher ui, Func<Slot, T> map, DiffObservableCollection<T>? items = null) =>
        new(this, _items, ui, map, items);

    /// <summary>The UI shows slots <paramref name="first"/> to <paramref name="last"/> (inclusive): the pages around them load.</summary>
    public void SetVisibleRange(int first, int last)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(first);
        ArgumentOutOfRangeException.ThrowIfNegative(last);
        _items.SetVisibleRange((uint)first, (uint)last);
    }

    /// <summary>Reads the list again without resetting it: the scroll position stays.</summary>
    public void Refresh() => _items.Refresh();

    /// <summary>The page showing the grid was left.</summary>
    public void Hidden() => _items.Hidden();

    /// <summary>
    /// The page showing the grid is shown again: it refreshes, without a reset, if it was left five minutes or more ago (the core
    /// decides). Returns whether it refreshed.
    /// </summary>
    public bool Shown() => _items.Shown();

    /// <summary>Reloads every failed page (the Retry button).</summary>
    public void Retry() => _items.Retry();

    /// <summary>A new sort or filter: a new query, which resets the list and starts at the top.</summary>
    public void SetQuery(ItemQuery query) => _items.SetQuery(MapBrowse.ToFfi(query));

    /// <summary>Whether <see cref="Dispose"/> ran: a collection of it then sends nothing more.</summary>
    public bool IsDisposed { get; private set; }

    public void Dispose()
    {
        if (IsDisposed)
        {
            return;
        }
        IsDisposed = true;
        _items.Dispose();
    }
}

/// <summary>
/// A grid's slots mirrored on the UI thread (<see cref="Items"/>), with the visible range forwarded to the core. Every item is non-null:
/// slots still loading or failed are placeholder items. Dispose it when the grid goes away; its <see cref="PagedItems"/> stays.
/// </summary>
public sealed class PagedCollection<T> : IDisposable
{
    private readonly PagedItems _owner;
    private readonly Ffi.PagedItems _items;
    private readonly ListSubscription<T> _slots;
    private (int First, int Last)? _range;

    internal PagedCollection(
        PagedItems owner, Ffi.PagedItems items, IUiDispatcher ui, Func<Slot, T> map, DiffObservableCollection<T>? collection)
    {
        ArgumentNullException.ThrowIfNull(map);
        _owner = owner;
        _items = items;
        _slots = new ListSubscription<T>(
            (subscription, generation) => items.SubscribeSlots(new SlotsForwarder<T>(subscription, ui, generation, map)),
            collection);
    }

    /// <summary>Sized to the query's total once the first page answered; changes only on the UI thread.</summary>
    public DiffObservableCollection<T> Items => _slots.Items;

    /// <summary>The mirror's subscription: its recoveries and whether it faulted.</summary>
    public ListSubscription<T> Subscription => _slots;

    /// <summary>
    /// The grid shows items <paramref name="first"/> to <paramref name="last"/> (inclusive, either order). Call it from the UI thread
    /// whenever the view changes (IItemsRangeInfo.RangesChanged's visible range); a range that did not change, or one with a negative
    /// end (an empty view), is not sent, nor anything once this or its <see cref="PagedItems"/> is disposed.
    /// </summary>
    public void SetVisibleRange(int first, int last)
    {
        if (_slots.IsDisposed || _owner.IsDisposed || first < 0 || last < 0 || _range == (first, last))
        {
            return;
        }
        _range = (first, last);
        _items.SetVisibleRange((uint)first, (uint)last);
    }

    public void Dispose() => _slots.Dispose();
}

/// <summary>Home: rows, each with its own list (ADR 0018, section 6). Its layout is provisional.</summary>
public sealed class HomeFeed : IDisposable
{
    private readonly Ffi.HomeFeed _feed;

    internal HomeFeed(Ffi.HomeFeed feed)
    {
        _feed = feed;
    }

    /// <summary>The rows, in order, mirrored on the UI thread. A row exists only while it has items.</summary>
    public ListSubscription<HomeRow> ObserveRows(IUiDispatcher ui) =>
        new((subscription, generation) => _feed.SubscribeRows(new HomeRowsForwarder(subscription, ui, generation)));

    /// <summary>
    /// The items of the row with <paramref name="key"/>, mirrored on the UI thread: one subscription per row. Null if no such row exists
    /// now; a row's list exists before the row appears in <see cref="ObserveRows"/>.
    /// </summary>
    public ListSubscription<ItemSummary>? ObserveRow(string key, IUiDispatcher ui)
    {
        var row = new ListSubscription<ItemSummary>((subscription, generation) =>
            _feed.SubscribeRow(key, new ItemsForwarder(subscription, ui, generation)));
        if (row.IsSubscribed)
        {
            return row;
        }
        row.Dispose();
        return null;
    }

    public HomeState State => MapBrowse.From(_feed.State());

    public ValueSubscription<HomeState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<HomeState>(State);
        subscription.Attach(_feed.SubscribeState(new HomeStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Loads every row again, keeping what they show until the answers arrive.</summary>
    public void Refresh() => _feed.Refresh();

    /// <summary>Home was left.</summary>
    public void Hidden() => _feed.Hidden();

    /// <summary>Home is shown again: its rows load again if it was left five minutes or more ago (the core decides).</summary>
    public bool Shown() => _feed.Shown();

    /// <summary>The Retry button.</summary>
    public void Retry() => _feed.Retry();

    public void Dispose() => _feed.Dispose();
}

/// <summary>An item's detail page: its details, and a series' seasons, a season's episodes and similar items.</summary>
public sealed class DetailsView : IDisposable
{
    private readonly Ffi.DetailsView _view;

    internal DetailsView(Ffi.DetailsView view)
    {
        _view = view;
    }

    public ItemRef Item => MapBrowse.From(_view.Item());

    /// <summary>The details, null until loaded, mirrored on the UI thread.</summary>
    public ValueSubscription<ItemDetails?> ObserveDetails(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<ItemDetails?>(null);
        subscription.Attach(_view.SubscribeDetails(new ItemDetailsForwarder(subscription, ui)));
        return subscription;
    }

    public ValueSubscription<LoadState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<LoadState>(new LoadState(true, null));
        subscription.Attach(_view.SubscribeState(new LoadStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>What the page's Play button starts: the item from its resume point, or a series' next episode (else its first); null until
    /// loaded and for an item that does not play.</summary>
    public ValueSubscription<PlayTarget?> ObservePlay(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<PlayTarget?>(null);
        subscription.Attach(_view.SubscribePlay(new PlayTargetForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>A series' seasons; empty for other kinds.</summary>
    public ListSubscription<ItemSummary> ObserveSeasons(IUiDispatcher ui) =>
        new((subscription, generation) => _view.SubscribeSeasons(new ItemsForwarder(subscription, ui, generation)));

    /// <summary>The picked season's episodes (<see cref="SelectSeason"/>), the first season's until then; empty unless a series.</summary>
    public ListSubscription<ItemSummary> ObserveEpisodes(IUiDispatcher ui) =>
        new((subscription, generation) => _view.SubscribeEpisodes(new ItemsForwarder(subscription, ui, generation)));

    public ListSubscription<ItemSummary> ObserveSimilar(IUiDispatcher ui) =>
        new((subscription, generation) => _view.SubscribeSimilar(new ItemsForwarder(subscription, ui, generation)));

    public void SelectSeason(ItemRef season) => _view.SelectSeason(MapBrowse.ToFfi(season));

    /// <summary>Loads the page again, keeping what it shows until the answers arrive.</summary>
    public void Refresh() => _view.Refresh();

    public void Dispose() => _view.Dispose();
}

/// <summary>The results of one search: one group per media kind.</summary>
public sealed class SearchResults : IDisposable
{
    private readonly Ffi.SearchResults _results;

    internal SearchResults(Ffi.SearchResults results)
    {
        _results = results;
    }

    /// <summary>The media kinds with matches, in order, mirrored on the UI thread.</summary>
    public ListSubscription<string> ObserveGroups(IUiDispatcher ui) =>
        new((subscription, generation) => _results.SubscribeGroups(new SearchGroupsForwarder(subscription, ui, generation)));

    /// <summary>The matches of <paramref name="kind"/>; null if it has none now.</summary>
    public ListSubscription<ItemSummary>? ObserveGroup(string kind, IUiDispatcher ui)
    {
        var group = new ListSubscription<ItemSummary>((subscription, generation) =>
            _results.SubscribeGroup(kind, new ItemsForwarder(subscription, ui, generation)));
        if (group.IsSubscribed)
        {
            return group;
        }
        group.Dispose();
        return null;
    }

    public ValueSubscription<LoadState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<LoadState>(new LoadState(true, null));
        subscription.Attach(_results.SubscribeState(new LoadStateForwarder(subscription, ui)));
        return subscription;
    }

    public void Refresh() => _results.Refresh();

    public void Dispose() => _results.Dispose();
}

internal static class MapBrowseDiffs
{
    private static int Index(uint index) => (int)Math.Min(index, int.MaxValue);

    /// <summary>A grid binds every item: WinUI gets a placeholder item, never null, for a slot not loaded.</summary>
    private static T NotNull<T>(T value) =>
        value is null ? throw new InvalidOperationException("a slot was mapped to null; placeholders must be items") : value;

    public static ListDiff<T> From<T>(Ffi.SlotDiff diff, Func<Slot, T> map)
    {
        T One(Ffi.Slot slot) => NotNull(map(MapBrowse.From(slot)));
        return diff switch
        {
            Ffi.SlotDiff.Append append => new ListDiff<T>.Append(Array.ConvertAll(append.Values, One)),
            Ffi.SlotDiff.Insert insert => new ListDiff<T>.Insert(Index(insert.Index), One(insert.Value)),
            Ffi.SlotDiff.Set set => new ListDiff<T>.Set(Index(set.Index), One(set.Value)),
            Ffi.SlotDiff.Remove remove => new ListDiff<T>.Remove(Index(remove.Index)),
            Ffi.SlotDiff.Reset reset => new ListDiff<T>.Reset(Array.ConvertAll(reset.Values, One)),
            _ => throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff)),
        };
    }

    public static ListDiff<ItemSummary> From(Ffi.ItemSummaryDiff diff) => diff switch
    {
        Ffi.ItemSummaryDiff.Append append =>
            new ListDiff<ItemSummary>.Append(Array.ConvertAll(append.Values, value => MapBrowse.From(value))),
        Ffi.ItemSummaryDiff.Insert insert => new ListDiff<ItemSummary>.Insert(Index(insert.Index), MapBrowse.From(insert.Value)),
        Ffi.ItemSummaryDiff.Set set => new ListDiff<ItemSummary>.Set(Index(set.Index), MapBrowse.From(set.Value)),
        Ffi.ItemSummaryDiff.Remove remove => new ListDiff<ItemSummary>.Remove(Index(remove.Index)),
        Ffi.ItemSummaryDiff.Reset reset =>
            new ListDiff<ItemSummary>.Reset(Array.ConvertAll(reset.Values, value => MapBrowse.From(value))),
        _ => throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff)),
    };

    public static ListDiff<HomeRow> From(Ffi.HomeRowDiff diff) => diff switch
    {
        Ffi.HomeRowDiff.Append append => new ListDiff<HomeRow>.Append(Array.ConvertAll(append.Values, value => MapBrowse.From(value))),
        Ffi.HomeRowDiff.Insert insert => new ListDiff<HomeRow>.Insert(Index(insert.Index), MapBrowse.From(insert.Value)),
        Ffi.HomeRowDiff.Set set => new ListDiff<HomeRow>.Set(Index(set.Index), MapBrowse.From(set.Value)),
        Ffi.HomeRowDiff.Remove remove => new ListDiff<HomeRow>.Remove(Index(remove.Index)),
        Ffi.HomeRowDiff.Reset reset => new ListDiff<HomeRow>.Reset(Array.ConvertAll(reset.Values, value => MapBrowse.From(value))),
        _ => throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff)),
    };

    public static ListDiff<string> From(Ffi.SearchGroupDiff diff) => diff switch
    {
        Ffi.SearchGroupDiff.Append append => new ListDiff<string>.Append(Array.ConvertAll(append.Values, value => value.Kind)),
        Ffi.SearchGroupDiff.Insert insert => new ListDiff<string>.Insert(Index(insert.Index), insert.Value.Kind),
        Ffi.SearchGroupDiff.Set set => new ListDiff<string>.Set(Index(set.Index), set.Value.Kind),
        Ffi.SearchGroupDiff.Remove remove => new ListDiff<string>.Remove(Index(remove.Index)),
        Ffi.SearchGroupDiff.Reset reset => new ListDiff<string>.Reset(Array.ConvertAll(reset.Values, value => value.Kind)),
        _ => throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff)),
    };
}

internal sealed class SlotsForwarder<T>(ListSubscription<T> target, IUiDispatcher ui, int generation, Func<Slot, T> map)
    : ListForwarder<Ffi.SlotDiff, T>(target, ui, generation, diff => MapBrowseDiffs.From(diff, map)), Ffi.SlotsListener
{
    public void OnDiffs(Ffi.SlotDiff[] diffs) => Forward(diffs);
}

internal sealed class ItemsForwarder(ListSubscription<ItemSummary> target, IUiDispatcher ui, int generation)
    : ListForwarder<Ffi.ItemSummaryDiff, ItemSummary>(target, ui, generation, MapBrowseDiffs.From), Ffi.ItemsListener
{
    public void OnDiffs(Ffi.ItemSummaryDiff[] diffs) => Forward(diffs);
}

internal sealed class HomeRowsForwarder(ListSubscription<HomeRow> target, IUiDispatcher ui, int generation)
    : ListForwarder<Ffi.HomeRowDiff, HomeRow>(target, ui, generation, MapBrowseDiffs.From), Ffi.HomeRowsListener
{
    public void OnDiffs(Ffi.HomeRowDiff[] diffs) => Forward(diffs);
}

internal sealed class SearchGroupsForwarder(ListSubscription<string> target, IUiDispatcher ui, int generation)
    : ListForwarder<Ffi.SearchGroupDiff, string>(target, ui, generation, MapBrowseDiffs.From), Ffi.SearchGroupsListener
{
    public void OnDiffs(Ffi.SearchGroupDiff[] diffs) => Forward(diffs);
}

internal sealed class PagedStateForwarder(ValueSubscription<PagedState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.PagedState, PagedState>(target, ui, MapBrowse.From), Ffi.PagedStateListener
{
    public void OnValue(Ffi.PagedState value) => Forward(value);
}

internal sealed class HomeStateForwarder(ValueSubscription<HomeState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.HomeState, HomeState>(target, ui, MapBrowse.From), Ffi.HomeStateListener
{
    public void OnValue(Ffi.HomeState value) => Forward(value);
}

internal sealed class LoadStateForwarder(ValueSubscription<LoadState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.LoadState, LoadState>(target, ui, MapBrowse.From), Ffi.LoadStateListener
{
    public void OnValue(Ffi.LoadState value) => Forward(value);
}

internal sealed class PlayTargetForwarder(ValueSubscription<PlayTarget?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.PlayTarget?, PlayTarget?>(target, ui, play => play is null ? null : MapBrowse.From(play)),
      Ffi.PlayTargetListener
{
    public void OnValue(Ffi.PlayTarget? value) => Forward(value);
}

internal sealed class ItemDetailsForwarder(ValueSubscription<ItemDetails?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.ItemDetails?, ItemDetails?>(target, ui, details => details is null ? null : MapBrowse.From(details)),
      Ffi.ItemDetailsListener
{
    public void OnValue(Ffi.ItemDetails? value) => Forward(value);
}

internal sealed class LibrariesForwarder(ValueSubscription<IReadOnlyList<Library>?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.Library[]?, IReadOnlyList<Library>?>(
        target, ui, libraries => libraries is null ? null : Array.ConvertAll(libraries, library => MapBrowse.From(library))),
      Ffi.LibrariesListener
{
    public void OnValue(Ffi.Library[]? value) => Forward(value);
}

internal sealed class BrowseNoticeForwarder(ValueSubscription<BrowseNotice?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.BrowseNotice?, BrowseNotice?>(target, ui, MapBrowse.From), Ffi.BrowseNoticeListener
{
    public void OnValue(Ffi.BrowseNotice? value) => Forward(value);
}
