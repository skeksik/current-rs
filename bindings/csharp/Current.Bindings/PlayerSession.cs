using System.ComponentModel;
using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// A reference to the engine's swap chain for the window's <c>SwapChainPanel</c> (ADR 0007): pass <see cref="Pointer"/> to
/// <c>ISwapChainPanelNative::SetSwapChain</c>, then call <see cref="Release"/>. Releasing twice does nothing; the session releases a
/// swap chain once a newer one replaces it, and when it closes.
/// </summary>
public sealed class SwapChainRef : IDisposable
{
    private readonly Ffi.SwapChainHandle _handle;

    internal SwapChainRef(Ffi.SwapChainHandle handle)
    {
        _handle = handle;
        Handle = handle.Pointer();
        Generation = handle.Generation();
    }

    /// <summary>The <c>IDXGISwapChain*</c> as an integer.</summary>
    public ulong Handle { get; }

    /// <summary>The <c>IDXGISwapChain*</c> for <c>SetSwapChain</c>.</summary>
    public nint Pointer => unchecked((nint)Handle);

    /// <summary>Grows with every new swap chain.</summary>
    public ulong Generation { get; }

    private bool _disposed;

    public bool IsReleased => _disposed || _handle.IsReleased();

    /// <summary>Gives the core's reference back; call it once <c>SetSwapChain</c> took its own. Nothing after <see cref="Dispose"/>.</summary>
    public void Release()
    {
        if (!_disposed)
        {
            _handle.Release();
        }
    }

    /// <summary>Releases the reference and frees the native handle; a second call does nothing.</summary>
    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _handle.Release();
        _handle.Dispose();
        _disposed = true;
    }

    /// <summary>The generation only: the handle is a pointer.</summary>
    public override string ToString() => $"SwapChainRef(generation {Generation})";
}

/// <summary>
/// A player for one window (ADR 0019), mirrored on the UI thread: <see cref="State"/>, <see cref="NowPlaying"/> and
/// <see cref="SwapChain"/> change only there. Long actions (play, track and quality changes, the next episode) are awaited and
/// cancellable; quick ones (pause, seek, volume) are queued and run in the order they were called. Close it with
/// <see cref="CloseAsync"/> after detaching the panel: that reports the stop to the server and shuts the engine down, so the next
/// player can open.
/// </summary>
public sealed class PlayerSession : IAsyncDisposable
{
    private readonly Ffi.Player _player;
    private SwapChainRef? _current;
    private bool _closed;

    /// <summary>The generated player while the session is open: every call after <see cref="CloseAsync"/> fails with
    /// <see cref="PlayerException"/> (<see cref="PlayerProblem.Closed"/>), never with the freed native object's exception.</summary>
    private Ffi.Player Open => _closed ? throw new PlayerException(new PlayerProblem.Closed()) : _player;

    private PlayerSession(Ffi.Player player, IUiDispatcher ui)
    {
        _player = player;
        State = new ValueSubscription<PlayerState>(PlayerState.Idle);
        State.Attach(player.SubscribeState(new PlayerStateForwarder(State, ui)));
        NowPlaying = new ValueSubscription<NowPlaying>(Current.Bindings.NowPlaying.Nothing);
        NowPlaying.Attach(player.SubscribeNowPlaying(new NowPlayingForwarder(NowPlaying, ui)));
        SwapChain = new ValueSubscription<SwapChainRef?>(null);
        // Registered before any page handler, so a replaced swap chain is given back as the new one arrives (the panel keeps its own
        // reference to whatever it shows).
        SwapChain.PropertyChanged += OnSwapChain;
        SwapChain.Attach(player.SubscribeSwapChain(new SwapChainForwarder(SwapChain, ui)));
    }

    /// <summary>
    /// Opens a player with the panel's size in physical pixels and the display's facts. One engine per process for now: while
    /// another player is open this waits a few seconds for it to close.
    /// </summary>
    /// <exception cref="PlayerException">No engine in this build, another player is open (<c>Busy</c>), or the engine failed.</exception>
    public static async Task<PlayerSession> OpenAsync(CoreHost core, CompositionSize size, DisplayFacts display, IUiDispatcher ui)
    {
        var player = await CoreCall.RunUncancellableAsync(
            () => core.Core.OpenPlayer(PlayerMap.ToFfi(size), PlayerMap.ToFfi(display))).ConfigureAwait(false);
        return new PlayerSession(player, ui);
    }

    /// <summary>
    /// Opens a player with the platform's own player (<paramref name="engine"/>, ADR 0025) beside libmpv: the core plays a file on
    /// <paramref name="engine"/> when Windows decodes it (<paramref name="answers"/> are the UI's answers to
    /// <see cref="PlatformEngineHost.WindowsCodecQueries"/>), on libmpv otherwise, and moves it between them. The routes and the
    /// info panel's words are then ADR 0025's. <see cref="OpenAsync"/> stays the player with libmpv alone.
    /// </summary>
    /// <exception cref="PlayerException">As <see cref="OpenAsync"/>, and <c>EngineFailed</c> when the engine could not be
    /// attached.</exception>
    public static async Task<PlayerSession> OpenWithPlatformEngineAsync(
        CoreHost core, CompositionSize size, DisplayFacts display, PlatformEngineHost engine, IReadOnlyList<CodecQueryAnswer> answers,
        IUiDispatcher ui)
    {
        ArgumentNullException.ThrowIfNull(engine);
        ArgumentNullException.ThrowIfNull(answers);
        var native = answers.Select(answer => PlatformEngineMap.ToFfi(answer)).ToArray();
        var player = await CoreCall.RunUncancellableAsync(
            () => core.Core.OpenPlayerWithPlatformEngine(PlayerMap.ToFfi(size), PlayerMap.ToFfi(display), engine, native))
            .ConfigureAwait(false);
        return new PlayerSession(player, ui);
    }

    public ValueSubscription<PlayerState> State { get; }

    /// <summary>What the system media controls show.</summary>
    public ValueSubscription<NowPlaying> NowPlaying { get; }

    /// <summary>The engine's swap chain (Windows), null until there is one and after closing.</summary>
    public ValueSubscription<SwapChainRef?> SwapChain { get; }

    /// <summary>Plays <paramref name="item"/> from <paramref name="start"/> (a resume position or zero). Returns once it is loading or
    /// failed: <see cref="State"/> says which.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled: nothing stays half-started.</exception>
    /// <exception cref="PlayerException">The item's server is not signed in, or the player is closed.</exception>
    public Task PlayAsync(ItemRef item, TimeSpan start, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => Open.Play(MapBrowse.ToFfi(item), PlayerMap.ToMs(start), token), ct);

    /// <summary>The audio stream with this server index.</summary>
    public Task SelectAudioAsync(uint index, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => Open.SelectAudio(index, token), ct);

    public Task SelectSubtitleAsync(SubtitleSelection subtitle, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => Open.SelectSubtitle(PlayerMap.ToFfi(subtitle), token), ct);

    /// <summary>
    /// The quality of the item playing, as picked; plays on at the position with it. Not saved: the next item plays at the preferred
    /// quality (<see cref="PlaybackExtensions.SetPreferredQualityAsync"/>).
    /// </summary>
    public Task SetQualityAsync(QualityChoice quality, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => Open.SetQuality(PlayerMap.ToFfi(quality), token), ct);

    /// <summary>"Play with app engine": the item playing moves to libmpv at the position, then the server if libmpv cannot (ADR 0025).
    /// Only this item; nothing changes on a player with libmpv alone.</summary>
    public Task PlayWithAppEngineAsync(CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => Open.PlayWithAppEngine(token), ct);

    /// <summary>Plays the next episode now, from its start.</summary>
    public Task PlayNextAsync(CancellationToken ct = default) => CoreCall.RunVoidAsync(token => Open.PlayNext(token), ct);

    /// <exception cref="PlayerException">The player is closed (this and the other quick actions).</exception>
    public void Pause() => Quick(() => Open.Pause());

    public void Resume() => Quick(() => Open.Resume());

    public void Toggle() => Quick(() => Open.Toggle());

    public void SeekTo(TimeSpan position) => Quick(() => Open.SeekTo(PlayerMap.ToMs(position)));

    /// <summary>Back when negative.</summary>
    public void SeekBy(TimeSpan offset) => Quick(() => Open.SeekBy((long)offset.TotalMilliseconds));

    /// <summary>Stops the item (reported to the server); the engine stays for the next one.</summary>
    public void Stop() => Quick(() => Open.Stop());

    public void SkipSegment() => Quick(() => Open.SkipSegment());

    public void DismissNext() => Quick(() => Open.DismissNext());

    /// <summary>0 to 100.</summary>
    public void SetVolume(byte volume) => Quick(() => Open.SetVolume(volume));

    public void SetMuted(bool muted) => Quick(() => Open.SetMuted(muted));

    /// <summary>The window moved to another monitor, or the monitor's mode changed.</summary>
    public void SetDisplayFacts(DisplayFacts facts) => Quick(() => Open.SetDisplayFacts(PlayerMap.ToFfi(facts)));

    /// <summary>The panel's new size in physical pixels.</summary>
    public void SetCompositionSize(CompositionSize size) => Quick(() => Open.SetCompositionSize(PlayerMap.ToFfi(size)));

    /// <summary>
    /// The quality menu for the version playing, as the core names it: Maximum (the original) first, then the common steps, or every
    /// step when <paramref name="all"/>, below the original's bitrate, each with the resolution the server would send it at and its
    /// bitrate.
    /// </summary>
    public async Task<IReadOnlyList<QualityOption>> QualityOptionsAsync(bool all) =>
        Array.ConvertAll(
            await CoreCall.RunUncancellableAsync(() => Open.QualityOptions(all)).ConfigureAwait(false),
            option => PlayerMap.From(option));

    /// <summary>The playback information view's content. No URL.</summary>
    public async Task<PlaybackInfo> PlaybackInfoAsync() =>
        PlayerMap.From(await CoreCall.RunUncancellableAsync(() => Open.PlaybackInfo()).ConfigureAwait(false));

    /// <summary>
    /// The playback info panel's lines as the core words them, an empty one between groups: the information now, the facts of the
    /// display the window is on (null when there are none) and the panel's sizes as the window measures them. Numbers with decimals
    /// use the decimal separator passed to <see cref="PlaybackExtensions.SetDecimalSeparator"/>. No URL.
    /// </summary>
    public async Task<IReadOnlyList<string>> PlaybackInfoLinesAsync(DisplayFacts? display, PanelSizes panel)
    {
        ArgumentNullException.ThrowIfNull(panel);
        var ffiDisplay = display is { } facts ? PlayerMap.ToFfi(facts) : null;
        return await CoreCall.RunUncancellableAsync(() => Open.PlaybackInfoLines(ffiDisplay, PlayerWordsMap.ToFfi(panel)))
            .ConfigureAwait(false);
    }

    /// <summary>
    /// Stops the item (reported to the server, best effort), gives the swap chain back and shuts the engine down; returns once it is
    /// gone. Detach the panel first. The subscriptions end.
    /// </summary>
    public async Task CloseAsync()
    {
        if (_closed)
        {
            return;
        }
        _closed = true;
        await _player.Close().ConfigureAwait(false);
        State.Dispose();
        NowPlaying.Dispose();
        SwapChain.PropertyChanged -= OnSwapChain;
        SwapChain.Dispose();
        _current?.Dispose();
        _current = null;
        _player.Dispose();
    }

    public async ValueTask DisposeAsync() => await CloseAsync().ConfigureAwait(false);

    private void OnSwapChain(object? sender, PropertyChangedEventArgs e)
    {
        var next = SwapChain.Value;
        if (!ReferenceEquals(next, _current))
        {
            _current?.Dispose();
            _current = next;
        }
    }

    private static void Quick(Action action) => CoreCall.Run(() =>
    {
        action();
        return true;
    });
}

/// <summary>The playback settings and what the core can play, on <see cref="CoreHost"/>.</summary>
public static class PlaybackExtensions
{
    /// <summary>The engine of this build of the core: <c>Libmpv</c> in the app once it ships libmpv, <c>Unavailable</c> before.</summary>
    public static PlayerEngineKind PlayerEngine(this CoreHost core) => PlayerMap.From(Ffi.CurrentFfiMethods.PlayerEngine());

    /// <summary>Settings → Playback now.</summary>
    public static PlaybackSettings PlaybackSettings(this CoreHost core) => PlayerMap.From(core.Core.PlaybackSettings());

    /// <summary>The Settings switch: skip intros and credits automatically. Saved at once; open players follow at once.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: the setting stands for this run.</exception>
    public static Task SetSkipIntrosAndCreditsAsync(this CoreHost core, bool skip) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await core.Core.SetSkipIntrosAndCredits(skip).ConfigureAwait(false);
            return true;
        });

    /// <summary>
    /// The Settings choice "Preferred quality": each item plays at the highest entry of its quality menu that is not above it, on local
    /// and internet addresses alike. Saved at once; it applies from the next item played.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: the setting stands for this run.</exception>
    public static Task SetPreferredQualityAsync(this CoreHost core, QualityChoice quality) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await core.Core.SetPreferredQuality(PlayerMap.ToFfi(quality)).ConfigureAwait(false);
            return true;
        });

    /// <summary>Settings → Playback → "Player" (ADR 0025): Automatic or Always app engine. Saved at once; it applies from the next
    /// item played.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: the setting stands for this run.</exception>
    public static Task SetPlayerChoiceAsync(this CoreHost core, PlayerChoice choice) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await core.Core.SetPlayerChoice(PlayerMap.ToFfi(choice)).ConfigureAwait(false);
            return true;
        });

    /// <summary>The Settings switch: show each quality's resolution in the quality menu. Saved at once.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: the setting stands for this run.</exception>
    public static Task SetShowQualityResolutionAsync(this CoreHost core, bool on) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await core.Core.SetShowQualityResolution(on).ConfigureAwait(false);
            return true;
        });

    /// <summary>The Settings switch: list every quality step in the quality menu, not only the common ones. Saved at once.</summary>
    /// <exception cref="CoreStoreException">The settings could not be saved: the setting stands for this run.</exception>
    public static Task SetShowAllQualitiesAsync(this CoreHost core, bool all) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await core.Core.SetShowAllQualities(all).ConfigureAwait(false);
            return true;
        });

    /// <summary>
    /// Settings' "Preferred quality" list (the core decides): Original, then the steps the quality menu lists ("Show all quality
    /// options" as set), each named by its usual resolution. The preferred quality is always one of them: switching "Show all" off
    /// moves a step only it listed to the nearest listed one not above it.
    /// </summary>
    public static IReadOnlyList<QualityOption> PreferredQualityOptions(this CoreHost core) =>
        Array.ConvertAll(core.Core.PreferredQualityOptions(), option => PlayerMap.From(option));

    /// <summary>The steps of the quality menu after "Maximum", highest first.</summary>
    public static IReadOnlyList<QualityChoice> QualitySteps(this CoreHost core) =>
        Array.ConvertAll(Ffi.CurrentFfiMethods.QualitySteps(), step => PlayerMap.From(step));

    /// <summary>
    /// The platform's decimal separator, for the core's labels with decimals ("26,1 Mbit/s" in a region that writes a comma): pass
    /// it once at start. "." until then; an empty one changes nothing.
    /// </summary>
    public static void SetDecimalSeparator(this CoreHost core, string separator)
    {
        ArgumentNullException.ThrowIfNull(separator);
        core.Core.SetDecimalSeparator(separator);
    }

    /// <summary>A bitrate as the player shows it: "40 Mbit/s", "1.5 Mbit/s", "720 kbit/s" below 1 Mbit/s (the core's words).</summary>
    public static string BitrateLabel(this CoreHost core, uint bitsPerSecond) => core.Core.BitrateLabel(bitsPerSecond);

    /// <summary>A quality entry's label: "Original · 1080p · 5.2 Mbit/s", "720p · 4 Mbit/s", without the resolution unless
    /// <paramref name="withResolution"/>; the parts not known are left out (the core's words).</summary>
    public static string QualityLabel(this CoreHost core, QualityOption option, bool withResolution)
    {
        ArgumentNullException.ThrowIfNull(option);
        return core.Core.QualityLabel(PlayerMap.ToFfi(option), withResolution);
    }

    /// <summary>
    /// The quality menu while playing at <paramref name="current"/> (the core decides): <paramref name="options"/> as
    /// <see cref="PlayerSession.QualityOptionsAsync"/> named them, or null before then for Original and the common steps. A limit
    /// below or between the entries is listed as its own entry; the entry in use is checked.
    /// </summary>
    public static IReadOnlyList<QualityMenuEntry> QualityMenu(
        this CoreHost core, IReadOnlyList<QualityOption>? options, QualityChoice current, bool withResolution)
    {
        ArgumentNullException.ThrowIfNull(current);
        var menu = core.Core.QualityMenu(
            options?.Select(option => PlayerMap.ToFfi(option)).ToArray(), PlayerMap.ToFfi(current), withResolution);
        return Array.ConvertAll(menu, entry => PlayerMap.From(entry));
    }
}

internal sealed class PlayerStateForwarder(ValueSubscription<PlayerState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.PlayerStateSnapshot, PlayerState>(target, ui, state => PlayerMap.From(state)), Ffi.PlayerStateListener
{
    public void OnValue(Ffi.PlayerStateSnapshot value) => Forward(value);
}

internal sealed class NowPlayingForwarder(ValueSubscription<NowPlaying> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.NowPlayingSnapshot, NowPlaying>(target, ui, now => PlayerMap.From(now)), Ffi.NowPlayingListener
{
    public void OnValue(Ffi.NowPlayingSnapshot value) => Forward(value);
}

internal sealed class SwapChainForwarder(ValueSubscription<SwapChainRef?> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.SwapChainHandle?, SwapChainRef?>(target, ui, handle => handle is null ? null : new SwapChainRef(handle)),
      Ffi.SwapChainListener
{
    public void OnValue(Ffi.SwapChainHandle? value) => Forward(value);
}
