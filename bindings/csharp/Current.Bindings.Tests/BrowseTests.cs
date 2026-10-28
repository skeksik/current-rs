using Current.Bindings;

/// <summary>
/// Browsing through the public adapters (ADR 0018, task 4), against the Jellyfin stand-in and its fake libraries: a paged grid's slots
/// and visible-range loading, a failed page and retry, image bytes and cancellation, a rolled-back toggle with its notice, Home rows,
/// and releasing every subscription. Each test has its own stand-in and a core that stores nothing.
/// </summary>
internal static class BrowseTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static readonly ServerRef FirstServer = new("jellyfin", "1");

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"expected: {what}");
        }
    }

    /// <summary>Runs the UI queue until <paramref name="done"/> holds.</summary>
    private static async Task Eventually(string what, ManualDispatcher ui, Func<bool> done)
    {
        var start = DateTime.UtcNow;
        while (true)
        {
            ui.Drain();
            if (done())
            {
                return;
            }
            Check(DateTime.UtcNow - start < Wait, what);
            await Task.Delay(10);
        }
    }

    private static async Task Eventually(string what, Func<bool> done) => await Eventually(what, new ManualDispatcher(), done);

    private static Task<CoreHost> SignedIn(StandIn standIn) => SignIn(CoreHost.Create(), standIn);

    private static async Task<CoreHost> SignIn(CoreHost core, StandIn standIn)
    {
        using var add = core.AddServer();
        await add.ProbeAsync(standIn.Address);
        Check(add.State is AddServerState.Ready, $"ready, not {add.State}");
        using var flow = add.SignIn();
        await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        Check(flow.State is SignInState.SignedIn, $"signed in, not {flow.State}");
        return core;
    }

    private static Library BigLibrary() =>
        new(new ItemRef(FirstServer, StandIn.BigLibraryId), "Example Big Library", "movies", []);

    private static string? Name(Slot slot) => slot is Slot.Item item ? item.Summary.Name : null;

    private static ImageRequest Poster(string itemId, int width = 320, int height = 480) => new(
        new ImageSource.Server(new ImageRef(new ItemRef(FirstServer, itemId), "primary", 0, StandIn.FirstFilmPosterTag)),
        width,
        height);

    public static async Task PagedGridLoadsTheVisibleRange()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        Check(browse.Server == FirstServer, "the browse's server");
        var first = ItemQuery.ForLibrary(BigLibrary());
        Check(first.Kinds.SequenceEqual(["movie"]) && first.Recursive && first.Sort.SequenceEqual(["sort_name"]),
            "a film library's first query lists its films by name");
        Check(ItemQuery.SortChoices[0] == new SortChoice("sort_name", SortOrder.Ascending), "the first sort choice is the first query's");
        Check(ItemQuery.FilterChoices[0] is null && ItemQuery.FilterChoices.Contains("unplayed") && !ItemQuery.FilterChoices.Contains("favourite")
            && ItemQuery.ToggleFilterChoices.SequenceEqual(["favourite"]),
            "the filter choices: everything first; favourites a tick beside them");
        var both = first.WithToggle("favourite", true).WithFilter("unplayed");
        Check(both.Filters.SequenceEqual(["unplayed", "favourite"]) && both.WithFilter(null).Filters.SequenceEqual(["favourite"]),
            "the favourites tick stays when the choice changes");
        Check(ItemQuery.OfKind("movie").Filters.Count == 0 && ItemQuery.Favourites("movie").Filters.SequenceEqual(["favourite"]),
            "a kind's page, and with the favourites tick on");
        var ui = new ManualDispatcher();
        using var items = browse.OpenLibrary(BigLibrary());
        using var grid = items.Collect(ui);

        await Eventually("the first page", ui, () => grid.Items.Count == StandIn.BigLibrarySize && grid.Items[0] is Slot.Item);
        Check(Name(grid.Items[0]) == StandIn.BigItemName(0), "the first item's name");
        Check(grid.Items[StandIn.BigLibrarySize - 1] is Slot.Loading, "a placeholder for a slot not loaded");
        Check(grid.Items.All(slot => slot is not null), "no null placeholder");
        Check(items.State.Total == StandIn.BigLibrarySize, "the total");

        grid.SetVisibleRange(440, 449);
        await Eventually("the last page", ui, () => grid.Items[449] is Slot.Item);
        Check(Name(grid.Items[449]) == StandIn.BigItemName(449), "the last item's name");
        Check(grid.Subscription.Recoveries == 0, "no recovery");

        // Left and shown again at once: not stale, so no refresh (the core's five-minute rule).
        items.Hidden();
        Check(!items.Shown(), "a grid shown again at once does not refresh");
    }

    /// <summary>A tile view model per slot, as a page binds; placeholders map to placeholder tiles, never null.</summary>
    private sealed record Tile(string? Name, bool Failed);

    public static async Task MappedGridGetsAnItemForEverySlot()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();
        using var items = browse.OpenLibrary(BigLibrary());
        using var grid = items.Collect(ui, slot => new Tile(Name(slot), slot is Slot.Failed));
        await Eventually("the first page", ui, () => grid.Items.Count == StandIn.BigLibrarySize && grid.Items[0].Name is not null);
        Check(grid.Items[200].Name is null && !grid.Items[200].Failed, "a loading tile");

        // A map that returns null fails its batches: the mirror recovers, then stops.
        using var nulls = items.Collect<Tile>(ui, _ => null!);
        await Eventually("the null map faults", ui, () => nulls.Subscription.IsFaulted);
        Check(nulls.Items.Count == 0, "nothing null reached the collection");
    }

    public static async Task FailedPageAndRetry()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        standIn.Send($"fail {StandIn.PageKey(400)}");
        var ui = new ManualDispatcher();
        using var items = browse.OpenLibrary(BigLibrary());
        using var grid = items.Collect(ui);
        using var state = items.ObserveState(ui);
        await Eventually("the first page", ui, () => grid.Items.Count == StandIn.BigLibrarySize);

        grid.SetVisibleRange(445, 449);
        await Eventually("the failed page", ui, () => grid.Items[449] is Slot.Failed);
        await Eventually("the failure in the state", ui, () => state.Value.Failed is BrowseProblem.Network);

        standIn.Send($"unfail {StandIn.PageKey(400)}");
        items.Retry();
        await Eventually("the retried page", ui, () => grid.Items[449] is Slot.Item);
        await Eventually("the state clears", ui, () => state.Value.Failed is null);
    }

    public static async Task ImageCacheSizeAndClear()
    {
        var folder = Directory.CreateTempSubdirectory("current-tests-").FullName;
        try
        {
            using var standIn = StandIn.Start();
            using var core = await SignIn(
                await CoreHost.OpenAsync(Path.Combine(folder, "Data"), Path.Combine(folder, "Cache"), "", SecretStore.Memory),
                standIn);
            Check(await core.ImageCacheSizeAsync() == 0, "an empty cache");

            var bytes = await core.LoadImageAsync(Poster(StandIn.FilmIds[0]));
            // The disk write is not awaited by the load.
            await Eventually("the image is on disk", () => core.ImageCacheSizeAsync().Result == bytes.Length);

            await core.ClearImageCacheAsync();
            Check(await core.ImageCacheSizeAsync() == 0, "cleared");
            var path = $"/Items/{StandIn.FilmIds[0]}/Images/Primary";
            var before = standIn.Count(path);
            var again = await core.LoadImageAsync(Poster(StandIn.FilmIds[0]));
            Check(again.AsSpan().SequenceEqual(bytes), "the same image after a clear");
            Check(standIn.Count(path) == before + 1, "fetched from the server again");
            await Eventually("stored again", () => core.ImageCacheSizeAsync().Result == bytes.Length);

            await core.SignOutAsync(FirstServer);
            Check(await core.ImageCacheSizeAsync() == 0, "a sign-out deletes the server's images");
        }
        finally
        {
            try
            {
                Directory.Delete(folder, recursive: true);
            }
            catch (Exception)
            {
                // A file still open: the system cleans its temp folder later.
            }
        }

        using var memoryOnly = CoreHost.Create();
        Check(await memoryOnly.ImageCacheSizeAsync() == 0, "no disk cache without a cache folder");
    }

    public static async Task ImageBytesAndCancellation()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);

        var bytes = await core.LoadImageAsync(Poster(StandIn.FilmIds[0]));
        Check(bytes.AsSpan().StartsWith(StandIn.PngSignature), "the image's bytes");

        // Cancelled while the server holds the answer: the call ends cancelled and the fetch stops.
        var path = $"/Items/{StandIn.FilmIds[1]}/Images/Primary";
        standIn.Send($"hold {path}");
        using var cts = new CancellationTokenSource();
        var load = core.LoadImageAsync(Poster(StandIn.FilmIds[1]), cts.Token);
        await Eventually("the image request arrives", () => standIn.Count(path) == 1);
        cts.Cancel();
        try
        {
            await load;
            throw new Exception("expected the load to be cancelled");
        }
        catch (OperationCanceledException)
        {
        }
        await Eventually("the fetch stops", () => standIn.Cancelled(path) == 1);
        standIn.Send($"release {path}");

        // A box without area is refused without a request.
        try
        {
            await core.LoadImageAsync(Poster(StandIn.FilmIds[2], width: 0));
            throw new Exception("expected an empty box to fail");
        }
        catch (ImageLoadException error)
        {
            Check(error.Problem is ImageProblem.EmptyBox, $"EmptyBox, not {error.Problem}");
        }
        Check(standIn.Count($"/Items/{StandIn.FilmIds[2]}/Images/Primary") == 0, "no request for the empty box");

        var placeholder = await core.LoadImageAsync(new ImageRequest(new ImageSource.BlurHash("LEHV6nWB2yk8pyo0adR*.7kCMdnj"), 320, 480));
        Check(placeholder.Length > 2 && placeholder[0] == (byte)'B' && placeholder[1] == (byte)'M', "a BMP placeholder");
    }

    private static bool? Favourite(IEnumerable<Slot> slots, ItemRef film) =>
        slots.OfType<Slot.Item>().FirstOrDefault(slot => slot.Summary.Item == film)?.Summary.User.Favourite;

    public static async Task RefusedToggleRollsBack()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var libraries = await browse.LibrariesAsync();
        var films = libraries.Single(library => library.Item.Id == StandIn.FilmsLibraryId);
        Check(films.Name == StandIn.FilmsLibraryName, "the library's name");
        var film = new ItemRef(FirstServer, StandIn.FilmIds[0]);

        var ui = new ManualDispatcher();
        using var items = browse.OpenLibrary(films);
        using var grid = items.Collect(ui);
        using var notices = core.ObserveBrowseNotices(ui);
        await Eventually("the films", ui, () => Favourite(grid.Items, film) == false);

        var path = $"/UserFavoriteItems/{StandIn.FilmIds[0]}";
        standIn.Send($"hold {path}");
        browse.SetFavourite(film, true);
        await Eventually("the change shown at once", ui, () => Favourite(grid.Items, film) == true);
        Check(browse.GetUserState(film)?.Favourite == true, "the browse's state");
        await Eventually("the request arrives", () => standIn.Count(path) == 1);
        standIn.Send($"fail {path}");
        standIn.Send($"release {path}");
        await Eventually("the rollback", ui, () => Favourite(grid.Items, film) == false);
        await Eventually("the notice", ui, () => notices.Value is not null);

        var notice = notices.Value as BrowseNotice.ActionFailed ?? throw new Exception($"ActionFailed, not {notices.Value}");
        Check(notice.Action == new ItemAction.SetFavourite(true), $"the action, not {notice.Action}");
        Check(notice.ItemName == StandIn.FirstFilmName, "the item's name, for the message");
        Check(notice.Problem is BrowseProblem.Network, $"a network problem, not {notice.Problem}");
        var text = notice.ToString();
        Check(!text.Contains("Example Film", StringComparison.Ordinal), $"no name in {text}");
        Check(text.Contains("SetFavourite", StringComparison.Ordinal), $"the action in {text}");
        Check(!core.DiagnosticsText().Contains("Example Film", StringComparison.Ordinal), "no name in the request log");
    }

    public static Task LibrarySearchTurnsOffSortAndFilter()
    {
        // What the core decides for a library's search box; no server needed.
        var browsing = ItemQuery.ForLibrary(BigLibrary()) with { Filters = ["unplayed"] };
        var tools = browsing.Tools();
        Check(tools.SortAvailable && tools.FilterAvailable && tools.SortTooltip is null, "Sort and Filter while browsing");
        Check(browsing.SearchNote(1000) is null, "no note while browsing");

        var searching = browsing.Searching("  example ");
        Check(searching.SearchTerm == "example" && searching.Sort.Count == 0 && searching.Filters.Count == 0,
            "a search: the text trimmed, no sort and no filter");
        tools = searching.Tools();
        Check(!tools.SortAvailable && !tools.FilterAvailable, "neither Sort nor Filter while searching");
        Check(tools.SortTooltip == "Sorted by relevance while searching", $"the Sort tooltip, not {tools.SortTooltip}");
        Check(searching.SearchNote(299) is null, "no note below the search window");
        Check(searching.SearchNote(300) == "Showing the best 300 matches. Type more to narrow it.", "the note at the window");

        var cleared = browsing.Searching("  ");
        Check(cleared.SearchTerm is null && cleared.Filters.SequenceEqual(["unplayed"]) && cleared.Sort.SequenceEqual(browsing.Sort),
            "blank text: the query before searching, its sort and filter back");
        return Task.CompletedTask;
    }

    public static Task ViewsAndRowsAreTheCores()
    {
        // What the core decides for the views, Home's rows, the search page's rows, names and times; no server needed.
        Check(Views.Query(BrowseView.Favourites).Filters.SequenceEqual(["favourite"]), "the Favourites tab's query");
        var recent = Views.Query(BrowseView.RecentlyAdded);
        Check(recent.Sort.SequenceEqual(["date_created"]) && recent.Order == SortOrder.Descending && recent.Recursive,
            "Recently added: the newest first, in every library");
        Check(Views.Query(BrowseView.RecentlyAdded, "series").Kinds.SequenceEqual(["series"]), "Recently added narrowed to a type");
        var ticked = Views.Opening(BrowseView.Everything, "episode", favourite: true);
        Check(ticked.Query.Kinds.SequenceEqual(["episode"]) && ticked.Query.Filters.SequenceEqual(["favourite"]) && !ticked.Wide
            && ticked.ShowTitles, "a Favourites row's header: its kind's page, favourites ticked, posters with titles");
        Check(Views.Layout(BrowseView.Favourites) == ViewLayout.Rows && Views.Layout(BrowseView.Favourites, "movie") == ViewLayout.Grid
            && Views.Layout(BrowseView.RecentlyAdded) == ViewLayout.Grid, "only the Favourites tab is rows");
        var tv = BigLibrary() with { Item = new ItemRef(FirstServer, "tv"), Collection = "tv_shows" };
        var music = BigLibrary() with { Item = new ItemRef(FirstServer, "music"), Collection = "music" };
        Check(Views.KindTabs([tv, music, BigLibrary()]).SequenceEqual(["movie", "series"]) && Views.KindTabs([music]).Count == 0
            && Views.KindTabs([BigLibrary() with { Collection = null }]).Count == 0, "a kind tab shows while a library of its type is listed");
        Check(Views.Remembers(BrowseView.Everything, "movie") && Views.Remembers(BrowseView.Everything, "series")
            && !Views.Remembers(BrowseView.Everything, "episode") && !Views.Remembers(BrowseView.RecentlyAdded),
            "only the kind tabs' pages remember");
        var menu = Views.FilterMenu(BrowseView.RecentlyAdded);
        Check(menu.Kinds.SequenceEqual([null, "movie", "series"]) && menu.Filters, "Recently added narrows by type, and filters");
        Check(Views.FilterMenu(null).Kinds.Count == 0 && Views.FilterMenu(BrowseView.Everything, "movie").Kinds.Count == 0
            && Views.FilterMenu(null).Filters, "a library and a kind's page are one type already");
        Check(Views.Title(BrowseView.Everything, "series") == "Series" && Views.Title(BrowseView.RecentlyAdded) == "Recently added",
            "the views' titles");
        Check(Views.LinkTooltip(BrowseView.Everything, "movie") == "Open Movies, favourites ticked", "a kind's link tooltip");

        static RowDisplay Row(HomeRowKind kind, string title = "", ItemRef? library = null) =>
            Views.ForHomeRow(new HomeRow("row", kind, title, library));
        Check(Row(HomeRowKind.ContinueWatching) is { Title: "Continue watching", Wide: true, Link: null }, "Continue watching");
        Check(Row(HomeRowKind.NextUp) is { Title: "Next up", Wide: true, Link: null }, "Next up");
        Check(Row(HomeRowKind.SeenRecently) is { Title: "Seen recently", Wide: false, Link: null }, "Seen recently");
        Check(Row(HomeRowKind.RecentlyAdded) is { Title: "Recently added", Wide: false, Link.Tooltip: "Open Recently added" }
            && Row(HomeRowKind.RecentlyAdded).Link?.Target == new RowTarget.OpenView(BrowseView.RecentlyAdded, null, false),
            "Recently added opens its tab");
        Check(Row(HomeRowKind.Favourites) is { Title: "Favourites", Link.Tooltip: "Open Favourites" }
            && Row(HomeRowKind.Favourites).Link?.Target == new RowTarget.OpenView(BrowseView.Favourites, null, false),
            "Favourites opens its tab");
        var films = new ItemRef(FirstServer, StandIn.FilmsLibraryId);
        var latest = Row(HomeRowKind.Latest, "Films", films);
        Check(latest is { Title: "Recently added in Films", Link.Tooltip: "Open Films, newest first" }
            && latest.Link?.Target == new RowTarget.OpenLibrary(films), "a Latest row opens its library, newest first");

        var group = Views.ForSearchGroup("episode", BrowseView.Everything);
        Check(group is { Title: "Episodes", Wide: true }
            && group.Link?.Target == new RowTarget.OpenView(BrowseView.Everything, "episode", true), "a Favourites row of episodes");
        Check(Views.ForSearchGroup("movie") is { Title: "Movies", Wide: false, Link: null }, "a search group");

        Check(BrowseLabels.KindGroup("live_tv") == "Live Tv", "a kind Current has no name for, title-cased");
        Check(BrowseLabels.KindChoice(null) == "All types" && BrowseLabels.SortName("date_created") == "Date added"
            && BrowseLabels.FilterName(null) == "All" && BrowseLabels.FilterName("favourite") == "Favourites", "the menus' names");
        Check(BrowseLabels.TimeLeftShort(TimeSpan.FromMinutes(72)) == "1h 12m"
            && BrowseLabels.TimeLeftShort(TimeSpan.FromSeconds(20)) == "1m",
            "the time left on a tile");
        Check(BrowseLabels.TimeLeft(TimeSpan.FromMinutes(45)) == "45 min left"
            && BrowseLabels.TimeLeft(TimeSpan.FromSeconds(20)) == "1 min left",
            "the time left in words");
        Check(BrowseLabels.Runtime(TimeSpan.FromMinutes(90.5)) == "1 h 30 min", "a half minute rounds to the even one");
        Check(BrowseLabels.FavouritesStatus(0, false, false) == "No favourites yet"
            && BrowseLabels.FavouritesStatus(0, false, true) == "Couldn't load favourites."
            && BrowseLabels.FavouritesStatus(1, false, true) is null && BrowseLabels.FavouritesStatus(0, true, false) is null,
            "the Favourites tab's line");
        return Task.CompletedTask;
    }

    public static async Task ViewGridsOpenTheirItems()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();

        using var recent = browse.OpenItems(Views.Opening(BrowseView.RecentlyAdded).Query);
        using var recentGrid = recent.Collect(ui);
        await Eventually("Recently added's items", ui, () => recentGrid.Items.Count > 0 && recentGrid.Items[0] is Slot.Item);
        Check(recentGrid.Items.OfType<Slot.Item>().All(slot => slot.Summary.Kind is "movie" or "series"), "films and series");

        using var favourites = browse.OpenItems(Views.Query(BrowseView.Everything, "movie", favourite: true));
        using var favouritesGrid = favourites.Collect(ui);
        await Eventually("a kind's page", ui, () => favouritesGrid.Items.Count > 0 && favouritesGrid.Items[0] is Slot.Item);
        Check(favouritesGrid.Items.OfType<Slot.Item>().All(slot => slot.Summary.Kind == "movie"), "films only");

        using var searched = browse.OpenItems(Views.Query(BrowseView.RecentlyAdded).Searching(StandIn.FirstFilmName));
        using var searchedGrid = searched.Collect(ui);
        await Eventually("the search's items", ui,
            () => searchedGrid.Items.OfType<Slot.Item>().Any(slot => slot.Summary.Name == StandIn.FirstFilmName));

        // What a library opens with: nothing remembered yet, then what is, unless a Home row's header opened it.
        var first = core.LibraryOpening(BigLibrary());
        Check(first.Query.Sort.SequenceEqual(["sort_name"]) && !first.Wide && first.ShowTitles, "the first query, posters with titles");
        await core.SetRememberSortAsync(true);
        await core.SetRememberFilterAsync(true);
        await core.SetRememberViewAsync(true);
        await core.RememberLibraryChoicesAsync(BigLibrary().Item, new LibraryChoices(
            new SortChoice("community_rating", SortOrder.Descending), ["unplayed"], new LibraryView(LibraryView.Wide, false)));
        var remembered = core.LibraryOpening(BigLibrary());
        Check(remembered.Query.Sort.SequenceEqual(["community_rating"]) && remembered.Query.Order == SortOrder.Descending
            && remembered.Query.Filters.SequenceEqual(["unplayed"]) && remembered.Wide && !remembered.ShowTitles, "what is remembered");
        await core.RememberLibraryChoicesAsync(BigLibrary().Item, new LibraryChoices(Filters: ["unplayed", "resumable"]));
        Check(core.LibraryOpening(BigLibrary()).Query.Filters.Count == 0, "filters the menu no longer offers are dropped");
        var home = core.LibraryOpening(BigLibrary(), ItemQuery.RecentlyAdded(BigLibrary()));
        Check(home.Query.Sort.SequenceEqual(["date_created"]) && !home.Wide && home.ShowTitles, "a Home header ignores what is remembered");

        // A kind tab's page remembers its own, as a library; a Favourites row's header opens it with the favourites ticked.
        var tab = core.ViewOpening(FirstServer, BrowseView.Everything, "movie");
        Check(tab.Query.Kinds.SequenceEqual(["movie"]) && tab.Query.Sort.SequenceEqual(["sort_name"]) && !tab.Wide,
            "the Movies tab, nothing remembered yet");
        await core.RememberViewChoicesAsync(FirstServer, BrowseView.Everything, "movie", new LibraryChoices(
            new SortChoice("date_created", SortOrder.Descending), ["played"], new LibraryView(LibraryView.Wide, true)));
        var tabRemembered = core.ViewOpening(FirstServer, BrowseView.Everything, "movie");
        Check(tabRemembered.Query.Sort.SequenceEqual(["date_created"]) && tabRemembered.Query.Filters.SequenceEqual(["played"])
            && tabRemembered.Wide, "the Movies tab opens with what is remembered");
        Check(core.ViewOpening(FirstServer, BrowseView.Everything, "series").Query.Sort.SequenceEqual(["sort_name"]),
            "the Series tab remembers its own");
        var fromHeader = core.ViewOpening(FirstServer, BrowseView.Everything, "movie", favourite: true);
        Check(fromHeader.Query.Filters.SequenceEqual(["favourite"]) && !fromHeader.Wide, "a Favourites header ignores what is remembered");
        using var movies = browse.OpenItems(tabRemembered.Query with { Filters = [] });
        using var moviesGrid = movies.Collect(ui);
        await Eventually("the Movies tab's items", ui, () => moviesGrid.Items.Count > 0 && moviesGrid.Items[0] is Slot.Item);
        Check(moviesGrid.Items.OfType<Slot.Item>().All(slot => slot.Summary.Kind == "movie"), "the Movies tab lists films only");
    }

    public static async Task SearchFiltersNarrowAndAPersonOpensTheirTitles()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();

        // The core's choices: the catalog is listed, unavailable without a Seerr server, its reason in its name.
        var tools = core.SearchToolsFor(new SearchFilter("person", SearchSource.Catalog));
        Check(tools.Types.Select(type => type.Choice.Label).SequenceEqual(["All", "Movies", "Series", "People"]),
            $"the types, not {string.Join(",", tools.Types.Select(type => type.Choice.Label))}");
        Check(tools.Types[3].Kind == "person" && tools.Types[3].Choice.Chosen, "People chosen");
        var catalog = tools.Sources[2];
        Check(catalog.Source == SearchSource.Catalog && !catalog.Choice.Enabled && !catalog.Choice.Chosen,
            "the catalog is listed but unavailable");
        Check(catalog.Choice.Label == "Catalog (needs Seerr)" && catalog.Choice.Tooltip == "Needs a Seerr server",
            $"the catalog's name and tooltip, not {catalog.Choice.Label} / {catalog.Choice.Tooltip}");
        Check(tools.Sources[0].Choice.Chosen, "without the catalog, every source is in effect");
        var peopleRow = Views.ForSearchResult("person", new SearchFilter("person"), wide: true, showTitles: true);
        Check(peopleRow is { Title: "People", Cards: true, Wraps: true, Opens: ItemOpens.PersonTitles, ItemMenu: false, Note.FullAt: 100 },
            $"a People group: cast cards that open their titles, no item menu, wrapping, full at 100 ({peopleRow})");
        Check(Views.ForSearchResult("movie", new SearchFilter("episode"), wide: false, showTitles: false)
            is { Wide: false, ShowTitles: false, Wraps: false, Cards: false, ItemMenu: true, Note.FullAt: 24 },
            "a type the search does not offer runs as every type");

        using var search = browse.Search("example", new SearchFilter("person", SearchSource.Library));
        using var groups = search.ObserveGroups(ui);
        await Eventually("the people", ui, () => groups.Items.Count == 1);
        Check(groups.Items.SequenceEqual(["person"]), $"people only, not {string.Join(",", groups.Items)}");
        using var people = search.ObserveGroup("person", ui);
        Check(people is not null, "the people's list");
        await Eventually("both people", ui, () => people!.Items.Count == 2);
        Check(people!.Items[0].Item.Id == StandIn.PersonId && people.Items[0].Kind == "person", "the actor first");

        var titles = ItemQuery.ForPerson(new ItemRef(FirstServer, StandIn.PersonId));
        Check(titles.Person?.Id == StandIn.PersonId && titles.Kinds.SequenceEqual(["movie", "series"]), "a person's films and series");
        using var items = browse.OpenItems(titles);
        using var grid = items.Collect(ui);
        await Eventually("the person's titles", ui, () => grid.Items.Count == 2 && grid.Items.All(slot => slot is Slot.Item));
        Check(grid.Items.Any(slot => slot is Slot.Item { Summary.Item.Id: StandIn.SeriesId }), "the series among them");
    }

    public static async Task FavouritesAreARowPerKind()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();
        using var favourites = browse.Favourites();
        using var groups = favourites.ObserveGroups(ui);
        // The stand-in's item list takes no filter: every film, series and episode counts as a favourite here.
        await Eventually("the rows", ui, () => groups.Items.Count == 3);
        Check(groups.Items.SequenceEqual(["movie", "series", "episode"]), $"films, series, episodes, not {string.Join(",", groups.Items)}");
        Check(ItemQuery.Favourites("episode").Kinds.SequenceEqual(["episode"]), "an Episodes row's header opens the episodes");
    }

    public static async Task HomeRowsHaveTheirOwnLists()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();
        using var home = browse.Home();
        using var rows = home.ObserveRows(ui);
        var latest = $"latest:{StandIn.FilmsLibraryId}";
        await Eventually("the Latest row", ui, () => rows.Items.Any(row => row.Key == latest));

        var row = rows.Items.Single(row => row.Key == latest);
        Check(row.Kind == HomeRowKind.Latest, "a Latest row");
        Check(row.Title == StandIn.FilmsLibraryName, "the library's name as its title");
        Check(row.Library?.Id == StandIn.FilmsLibraryId, "its library");
        Check(!row.ToString().Contains(StandIn.FilmsLibraryName, StringComparison.Ordinal), $"no title in {row}");

        using var films = home.ObserveRow(latest, ui) ?? throw new Exception("the row's list");
        await Eventually("the row's items", ui, () => films.Items.Count > 0);
        Check(films.Items.All(film => film.Kind == "movie"), "films in the films row");
        Check(home.ObserveRow("no-such-row", ui) is null, "no list for a row that does not exist");
        await Eventually("Home loaded", ui, () => !home.State.Loading);
    }

    public static async Task NoBrowseWithoutASignedInServer()
    {
        using (var empty = CoreHost.Create())
        {
            try
            {
                empty.Browse(FirstServer).Dispose();
                throw new Exception("expected no browse without a saved server");
            }
            catch (UnknownServerException)
            {
            }
        }

        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        await core.SignOutAsync(FirstServer);
        try
        {
            core.Browse(FirstServer).Dispose();
            throw new Exception("expected no browse after signing out");
        }
        catch (NotSignedInException)
        {
        }
        try
        {
            await browse.LibrariesAsync();
            throw new Exception("expected the old browse to fail");
        }
        catch (BrowseRequestException error)
        {
            Check(error.Problem is BrowseProblem.NotSignedIn, $"NotSignedIn, not {error.Problem}");
        }
    }

    public static async Task PlayStartsTheItemFromWhereTheCoreSays()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();
        using var details = browse.Details(new ItemRef(FirstServer, StandIn.FilmIds[0]));
        using var play = details.ObservePlay(ui);
        await Eventually("the film's Play", ui, () => play.Value is not null);
        var target = play.Value!;
        Check(target.Item.Item == new ItemRef(FirstServer, StandIn.FilmIds[0]) && !target.Resumes && target.Start == TimeSpan.Zero,
            "a film without a resume point plays from its start");

        // A tile's item: the round trip through the core keeps it (records compare their lists by reference, so by its fields), and
        // the rule is the core's.
        var started = target.Item with { User = target.Item.User with { Position = TimeSpan.FromSeconds(90) } };
        var tile = PlayTarget.ForItem(started);
        Check(tile is { Resumes: true } && tile.Start == TimeSpan.FromSeconds(90), "a tile's item with a resume point resumes");
        Check(tile is not null && tile.Item.Item == started.Item && tile.Item.User == started.User
            && tile.Item.Artwork.SequenceEqual(started.Artwork) && tile.Item.Poster == started.Poster, "the item crosses back unchanged");
        Check(PlayTarget.ForItem(started with { User = started.User with { Played = true } })?.Start == TimeSpan.Zero,
            "played, it starts again");
        Check(PlayTarget.ForItem(started with { Kind = "series" }) is null, "a series tile has no Play of its own");
    }

    public static async Task BrowseSubscriptionsAreReleased()
    {
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        var before = core.LiveListeners;
        var ui = new ManualDispatcher();
        using (var browse = core.Browse(FirstServer))
        using (var items = browse.OpenLibrary(BigLibrary()))
        using (var grid = items.Collect(ui))
        using (var state = items.ObserveState(ui))
        using (var home = browse.Home())
        using (var rows = home.ObserveRows(ui))
        using (var details = browse.Details(new ItemRef(FirstServer, StandIn.FilmIds[0])))
        using (var similar = details.ObserveSimilar(ui))
        using (var search = browse.Search("Example"))
        using (var groups = search.ObserveGroups(ui))
        using (var notices = core.ObserveBrowseNotices(ui))
        {
            Check(core.LiveListeners == before + 6, $"six listeners, not {core.LiveListeners - before}");
        }
        await Eventually("every listener is released", ui, () => core.LiveListeners == before);
    }

    private static string[] Ids(IReadOnlyList<Library>? libraries) => [.. (libraries ?? []).Select(library => library.Item.Id)];

    public static async Task LibrariesFollowTheServer()
    {
        const string added = "57a2d1e0000000000000000000001003";
        using var standIn = StandIn.Start();
        using var core = await SignedIn(standIn);
        using var browse = core.Browse(FirstServer);
        var ui = new ManualDispatcher();
        using var libraries = browse.ObserveLibraries(ui);
        await Eventually("the libraries load at once", ui,
            () => Ids(libraries.Value).SequenceEqual([StandIn.FilmsLibraryId, StandIn.ShowsLibraryId]));
        var films = libraries.Value![0];

        // Added on the server, then renamed and moved first: the list follows on a refresh, its unknown kind passed through.
        standIn.Send($"library add {added} examplekind Example Added");
        browse.Refresh();
        await Eventually("the added library", ui,
            () => Ids(libraries.Value).SequenceEqual([StandIn.FilmsLibraryId, StandIn.ShowsLibraryId, added]));
        Check(libraries.Value![2].Collection == "examplekind", "an unknown kind passes through");
        standIn.Send($"library rename {added} Example Renamed");
        standIn.Send($"library order {added},{StandIn.FilmsLibraryId},{StandIn.ShowsLibraryId}");
        core.ReturnedToForeground();
        await Eventually("renamed and moved", ui,
            () => libraries.Value?.FirstOrDefault() is { Name: "Example Renamed" } first && first.Item.Id == added);

        // The films library removed; its stale entry opened: "not available", the list drops it, and the user stays signed in.
        standIn.Send($"library remove {StandIn.FilmsLibraryId}");
        using var items = browse.OpenLibrary(films);
        using var state = items.ObserveState(ui);
        await Eventually("not available", ui, () => state.Value.Failed is BrowseProblem.NotAvailable);
        await Eventually("the list drops it", ui,
            () => Ids(libraries.Value).SequenceEqual([added, StandIn.ShowsLibraryId]));
        // A Browse is only given for a server signed in with a live session.
        using var again = core.Browse(FirstServer);

        // A grid freed while its page still holds its collection (the library went): the collection sends nothing more.
        using var collection = items.Collect(ui);
        items.Dispose();
        collection.SetVisibleRange(0, 10);
        items.Dispose();
    }
}
