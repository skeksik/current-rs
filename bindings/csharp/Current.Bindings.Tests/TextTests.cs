using Current.Bindings;

/// <summary>
/// The core's words through the public adapters: the problems in plain words (static and the platform's), the browse screens' tile
/// lines, names, notices, sizes, counts and facts, and the search page's status line. Each test has a core that stores nothing.
/// </summary>
internal static class TextTests
{
    private static readonly ServerRef FirstServer = new("jellyfin", "1");

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"expected: {what}");
        }
    }

    public static Task ProblemTextsAreTheCores()
    {
        using var core = CoreHost.Create();
        core.SetPlatform(Platform.Windows);
        Check(ProblemTexts.Unexpected == "Something went wrong in Current. Try again.", "the unexpected problem");
        Check(ProblemTexts.GatewayUnsupported == "The server asks for a sign-in method Current does not support.",
            "an unsupported gateway");
        Check(ProblemTexts.GatewayRefused.EndsWith("add it under Advanced.", StringComparison.Ordinal)
            && !ProblemTexts.GatewayRefused.Contains("  ", StringComparison.Ordinal), "a refusing gateway, one line");
        Check(ProblemTexts.Gateway(GatewayProblem.SignIn) == "This address shows a sign-in page before the server.", "a gateway");
        Check(ProblemTexts.GatewayInput(GatewayInputProblem.NoHeaders) == "Add at least one header.", "a gateway input");
        Check(ProblemTexts.Store(new StoreProblem.Io("save", "permission denied")) == "Step: save. Error: permission denied.",
            "a store problem");
        Check(core.ProblemText(new ProbeProblem.Tls())
            == "The secure connection failed. The server's certificate may not be trusted by this PC.", "this PC");
        Check(core.ProblemText(new ProbeProblem.InvalidAddress(AddressProblem.UnsupportedScheme))
            == "Use an address that starts with http:// or https://.", "an invalid address");
        Check(core.ProblemText(new ProbeProblem.Redirected(302)) == "The server sent Current to another address (HTTP 302).",
            "a redirect's status");
        Check(core.ProblemText(DiscoveryProblem.NotPermitted) == "Windows didn't allow Current to search the network.", "Windows");
        Check(core.ProblemText(new SignInProblem.Save(new StoreProblem.NewerVersion(3)))
            == "These settings are from a newer version of Current, so this sign-in can't be saved. Update Current.",
            "a sign-in that can't be saved");
        Check(core.ProblemText(new SignInProblem.Network(new NetProblem.Auth(403)))
            == "The server answered with an error (HTTP 403).", "a sign-in's network problem");
        Check(core.ProblemText(new GatewaySignInProblem.ServerRemoved()) == "This server was removed.", "a gateway sign-in");
        Check(core.ProblemText(new NetProblem.Unreachable()) == "Can't reach the server.", "a network problem");
        Check(core.ProblemText(new BrowseProblem.Gateway(GatewayProblem.Password))
            == "The gateway in front of the server wants you to sign in again. Sign in to the gateway in Settings.",
            "a browse problem");
        return Task.CompletedTask;
    }

    private static ItemSummary Item(string kind, string name, EpisodeLinks? episode = null, UserState? user = null,
        int? year = null, int? childCount = null) => new(
            new ItemRef(FirstServer, "57a2d1e0000000000000000000001101"),
            kind,
            name,
            year,
            null,
            [],
            user ?? new UserState(false, null, null, false, null, 0),
            episode,
            childCount);

    public static Task TileTextsAreTheCores()
    {
        var episode = Item("episode", "Pilot", new EpisodeLinks(null, "A Series", null, null, 2, null, 1, "S1:E2"),
            new UserState(false, null, 0.5f, true, null, 0));
        Check(BrowseLabels.TileTitle(episode) == "A Series" && BrowseLabels.TileSubtitle(episode) == "S1:E2 · Pilot",
            "an episode under its series");
        Check(BrowseLabels.TileTitle(episode, withSeries: false) == "Pilot"
            && BrowseLabels.TileSubtitle(episode, withSeries: false) == "S1:E2", "an episode on its series' page");
        Check(BrowseLabels.TileName(episode) == "A Series, S1:E2 · Pilot, in progress, favourite",
            $"a tile's name with its marks, not {BrowseLabels.TileName(episode)}");
        var series = Item("series", "A Series", user: new UserState(false, null, null, false, 3, 0), year: 2019, childCount: 4);
        Check(BrowseLabels.TileSubtitle(series) == "4 new episodes", "a series' new episodes");
        Check(BrowseLabels.TileName(series) == "A Series, 4 new episodes, 3 unplayed", "a series' unplayed count");
        Check(BrowseLabels.TileSubtitle(Item("movie", "A Film", year: 2019)) == "2019", "a film's year");
        Check(BrowseLabels.LoadingTileName == "Loading"
            && BrowseLabels.FailedTileName == "Couldn't load. Use Retry above the grid.", "the placeholders' names");
        Check(BrowseLabels.PersonKind("guest_star") == "Guest star" && BrowseLabels.PersonKind("actor") == "", "a person's part");
        var failed = new BrowseNotice.ActionFailed(1, new ItemAction.SetFavourite(false), "A Film", new BrowseProblem.Refused());
        Check(BrowseLabels.ActionFailedTitle(failed) == "Couldn't remove A Film from favourites.", "a failed mark");
        Check(BrowseLabels.ActionFailedTitle(failed with { ItemName = null, Action = new ItemAction.SetPlayed(true) })
            == "Couldn't mark the item as played.", "a failed mark without a name");
        Check(BrowseLabels.LibraryGoneTitle == "This library is no longer available"
            && BrowseLabels.LibraryGoneMessage == "It was removed from the server, or you no longer have access to it.",
            "the library-gone notice");
        return Task.CompletedTask;
    }

    public static Task NumbersAndStatusesAreTheCores()
    {
        using var core = CoreHost.Create();
        Check(core.SizeOnDisk(1536) == "1.5 KB" && core.SizeOnDisk(10 * 1024 * 1024 - 1) == "10 MB"
            && core.SizeOnDisk(512) == "512 bytes", "sizes on disk");
        Check(core.ItemCount(0) == "" && core.ItemCount(1) == "1 item" && core.ItemCount(12_345) == "12,345 items", "counts");
        core.SetDecimalSeparator(",");
        core.SetDigitGrouping(".", [3, 2]);
        Check(core.SizeOnDisk(1536) == "1,5 KB", "the platform's decimal separator");
        Check(core.ItemCount(1_234_567) == "12.34.567 items", $"the platform's grouping, not {core.ItemCount(1_234_567)}");
        var film = Item("movie", "A Film", year: 2019) with { Runtime = TimeSpan.FromMinutes(90) };
        var details = new ItemDetails(film, null, null, [], [], [], [], "PG-13", 7.25f, null, null, null, null, [], null, [], null);
        Check(core.DetailsFacts(details) == "2019 · 1 h 30 min · PG-13 · 7,3/10", $"the facts line, not {core.DetailsFacts(details)}");
        Check(core.DetailsFacts(details with { Run = new SeriesRun(2019, 2023, false), CommunityRating = null, OfficialRating = null })
            == "2019–2023 · 1 h 30 min", "a series' run");

        core.SetPlatform(Platform.Windows);
        var timeout = new BrowseProblem.Network(new NetProblem.Timeout());
        Check(core.SearchStatus(0, true, null, favourites: false) == "Searching…", "searching");
        Check(core.SearchStatus(3, true, null, favourites: false) == "", "results");
        Check(core.SearchStatus(0, false, null, favourites: false) == "No results", "no results");
        Check(core.SearchStatus(0, false, timeout, favourites: false) == "Couldn't search. The server took too long to answer.",
            "a failed search");
        Check(core.SearchStatus(0, false, null, favourites: true) == "No favourites yet", "no favourites");
        Check(core.SearchStatus(0, false, timeout, favourites: true)
            == "Couldn't load favourites. The server took too long to answer.", "failed favourites");
        return Task.CompletedTask;
    }
}
