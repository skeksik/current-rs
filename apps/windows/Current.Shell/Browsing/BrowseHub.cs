using System.ComponentModel;
using Current.Bindings;

namespace Current.Shell.Browsing;

/// <summary>
/// The browse sessions of the window (ADR 0018): one <see cref="Browse"/> per signed-in Jellyfin server, its libraries for the
/// navigation as the core's list follows the server (added, renamed, reordered, gone), the library grids that stay open while the app
/// runs (so going back to one keeps its place), and the core's played and favourite notices. It follows the capabilities state: a server signed out of, removed or signed in to again gets its session
/// dropped or a new one, and everything browse-related hides while no server is signed in (<see cref="IsAvailable"/>). Use it on
/// the UI thread; dispose it with the window.
/// </summary>
internal sealed class BrowseHub : IDisposable
{
    private readonly CoreHost _core;
    private readonly IUiDispatcher _ui;
    private readonly ValueSubscription<Capabilities> _capabilities;
    private readonly ValueSubscription<BrowseNotice?> _notices;
    private readonly Dictionary<ServerRef, ServerBrowse> _servers = [];
    private ulong _lastNotice;
    private bool _disposed;

    /// <summary>The searches of the history's search pages, by their navigation parameter (the same object on Back).</summary>
    private readonly Dictionary<object, KeptSearch> _searches = new(ReferenceEqualityComparer.Instance);

    private ulong _searchUses;

    /// <summary>
    /// How many history entries keep their search: the most recently shown. Each is refreshed with every list on return to the
    /// foreground, so older entries search again on Back.
    /// </summary>
    private const int KeptSearches = 5;

    public BrowseHub(CoreHost core, IUiDispatcher ui)
    {
        _core = core;
        _ui = ui;
        _capabilities = core.ObserveCapabilities(ui);
        _capabilities.PropertyChanged += OnCapabilitiesChanged;
        _notices = core.ObserveBrowseNotices(ui);
        _notices.PropertyChanged += OnNotice;
        Sync();
    }

    /// <summary>Raised on the UI thread when a session starts or ends, or a server's libraries change.</summary>
    public event EventHandler? Changed;

    public CoreHost Core => _core;

    /// <summary>At least one Jellyfin server is signed in with a live session: browse features show.</summary>
    public bool IsAvailable => _servers.Count > 0;

    /// <summary>The servers with a browse session, in the order they were saved.</summary>
    public IReadOnlyList<ServerBrowse> Servers =>
        [.. _core.Servers.Select(saved => _servers.GetValueOrDefault(saved.Server)).OfType<ServerBrowse>()];

    public ServerBrowse? Find(ServerRef server) => _servers.GetValueOrDefault(server);

    /// <summary>The saved name of <paramref name="server"/>, for the navigation's group header.</summary>
    public string ServerName(ServerRef server) =>
        _core.Servers.FirstOrDefault(saved => saved.Server == server)?.Name ?? "";

    /// <summary>The library's name as the navigation shows it, or null if it is no longer known.</summary>
    public Library? FindLibrary(ItemRef library) =>
        Find(library.Server)?.Libraries.FirstOrDefault(known => known.Item == library);

    /// <summary>
    /// Whether <paramref name="library"/> left its server's list in this session: the core no longer lists it (removed on the server,
    /// or hidden from the user). A library never listed (the smoke run opens one by its id) is not gone.
    /// </summary>
    public bool IsGone(ItemRef library) => Find(library.Server)?.IsGone(library) ?? false;

    private void OnCapabilitiesChanged(object? sender, PropertyChangedEventArgs e) => Sync();

    /// <summary>
    /// A Jellyfin server with a live session gets a browse session, one signed out or gone loses it, and one whose session number
    /// changed (a new sign-in, even as the same user) gets a new one: the core names the session in the status
    /// (<see cref="ServerStatus.Session"/>), so the hub decides nothing itself. A server that is unreachable keeps its session and its
    /// last libraries; the core reads them again once it answers.
    /// </summary>
    private void Sync()
    {
        if (_disposed)
        {
            return;
        }
        var changed = false;
        var live = _capabilities.Value.Servers
            .Where(status => status.Service == "jellyfin" && status.SignedIn && status.Session is not null)
            .ToDictionary(status => new ServerRef(status.Service, status.Key));
        foreach (var server in _servers.Keys.ToList())
        {
            if (!live.TryGetValue(server, out var status) || status.Session != _servers[server].SessionNumber)
            {
                _servers[server].LibrariesChanged -= OnLibrariesChanged;
                _servers[server].Dispose();
                _servers.Remove(server);
                changed = true;
            }
        }
        foreach (var (server, status) in live)
        {
            if (!_servers.TryGetValue(server, out var browse))
            {
                Browse session;
                try
                {
                    session = _core.Browse(server);
                }
                catch (Exception error) when (error is NotSignedInException or UnknownServerException)
                {
                    // The session ended between the status and this call: the next status change tries again.
                    continue;
                }
                browse = new ServerBrowse(server, status.Session, session, _ui);
                browse.LibrariesChanged += OnLibrariesChanged;
                _servers[server] = browse;
                changed = true;
            }
        }
        if (changed)
        {
            // A search spans every signed-in server: one that came or went makes the kept ones out of date.
            ForgetSearches(_ => true);
            Changed?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>
    /// The search for a history entry (<paramref name="args"/>, the page's navigation parameter), with one section per signed-in
    /// server, kept from now on: Back to a search page shows the results it had, without searching again and without its rows
    /// arriving one by one (the owner's run, 2026-10-25). New for an entry not seen, or one let go of as one of the older ones.
    /// </summary>
    public IReadOnlyList<SearchSection> Search(object args, string term)
    {
        if (!_searches.TryGetValue(args, out var kept))
        {
            var servers = Servers;
            var sections = servers
                .Select(server => new SearchSection(servers.Count > 1 ? ServerName(server.Server) : "", server.Session.Search(term), _ui))
                .ToList();
            kept = new KeptSearch(sections);
            _searches[args] = kept;
        }
        kept.LastUsed = ++_searchUses;
        var older = _searches.OrderByDescending(entry => entry.Value.LastUsed).Skip(KeptSearches).Select(entry => entry.Key).ToHashSet(
            ReferenceEqualityComparer.Instance);
        ForgetSearches(older.Contains);
        return kept.Sections;
    }

    /// <summary>Drops the searches of entries no longer in the history (<paramref name="live"/>: the parameters still in it).</summary>
    public void PruneSearches(IReadOnlySet<object> live) => ForgetSearches(args => !live.Contains(args));

    private void ForgetSearches(Func<object, bool> which)
    {
        foreach (var args in _searches.Keys.Where(which).ToList())
        {
            foreach (var section in _searches[args].Sections)
            {
                section.Dispose();
            }
            _searches.Remove(args);
        }
    }

    private void OnLibrariesChanged(object? sender, EventArgs e)
    {
        if (!_disposed && sender is ServerBrowse { IsDisposed: false })
        {
            Changed?.Invoke(this, EventArgs.Empty);
        }
    }

    /// <summary>A played or favourite change the server did not take was rolled back: say so once, above every page.</summary>
    private void OnNotice(object? sender, PropertyChangedEventArgs e)
    {
        if (_notices.Value is not BrowseNotice.ActionFailed failed || failed.Seq <= _lastNotice)
        {
            return;
        }
        _lastNotice = failed.Seq;
        App.ShowNotice(Microsoft.UI.Xaml.Controls.InfoBarSeverity.Warning, BrowseText.ActionFailedTitle(failed), BrowseText.Problem(failed.Problem));
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _capabilities.PropertyChanged -= OnCapabilitiesChanged;
        _capabilities.Dispose();
        _notices.PropertyChanged -= OnNotice;
        _notices.Dispose();
        ForgetSearches(_ => true);
        foreach (var server in _servers.Values)
        {
            server.LibrariesChanged -= OnLibrariesChanged;
            server.Dispose();
        }
        _servers.Clear();
    }
}

/// <summary>One signed-in server's browse session, its libraries, and the grids opened on it.</summary>
internal sealed class ServerBrowse : IDisposable
{
    private readonly Dictionary<ItemRef, LibraryGrid> _grids = [];

    /// <summary>The core's list of the user's libraries: it follows the server.</summary>
    private readonly ValueSubscription<IReadOnlyList<Library>?> _libraries;

    /// <summary>Libraries that left the list in this session; one listed again leaves this set.</summary>
    private readonly HashSet<ItemRef> _gone = [];

    public ServerBrowse(ServerRef server, ulong? sessionNumber, Browse session, IUiDispatcher ui)
    {
        Server = server;
        SessionNumber = sessionNumber;
        Session = session;
        _libraries = session.ObserveLibraries(ui);
        _libraries.PropertyChanged += OnLibrariesChanged;
    }

    /// <summary>Raised on the UI thread when the core's list of libraries changed.</summary>
    public event EventHandler? LibrariesChanged;

    /// <summary>The detail views of the history's detail pages, by their navigation parameter (the same object on Back).</summary>
    private readonly Dictionary<object, KeptDetails> _details = new(ReferenceEqualityComparer.Instance);
    private HomeFeed? _home;

    public ServerRef Server { get; }

    /// <summary>The core's number for the session this browses (<see cref="ServerStatus.Session"/>).</summary>
    public ulong? SessionNumber { get; }

    public Browse Session { get; }

    /// <summary>The user's libraries in the user's order; empty until the server first answered.</summary>
    public IReadOnlyList<Library> Libraries { get; private set; } = [];

    public bool IsDisposed { get; private set; }

    /// <summary>Whether <paramref name="library"/> left the list in this session (see <see cref="BrowseHub.IsGone"/>).</summary>
    public bool IsGone(ItemRef library) => _gone.Contains(library);

    /// <summary>
    /// The core's list changed. A library that left a loaded list is gone: the window hears of it first (the page showing it goes
    /// back to Home), then its grid is retired, which frees it once no page binds to it any more.
    /// </summary>
    private void OnLibrariesChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (IsDisposed || _libraries.Value is not { } libraries)
        {
            return;
        }
        var listed = libraries.Select(library => library.Item).ToHashSet();
        var left = Libraries.Select(library => library.Item).Where(item => !listed.Contains(item)).ToList();
        _gone.UnionWith(left);
        _gone.ExceptWith(listed);
        Libraries = libraries;
        LibrariesChanged?.Invoke(this, EventArgs.Empty);
        foreach (var item in left)
        {
            if (_grids.Remove(item, out var grid))
            {
                grid.Retire();
            }
        }
    }

    /// <summary>
    /// The grid of <paramref name="library"/>, opened on first use and kept for the life of the session, so a page shown again binds
    /// to the slots it had and returns to the item it showed (ADR 0018, section 3: loaded pages stay loaded for the life of the list).
    /// </summary>
    public LibraryGrid Grid(Library library)
    {
        if (!_grids.TryGetValue(library.Item, out var grid))
        {
            grid = new LibraryGrid(Session.OpenLibrary(library), ItemQuery.ForLibrary(library));
            _grids[library.Item] = grid;
        }
        return grid;
    }

    /// <summary>Home's rows of this server, opened on first use and kept, as the grids are, so Home shows again without reloading.</summary>
    public HomeFeed Home => _home ??= Session.Home();


    /// <summary>
    /// The detail view kept for a history entry (<paramref name="args"/>, the page's navigation parameter), or a new one kept from
    /// now on: Back to a detail page binds the new page to the view it had, with its details, the season chosen in it, and where the
    /// page was scrolled to (the owner's choice, 2026-10-25). One page type serves every item, so the frame cannot keep the pages
    /// themselves.
    /// </summary>
    public KeptDetails Details(object args, ItemRef item)
    {
        if (!_details.TryGetValue(args, out var kept))
        {
            kept = new KeptDetails();
            _details[args] = kept;
        }
        if (kept.View is null)
        {
            // New, or let go of as one of the older entries (PruneDetails): opened again, on the season chosen in it before.
            kept.View = Session.Details(item);
            if (kept.Season is { } season)
            {
                kept.View.SelectSeason(season);
            }
        }
        kept.LastUsed = ++_detailsUses;
        TrimLiveDetails();
        return kept;
    }

    /// <summary>
    /// How many history entries keep a live detail view: the most recently shown. Each live view is refreshed with every list on
    /// return to the foreground (several requests each), so older entries keep only their season and offsets and open their view
    /// again on Back (review, 2026-10-25).
    /// </summary>
    private const int LiveDetailsViews = 5;

    private ulong _detailsUses;

    private void TrimLiveDetails()
    {
        foreach (var kept in _details.Values.Where(kept => kept.View is not null).OrderByDescending(kept => kept.LastUsed)
            .Skip(LiveDetailsViews))
        {
            kept.View!.Dispose();
            kept.View = null;
        }
    }

    /// <summary>Drops the entries no longer in the history (<paramref name="live"/>: the parameters still in it).</summary>
    public void PruneDetails(IReadOnlySet<object> live)
    {
        foreach (var args in _details.Keys.Where(args => !live.Contains(args)).ToList())
        {
            _details[args].View?.Dispose();
            _details.Remove(args);
        }
    }

    public void Dispose()
    {
        IsDisposed = true;
        _libraries.PropertyChanged -= OnLibrariesChanged;
        _libraries.Dispose();
        foreach (var kept in _details.Values)
        {
            kept.View?.Dispose();
        }
        _details.Clear();
        _home?.Dispose();
        _home = null;
        foreach (var grid in _grids.Values)
        {
            grid.Retire();
        }
        _grids.Clear();
        Session.Dispose();
    }
}

/// <summary>A library's paged items, with the query it shows and where the user left it.</summary>
internal sealed class LibraryGrid(PagedItems items, ItemQuery query)
{
    /// <summary>How many pages bind to it now: a new page can bind before the one it replaced has unloaded.</summary>
    private int _bindings;
    private bool _retired;
    private bool _freed;

    public PagedItems Items { get; } = items;

    /// <summary>A page binds its slots to the grid (LibraryPage, on navigation to it).</summary>
    public void Bind() => _bindings++;

    /// <summary>The page let go of the grid's slots (on Unloaded): a retired grid is freed now.</summary>
    public void Unbind()
    {
        _bindings = Math.Max(0, _bindings - 1);
        FreeIfUnused();
    }

    /// <summary>
    /// Its library is gone or its session ended: freed now, or once the page still bound to it lets go, so the page's grid never
    /// calls into freed native items (review, 2026-10-25).
    /// </summary>
    public void Retire()
    {
        _retired = true;
        FreeIfUnused();
    }

    private void FreeIfUnused()
    {
        if (_retired && _bindings == 0 && !_freed)
        {
            _freed = true;
            Items.Dispose();
        }
    }

    public ItemQuery Query { get; set; } = query;

    /// <summary>The View menu's choices: wide tiles instead of posters, and the title and year under each picture.</summary>
    public bool Wide { get; set; }

    public bool ShowsTitles { get; set; } = true;

    /// <summary>The first item the grid showed when its page was left, to return to.</summary>
    public int FirstVisible { get; set; }

}

/// <summary>A history entry's detail view, with the season chosen in it and where its page was scrolled to.</summary>
internal sealed class KeptDetails
{
    /// <summary>Live for the most recently shown entries only (ServerBrowse.Details); null for older ones until shown again.</summary>
    public DetailsView? View { get; set; }

    /// <summary>When it was last shown, counted per session, to keep the most recent live.</summary>
    public ulong LastUsed { get; set; }

    /// <summary>The season last chosen, or null for the core's first.</summary>
    public ItemRef? Season { get; set; }

    public double Offset { get; set; }

    /// <summary>The sideways offsets of the season bar, the cast and the similar items.</summary>
    public double SeasonsOffset { get; set; }

    public double PeopleOffset { get; set; }

    public double SimilarOffset { get; set; }
}

/// <summary>A history entry's search: its sections, one per server, and when it was last shown.</summary>
internal sealed class KeptSearch(IReadOnlyList<SearchSection> sections)
{
    public IReadOnlyList<SearchSection> Sections { get; } = sections;

    public ulong LastUsed { get; set; }
}
