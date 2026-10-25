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
        Handle = handle.Handle();
        Generation = handle.Generation();
    }

    /// <summary>The <c>IDXGISwapChain*</c> as an integer.</summary>
    public ulong Handle { get; }

    /// <summary>The <c>IDXGISwapChain*</c> for <c>SetSwapChain</c>.</summary>
    public nint Pointer => unchecked((nint)Handle);

    /// <summary>Grows with every new swap chain.</summary>
    public ulong Generation { get; }

    public bool IsReleased => _handle.IsReleased();

    /// <summary>Gives the core's reference back; call it once <c>SetSwapChain</c> took its own.</summary>
    public void Release() => _handle.Release();

    public void Dispose()
    {
        _handle.Release();
        _handle.Dispose();
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
        CoreCall.RunVoidAsync(token => _player.Play(MapBrowse.ToFfi(item), PlayerMap.ToMs(start), token), ct);

    /// <summary>The audio stream with this server index.</summary>
    public Task SelectAudioAsync(uint index, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _player.SelectAudio(index, token), ct);

    public Task SelectSubtitleAsync(SubtitleSelection subtitle, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _player.SelectSubtitle(PlayerMap.ToFfi(subtitle), token), ct);

    /// <summary>The quality for the kind of address in use, remembered for it (local or internet).</summary>
    public Task SetQualityAsync(QualityChoice quality, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _player.SetQuality(PlayerMap.ToFfi(quality), token), ct);

    /// <summary>Plays the next episode now, from its start.</summary>
    public Task PlayNextAsync(CancellationToken ct = default) => CoreCall.RunVoidAsync(token => _player.PlayNext(token), ct);

    /// <exception cref="PlayerException">The player is closed (this and the other quick actions).</exception>
    public void Pause() => Quick(() => _player.Pause());

    public void Resume() => Quick(() => _player.Resume());

    public void Toggle() => Quick(() => _player.Toggle());

    public void SeekTo(TimeSpan position) => Quick(() => _player.SeekTo(PlayerMap.ToMs(position)));

    /// <summary>Back when negative.</summary>
    public void SeekBy(TimeSpan offset) => Quick(() => _player.SeekBy((long)offset.TotalMilliseconds));

    /// <summary>Stops the item (reported to the server); the engine stays for the next one.</summary>
    public void Stop() => Quick(() => _player.Stop());

    public void SkipSegment() => Quick(() => _player.SkipSegment());

    public void DismissNext() => Quick(() => _player.DismissNext());

    /// <summary>0 to 100.</summary>
    public void SetVolume(byte volume) => Quick(() => _player.SetVolume(volume));

    public void SetMuted(bool muted) => Quick(() => _player.SetMuted(muted));

    /// <summary>The window moved to another monitor, or the monitor's mode changed.</summary>
    public void SetDisplayFacts(DisplayFacts facts) => Quick(() => _player.SetDisplayFacts(PlayerMap.ToFfi(facts)));

    /// <summary>The panel's new size in physical pixels.</summary>
    public void SetCompositionSize(CompositionSize size) => Quick(() => _player.SetCompositionSize(PlayerMap.ToFfi(size)));

    /// <summary>The playback information view's content. No URL.</summary>
    public async Task<PlaybackInfo> PlaybackInfoAsync() =>
        PlayerMap.From(await CoreCall.RunUncancellableAsync(() => _player.PlaybackInfo()).ConfigureAwait(false));

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

    /// <summary>The steps of the quality menu after "Maximum", highest first.</summary>
    public static IReadOnlyList<QualityChoice> QualitySteps(this CoreHost core) =>
        Array.ConvertAll(Ffi.CurrentFfiMethods.QualitySteps(), step => PlayerMap.From(step));
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
