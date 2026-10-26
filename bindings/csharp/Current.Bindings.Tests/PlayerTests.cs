using Current.Bindings;

/// <summary>
/// The player through <see cref="PlayerSession"/> (ADR 0019 task 4), on the core's test engine (the test build of the core library;
/// the app ships without it), against the Jellyfin stand-in: the state, the reports the stand-in receives, the swap chain and its
/// release, one engine per process, cancelling, playback behind a header gateway (through the core's loopback relay, ADR 0020), and
/// that no URL or secret is in a record.
/// </summary>
internal static class PlayerTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);
    private static readonly ServerRef FirstServer = new("jellyfin", "1");
    private static readonly CompositionSize Size = new(1280, 720);
    private static readonly DisplayFacts Display = new(Hdr: false, PeakNits: 300, SdrWhiteNits: 200, RefreshHz: 60);
    /// <summary>The test engine's fake swap chain (crates/app/src/player/test_engine.rs).</summary>
    private const ulong TestSwapChain = 0x5157_0001;

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"expected: {what}");
        }
    }

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

    private static async Task SignIn(CoreHost core, StandIn standIn, bool gateHeader = false)
    {
        using var add = core.AddServer();
        await add.ProbeAsync(standIn.Address);
        if (gateHeader)
        {
            await add.SubmitHeadersAsync([new GatewayHeader(StandIn.GateHeader, StandIn.GateHeaderValue)], confirmPlainHttp: false);
        }
        using var flow = add.SignIn();
        await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        Check(flow.State is SignInState.SignedIn, $"signed in, not {flow.State}");
        var ui = new ManualDispatcher();
        using var capabilities = core.ObserveCapabilities(ui);
        await Eventually("an address", ui, () => capabilities.Value.Servers.Count == 1 && capabilities.Value.Servers[0].AddressPosition is not null);
    }

    private static ItemRef Item(string id) => new(FirstServer, id);

    public static Task EngineIsTheTestEngine()
    {
        using var core = CoreHost.Create();
        Check(core.PlayerEngine() == PlayerEngineKind.Test, $"the test engine, not {core.PlayerEngine()}");
        return Task.CompletedTask;
    }

    public static async Task PlaysReportsAndReleases()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        var ui = new ManualDispatcher();
        var session = await PlayerSession.OpenAsync(core, Size, Display, ui);
        SwapChainRef? swapChain = null;
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await Eventually("playing", ui, () => session.State.Value.State == PlayerStateKind.Playing);
            var state = session.State.Value;
            Check(state.Item == Item(StandIn.DirectPlayItem), $"the item, not {state.Item}");
            Check(state.Duration == TimeSpan.FromMinutes(10), $"ten minutes, not {state.Duration}");
            Check(state.Route?.Kind == RouteKind.DirectPlay, $"direct play, not {state.Route}");
            Check(state.AudioTracks.Count == 1 && state.Audio == 1, "one audio track, playing");
            await Eventually("the start report", ui, () => standIn.Count("/Sessions/Playing") == 1);
            await Eventually("the media controls", ui, () => session.NowPlaying.Value.Title == StandIn.DirectPlayName
                && session.NowPlaying.Value.Status == MediaStatusKind.Playing);
            Check(session.NowPlaying.Value.CanPause && session.NowPlaying.Value.CanSeek, "pause and seek work");

            session.Pause();
            session.SeekTo(TimeSpan.FromSeconds(90));
            await Eventually("paused at 90 s", ui, () => session.State.Value.State == PlayerStateKind.Paused
                && session.State.Value.Position == TimeSpan.FromSeconds(90));
            await Eventually("a progress report", ui, () => standIn.Count("/Sessions/Playing/Progress") >= 1);

            await Eventually("a swap chain", ui, () => session.SwapChain.Value is not null);
            swapChain = session.SwapChain.Value!;
            Check(swapChain.Handle == TestSwapChain && swapChain.Generation == 1, $"the test engine's swap chain, not {swapChain}");
            Check(!swapChain.ToString().Contains(TestSwapChain.ToString(System.Globalization.CultureInfo.InvariantCulture),
                StringComparison.Ordinal), "no pointer in its text form");
            swapChain.Release();
            swapChain.Release();
            Check(swapChain.IsReleased, "released");

            session.Stop();
            await Eventually("idle", ui, () => session.State.Value.State == PlayerStateKind.Idle);
            await Eventually("the stop report", ui, () => standIn.Count("/Sessions/Playing/Stopped") == 1);
        }
        finally
        {
            await session.CloseAsync();
        }
        // Every call after closing fails with the documented exception, not the freed native object's.
        foreach (var (what, call) in new (string, Func<Task>)[]
        {
            ("Pause", () => { session.Pause(); return Task.CompletedTask; }),
            ("SeekTo", () => { session.SeekTo(TimeSpan.Zero); return Task.CompletedTask; }),
            ("PlayAsync", () => session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero)),
            ("SetQualityAsync", () => session.SetQualityAsync(new QualityChoice.Maximum())),
            ("PlaybackInfoAsync", () => session.PlaybackInfoAsync()),
        })
        {
            var closed = await ThrowsPlayer(call);
            Check(closed.Problem is PlayerProblem.Closed, $"{what} after closing: closed, not {closed.Problem}");
        }
        await session.CloseAsync();
        swapChain!.Dispose();
        swapChain.Release();
        Check(swapChain.IsReleased, "a disposed swap chain reads as released");
        await Eventually("listeners dropped", new ManualDispatcher(), () => core.LiveListeners == 0);
    }

    public static async Task SecondPlayerIsBusy()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        var first = await PlayerSession.OpenAsync(core, Size, Display, ui);
        try
        {
            var busy = await ThrowsPlayer(() => PlayerSession.OpenAsync(core, Size, Display, ui));
            Check(busy.Problem is PlayerProblem.Busy, $"busy, not {busy.Problem}");
        }
        finally
        {
            await first.CloseAsync();
        }
        // Closed: the next one opens.
        var next = await PlayerSession.OpenAsync(core, Size, Display, ui);
        await next.CloseAsync();
    }

    public static async Task CancelledPlayLeavesNothingStarted()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        var ui = new ManualDispatcher();
        var session = await PlayerSession.OpenAsync(core, Size, Display, ui);
        try
        {
            var path = $"/Items/{StandIn.DirectPlayItem}/PlaybackInfo";
            standIn.Send($"hold {path}");
            using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(300));
            try
            {
                await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero, cts.Token);
                Check(false, "the play is cancelled");
            }
            catch (OperationCanceledException error)
            {
                Check(error.CancellationToken == cts.Token, "the caller's token");
            }
            await Eventually("idle after the cancel", ui, () => session.State.Value.State == PlayerStateKind.Idle
                && session.State.Value.Item is null);
            standIn.Send($"release {path}");
            Check(standIn.Count("/Sessions/Playing") == 0, "nothing reported");
        }
        finally
        {
            await session.CloseAsync();
        }
    }

    public static async Task HlsBehindAHeaderGatewayIsRelayed()
    {
        using var standIn = StandIn.Start();
        standIn.Send("gate header");
        using var core = CoreHost.Create();
        await SignIn(core, standIn, gateHeader: true);
        var ui = new ManualDispatcher();
        var session = await PlayerSession.OpenAsync(core, Size, Display, ui);
        try
        {
            var seen = new List<string>();
            session.State.PropertyChanged += (_, _) => seen.Add(session.State.Value.ToString());
            session.NowPlaying.PropertyChanged += (_, _) => seen.Add(session.NowPlaying.Value.ToString());

            // Direct play behind the gateway plays (the engine gets a URL on the core's relay, the UI never sees it).
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await Eventually("playing", ui, () => session.State.Value.State == PlayerStateKind.Playing);
            seen.Add((await session.PlaybackInfoAsync()).ToString());

            // An HLS stream behind the gateway: the relay rewrites its playlists when the engine fetches them; the core fetches none first.
            await session.PlayAsync(Item(StandIn.TranscodeItem), TimeSpan.FromSeconds(30));
            await Eventually("the HLS stream playing", ui, () => session.State.Value.State == PlayerStateKind.Playing
                && session.State.Value.Item == Item(StandIn.TranscodeItem));
            Check(session.State.Value.Route?.Kind == RouteKind.Remux, $"a remux, not {session.State.Value.Route}");
            Check(standIn.Count($"/videos/{StandIn.TranscodeItem}/master.m3u8") == 0, "no playlist fetched before the engine asks");
            seen.Add((await session.PlaybackInfoAsync()).ToString());

            var host = standIn.Address.Replace("http://", "", StringComparison.Ordinal);
            var text = string.Join("\n", seen);
            foreach (var secret in new[] { StandIn.GateHeaderValue, host, "ApiKey", "api_key", "master.m3u8", "http" })
            {
                Check(!text.Contains(secret, StringComparison.OrdinalIgnoreCase), $"no '{secret}' in what crossed");
            }
            Check(!core.DiagnosticsText().Contains(StandIn.GateHeaderValue, StringComparison.Ordinal), "no header value in the log");
        }
        finally
        {
            await session.CloseAsync();
        }
    }

    public static async Task SkipSwitchIsSaved()
    {
        using var core = CoreHost.Create();
        Check(!core.PlaybackSettings().SkipsIntrosAndCredits && core.PlaybackSettings().AutoSkip.Count == 0, "off by default");
        Check(core.PlaybackSettings().ShowsQualityResolution, "resolutions shown by default");
        await core.SetShowQualityResolutionAsync(false);
        Check(!core.PlaybackSettings().ShowsQualityResolution, "resolutions hidden");
        await core.SetShowQualityResolutionAsync(true);
        Check(!core.PlaybackSettings().ShowsAllQualities, "common qualities by default");
        await core.SetShowAllQualitiesAsync(true);
        Check(core.PlaybackSettings().ShowsAllQualities, "all qualities");
        await core.SetShowAllQualitiesAsync(false);
        await core.SetSkipIntrosAndCreditsAsync(true);
        Check(core.PlaybackSettings().SkipsIntrosAndCredits, "on");
        Check(core.PlaybackSettings().AutoSkip.SequenceEqual(new[] { "intro", "outro" }), "intro and outro");
        await core.SetSkipIntrosAndCreditsAsync(false);
        Check(!core.PlaybackSettings().SkipsIntrosAndCredits, "off again");
        Check(core.QualitySteps()[0] == new QualityChoice.Bitrate(40_000_000), "the first step");
    }

    public static async Task SupportedFormatsCrossWithTheFactsPassedIn()
    {
        using var core = CoreHost.Create();
        var hardware = new HardwareDecodeFacts(
            "Example Graphics", [new DecoderProfileFact("{5B11D51B-2F4C-4452-BCC3-09F2A1160CC0}", Nv12: true, P010: false)]);
        var page = await core.SupportedFormatsAsync(hardware, new DisplayFacts(false, 300f, 200f, 60.0));
        Check(page.Tabs.Select(tab => tab.Kind).SequenceEqual(
            [FormatTabKind.Video, FormatTabKind.Audio, FormatTabKind.Containers, FormatTabKind.Subtitles, FormatTabKind.Hdr]), "five tabs in order");
        Check(page.Tabs.All(tab => tab.Rows.Count > 0), "every tab has rows");
        Check(page.Tabs.Select(tab => tab.Title).SequenceEqual(["Video", "Audio", "Containers", "Subtitles", "HDR"]), "the core's tab titles");
        Check(page.Tabs.All(tab => tab.SupportHeader == "On this PC" && tab.NotesHeader == "Notes"), "the core's column headers");
        Check(!hardware.ToString().Contains("Example"), "the adapter's name stays out of the text form");
        Check(page.HardwareNote == "Tested on: Example Graphics", page.HardwareNote);
        // The test engine decodes HEVC and H.264 only; the card lists HEVC Main.
        var video = page.Tabs[0].Rows;
        var hevc = video.Single(row => row.Name == "HEVC (H.265)");
        Check(hevc.Support == FormatSupport.Hardware && hevc.SupportLabel == "Hardware", "HEVC on the card");
        Check(video.Single(row => row.Name == "AV1").Support == FormatSupport.ServerConverts, "AV1 converted by the server");
        Check(page.Tabs[4].Rows.All(row => row.Support == FormatSupport.ToneMapped), "HDR tone-mapped on an SDR display");
        var unknown = await core.SupportedFormatsAsync(null, null);
        Check(unknown.Tabs[0].Rows.Single(row => row.Name == "H.264 (AVC)").Support == FormatSupport.Plays, "no test: Plays");
    }

    private static async Task<PlayerException> ThrowsPlayer(Func<Task> call)
    {
        try
        {
            await call();
        }
        catch (PlayerException error)
        {
            return error;
        }
        throw new Exception("expected a PlayerException");
    }
}
