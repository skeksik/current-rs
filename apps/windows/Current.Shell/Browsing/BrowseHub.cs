using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;

namespace Current.Shell.Browsing;

/// <summary>
/// The browse sessions of the window (ADR 0018): one <see cref="Browse"/> per signed-in Jellyfin server, its libraries for the
/// navigation, the library grids that stay open while the app runs (so going back to one keeps its place), and the core's played and
/// favourite notices. It follows the capabilities state: a server signed out of, removed or signed in to again gets its session
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

    /// <summary>Raised on the UI thread when a session starts or ends, or a server's libraries arrive.</summary>
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

    private void OnCapabilitiesChanged(object? sender, PropertyChangedEventArgs e) => Sync();

    /// <summary>
    /// A Jellyfin server signed in gets a session, one signed out or gone loses it; a server signed in as another user, or again
    /// after its session ended, gets a new one. A server that is unreachable keeps its session and its last libraries; they are read
    /// again once it answers.
    /// </summary>
    private void Sync()
    {
        if (_disposed)
        {
            return;
        }
        var changed = false;
        var live = _capabilities.Value.Servers
            .Where(status => status.Service == "jellyfin" && status.SignedIn)
            .ToDictionary(status => new ServerRef(status.Service, status.Key));
        foreach (var server in _servers.Keys.ToList())
        {
            if (!live.TryGetValue(server, out var status) || status.User?.Id != _servers[server].UserId)
            {
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
                    // Signed in, but the session is not live yet (a restore still checking it): the next status change tries again.
                    continue;
                }
                browse = new ServerBrowse(server, status.User?.Id, session);
                _servers[server] = browse;
                changed = true;
            }
            if (status.Reachability == Reachability.Reachable && !browse.LibrariesLoaded)
            {
                _ = LoadLibrariesAsync(browse);
            }
        }
        if (changed)
        {
            Changed?.Invoke(this, EventArgs.Empty);
        }
    }

    private async Task LoadLibrariesAsync(ServerBrowse server)
    {
        if (server.LoadingLibraries)
        {
            return;
        }
        server.LoadingLibraries = true;
        try
        {
            var libraries = await server.Session.LibrariesAsync();
            if (_disposed || server.IsDisposed)
            {
                return;
            }
            server.Libraries = libraries;
            server.LibrariesLoaded = true;
            Changed?.Invoke(this, EventArgs.Empty);
        }
        catch (BrowseRequestException)
        {
            // Unreachable or refused: the navigation keeps what it had, and the next status change tries again.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the libraries failed: {error.GetType().Name}");
        }
        finally
        {
            server.LoadingLibraries = false;
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
        foreach (var server in _servers.Values)
        {
            server.Dispose();
        }
        _servers.Clear();
    }
}

/// <summary>One signed-in server's browse session, its libraries, and the grids opened on it.</summary>
internal sealed class ServerBrowse(ServerRef server, string? userId, Browse session) : IDisposable
{
    private readonly Dictionary<ItemRef, LibraryGrid> _grids = [];

    /// <summary>The detail views of the history's detail pages, by their navigation parameter (the same object on Back).</summary>
    private readonly Dictionary<object, KeptDetails> _details = new(ReferenceEqualityComparer.Instance);
    private HomeFeed? _home;

    public ServerRef Server { get; } = server;

    public string? UserId { get; } = userId;

    public Browse Session { get; } = session;

    /// <summary>The user's libraries in the user's order; empty until the server first answered.</summary>
    public IReadOnlyList<Library> Libraries { get; set; } = [];

    public bool LibrariesLoaded { get; set; }

    public bool LoadingLibraries { get; set; }

    public bool IsDisposed { get; private set; }

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

    /// <summary>When Home was last shown: shown again after five minutes, it refreshes.</summary>
    public DateTime HomeLastShown { get; set; } = DateTime.UtcNow;

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
            kept = new KeptDetails(Session.Details(item));
            _details[args] = kept;
        }
        return kept;
    }

    /// <summary>Drops the views of entries no longer in the history (<paramref name="live"/>: the parameters still in it).</summary>
    public void PruneDetails(IReadOnlySet<object> live)
    {
        foreach (var args in _details.Keys.Where(args => !live.Contains(args)).ToList())
        {
            _details[args].View.Dispose();
            _details.Remove(args);
        }
    }

    public void Dispose()
    {
        IsDisposed = true;
        foreach (var kept in _details.Values)
        {
            kept.View.Dispose();
        }
        _details.Clear();
        _home?.Dispose();
        _home = null;
        foreach (var grid in _grids.Values)
        {
            grid.Items.Dispose();
        }
        _grids.Clear();
        Session.Dispose();
    }
}

/// <summary>A library's paged items, with the query it shows and where the user left it.</summary>
internal sealed class LibraryGrid(PagedItems items, ItemQuery query)
{
    public PagedItems Items { get; } = items;

    public ItemQuery Query { get; set; } = query;

    /// <summary>The first item the grid showed when its page was left, to return to.</summary>
    public int FirstVisible { get; set; }

    /// <summary>When its page was last shown: shown again after five minutes, it refreshes (ADR 0018, section 3).</summary>
    public DateTime LastShown { get; set; } = DateTime.UtcNow;
}

/// <summary>A history entry's detail view, with the season chosen in it and where its page was scrolled to.</summary>
internal sealed class KeptDetails(DetailsView view)
{
    public DetailsView View { get; } = view;

    /// <summary>The season last chosen, or null for the core's first.</summary>
    public ItemRef? Season { get; set; }

    public double Offset { get; set; }
}
