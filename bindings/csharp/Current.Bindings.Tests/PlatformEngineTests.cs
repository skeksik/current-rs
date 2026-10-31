using System.Collections.Concurrent;
using Current.Bindings;
using static PlayerTests;

/// <summary>
/// The platform engine through <see cref="PlatformEngineHost"/> (ADR 0025, W3): a fake C# engine beside the core's test engine,
/// against the Jellyfin stand-in. The native start through the loopback relay, events from another thread reaching the state, a
/// failed track handing the file to the test engine, ADR 0025's labels, exceptions in the engine, the "Player" setting, and that
/// no URL shows in the file's text forms.
/// </summary>
internal static class PlatformEngineTests
{
    private static readonly PanelSizes Panel = new(1280, 720, 1280, 720, null, null, null, null);

    /// <summary>Windows with H.264, HEVC and AAC decoders.</summary>
    private static IReadOnlyList<CodecQueryAnswer> Answers() =>
        PlatformEngineHost.WindowsCodecQueries()
            .Select(ask => new CodecQueryAnswer(
                ask.Subtype,
                ask.Subtype is "VideoFormatH264" or "VideoFormatHevc" or "AudioFormatAac" ? 1u : 0u))
            .ToArray();

    /// <summary>Records what the core asks, and reports when the test says so. Throws from every call when asked to.</summary>
    private sealed class FakeEngine(bool throws = false) : PlatformEngineHost
    {
        public ConcurrentQueue<PlatformLoad> Loads { get; } = new();
        public ConcurrentQueue<string> Calls { get; } = new();

        private void Record(string call)
        {
            Calls.Enqueue(call);
            if (throws)
            {
                // An exception with free text and an address in it: the core must keep neither.
                throw new InvalidOperationException("boom at http://127.0.0.1:1/token");
            }
        }

        /// <summary>Opens and starts the last load, from a thread-pool thread as a platform's own events come.</summary>
        public Task StartFromAnotherThread() => Task.Run(() =>
        {
            Report(new PlatformEngineEvent.Loaded());
            Report(new PlatformEngineEvent.Duration(TimeSpan.FromMinutes(10)));
            Report(new PlatformEngineEvent.Restarted());
            Report(new PlatformEngineEvent.RatesSupported(true));
        });

        public void FailATrack() => Report(new PlatformEngineEvent.TrackFailed("decoder_failed"));

        /// <summary>An error about an earlier load: dropped by the core.</summary>
        public void FailAnOlderLoad() => Report(Generation - 1, new PlatformEngineEvent.Failed("decoder_failed"));

        protected override void Load(PlatformLoad file)
        {
            Loads.Enqueue(file);
            Record("load");
        }

        protected override void AddSubtitle(PlatformSubtitle subtitle) => Record($"subtitle {subtitle.Index}");
        protected override void SetPaused(bool paused) => Record($"paused {paused}");
        protected override void SeekTo(TimeSpan position) => Record($"seek {position}");
        protected override void SelectAudio(PlatformTrack track) => Record($"audio {track}");
        protected override void SelectSubtitle(PlatformTrack track) => Record($"subtitles {track}");
        protected override void SetVolume(byte volume) => Record($"volume {volume}");
        protected override void SetMuted(bool muted) => Record($"muted {muted}");
        protected override void SetSpeed(double speed) => Record($"speed {speed}");
        protected override void Stop() => Record("stop");
    }

    /// <summary>An engine whose loads fail with a failure it knows.</summary>
    private sealed class RefusingEngine : PlatformEngineHost
    {
        public int Loads;

        protected override void Load(PlatformLoad file)
        {
            Interlocked.Increment(ref Loads);
            throw new PlatformEngineFailure("unsupported_format");
        }

        protected override void AddSubtitle(PlatformSubtitle subtitle) { }
        protected override void SetPaused(bool paused) { }
        protected override void SeekTo(TimeSpan position) { }
        protected override void SelectAudio(PlatformTrack track) { }
        protected override void SelectSubtitle(PlatformTrack track) { }
        protected override void SetVolume(byte volume) { }
        protected override void SetMuted(bool muted) { }
        protected override void SetSpeed(double speed) { }
        protected override void Stop() { }
    }

    public static Task CoreAsksForWindowsDecoders()
    {
        var asks = PlatformEngineHost.WindowsCodecQueries();
        Check(asks.Contains(new CodecQueryAsk(CodecQueryKind.Video, "VideoFormatHevc")), "HEVC asked for");
        Check(asks.Contains(new CodecQueryAsk(CodecQueryKind.Audio, "AudioFormatDolbyDDPlus")), "E-AC-3 asked for");
        return Task.CompletedTask;
    }

    public static async Task PlaysNativelyThroughTheRelayAndHandsOver()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        var ui = new ManualDispatcher();
        var engine = new FakeEngine();
        var session = await PlayerSession.OpenWithPlatformEngineAsync(core, Size, Display, engine, Answers(), ui);
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.FromSeconds(3));
            Check(engine.Loads.Count == 1, $"one load on the platform engine, not {engine.Loads.Count}");
            var load = engine.Loads.Single();
            // The relay's URL: 127.0.0.1 on the relay's own port, not the stand-in's, and no session token.
            var url = new Uri(load.Url);
            Check(url.Scheme == "http" && url.Host == "127.0.0.1", "a loopback URL");
            Check(url.Port != new Uri(standIn.Address).Port, "the relay's port, not the server's");
            Check(!load.Url.Contains("apikey", StringComparison.OrdinalIgnoreCase)
                && !load.Url.Contains("api_key", StringComparison.OrdinalIgnoreCase), "no session token in the URL");
            Check(load.Start == TimeSpan.FromSeconds(3) && load.Audio == new PlatformTrack.Stream(1), $"the position and track, not {load}");
            // Neither the adapter's record nor the generated one prints the URL.
            Check(!load.ToString().Contains("127.0.0.1", StringComparison.Ordinal), $"no URL in {load}");
            var generated = engine.LastFile?.ToString() ?? "";
            Check(generated.Length > 0 && !generated.Contains("127.0.0.1", StringComparison.Ordinal), $"no URL in {generated}");

            await engine.StartFromAnotherThread();
            await Eventually("playing natively", ui, () => session.State.Value.State == PlayerStateKind.Playing
                && session.State.Value.Route?.Label == RouteLabel.Native);
            var route = session.State.Value.Route!;
            Check(route.Engine == RouteEngine.Platform && route.Kind == RouteKind.DirectPlay, $"on the platform engine, not {route}");
            var lines = await session.PlaybackInfoLinesAsync(null, Panel);
            Check(lines[0] == "Native", $"ADR 0025's name, not {lines[0]}");
            Check(core.RouteLabelName(RouteLabel.AppEngine) == "App engine", "the labels' words");

            // An older load's error is dropped; this load's failed track hands the file to the test engine.
            engine.FailAnOlderLoad();
            engine.FailATrack();
            await Eventually("on the app engine", ui, () => session.State.Value.State == PlayerStateKind.Playing
                && session.State.Value.Route?.Label == RouteLabel.AppEngine);
            Check(session.State.Value.Route!.Engine == RouteEngine.App, "libmpv's (the test engine's) surface");
            Check(engine.Calls.Contains("stop"), "the platform engine let the file go");
            lines = await session.PlaybackInfoLinesAsync(null, Panel);
            Check(lines[0] == "App engine" && lines[1] == "• Native could not start.", $"the failed route named, not {string.Join(" | ", lines)}");
        }
        finally
        {
            await session.CloseAsync();
        }
    }

    public static async Task ChooseHowToPlayMovesTheItem()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        var ui = new ManualDispatcher();
        var engine = new FakeEngine();
        var session = await PlayerSession.OpenWithPlatformEngineAsync(core, Size, Display, engine, Answers(), ui);
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await engine.StartFromAnotherThread();
            await Eventually("playing natively", ui, () => session.State.Value.Route?.Label == RouteLabel.Native
                && session.State.Value.State == PlayerStateKind.Playing);
            var choices = session.State.Value.HowToPlayChoices;
            Check(choices.Contains(HowToPlay.Auto) && choices.Contains(HowToPlay.AppEngine) && !choices.Contains(HowToPlay.DeviceRemux),
                $"the player's choices: {string.Join(", ", choices)}");
            await session.ChooseHowToPlayAsync(HowToPlay.AppEngine);
            await Eventually("on the app engine", ui, () => session.State.Value.Route?.Label == RouteLabel.AppEngine);
            Check(session.State.Value.HowToPlay == HowToPlay.AppEngine, "the choice in the state");
            await session.ChooseHowToPlayAsync(HowToPlay.Auto);
            await Eventually("native again with Auto", ui, () => session.State.Value.Route?.Label == RouteLabel.Native);
        }
        finally
        {
            await session.CloseAsync();
        }
    }

    public static async Task EngineExceptionsNeverBringTheCoreDown()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        var ui = new ManualDispatcher();

        // Every call throws: the load is a failed route, so the test engine plays; commands that throw change nothing.
        var throwing = new FakeEngine(throws: true);
        var session = await PlayerSession.OpenWithPlatformEngineAsync(core, Size, Display, throwing, Answers(), ui);
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await Eventually("on the app engine", ui, () => session.State.Value.State == PlayerStateKind.Playing
                && session.State.Value.Route?.Label == RouteLabel.AppEngine);
            session.SetVolume(30);
            await Eventually("the volume reached it", ui, () => throwing.Calls.Contains("volume 30"));
            Check(session.State.Value.State == PlayerStateKind.Playing, $"still playing, not {session.State.Value.State}");
            Check(session.State.Value.Problem is null, "no problem shown");
        }
        finally
        {
            await session.CloseAsync();
        }

        // A failure the engine knows: the same.
        var refusing = new RefusingEngine();
        session = await PlayerSession.OpenWithPlatformEngineAsync(core, Size, Display, refusing, Answers(), ui);
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await Eventually("on the app engine", ui, () => session.State.Value.Route?.Label == RouteLabel.AppEngine);
            Check(refusing.Loads == 1, $"tried once, not {refusing.Loads}");
        }
        finally
        {
            await session.CloseAsync();
        }
    }

    public static async Task AlwaysAppEnginePlaysTheNextItemThere()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        await SignIn(core, standIn);
        Check(core.PlaybackSettings().HowToPlay == HowToPlay.Auto, "Auto by default");
        Check(!core.SettingsHowToPlayChoices(false).Contains(HowToPlay.DeviceRemux), "no remux on the device in Windows' Settings");
        Check(core.HowToPlayName(HowToPlay.ServerTranscode) == "Server transcode", "the choice's name");
        await core.SetHowToPlayAsync(HowToPlay.AppEngine);
        Check(core.PlaybackSettings().HowToPlay == HowToPlay.AppEngine, "the setting read back");
        var ui = new ManualDispatcher();
        var engine = new FakeEngine();
        var session = await PlayerSession.OpenWithPlatformEngineAsync(core, Size, Display, engine, Answers(), ui);
        try
        {
            await session.PlayAsync(Item(StandIn.DirectPlayItem), TimeSpan.Zero);
            await Eventually("on the app engine", ui, () => session.State.Value.Route?.Label == RouteLabel.AppEngine);
            Check(engine.Loads.IsEmpty, "nothing for the platform engine");
        }
        finally
        {
            await session.CloseAsync();
            await core.SetHowToPlayAsync(HowToPlay.Auto);
        }
    }
}
