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
        Check(ItemQuery.FilterChoices[0] is null && ItemQuery.FilterChoices.Contains("favourite"), "the filter choices");
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
