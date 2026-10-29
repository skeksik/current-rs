using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.Media.Streaming.Adaptive;

namespace Current.Shell.Player;

/// <summary>
/// Windows' own player as the core's platform engine (ADR 0025, W4): Media Foundation through <c>Windows.Media.Playback.MediaPlayer</c>,
/// shown by the player page's <c>MediaPlayerElement</c> with its transport controls off and our controls over it, as the W1 spike ran
/// it (docs/notes/windows-native-player.md, "How the prototype works"). The core picks the route and moves the file between engines;
/// this plays what it is told and reports facts: kinds from <see cref="Kinds"/> only, never a URL, a path or server text.
/// <list type="bullet">
/// <item>The decodability check (ADR 0025, "How the core decides"; W1 §2, "What the decodability check must use"): a start has failed
/// when a selected track's <c>DecoderStatus</c> is not <c>FullySupported</c>, a selected track raises <c>OpenFailed</c>, or any
/// <c>MediaFailed</c> arrives; the playback state is no proof (W1 §5: an MKV whose audio has no decoder plays on silently,
/// <c>Playing</c>). <c>Degraded</c> fails too, although "The media item can be decoded by the decoder, but with a degradation from the
/// encoding" (MediaDecoderStatus Enum, learn.microsoft.com, ms.date 2017-02-08, read 2026-10-29): libmpv plays it in full.</item>
/// <item>Tracks by <c>Id</c>: when the file opens, the n-th of the file's own audio streams by the server's index (the player state's
/// tracks) is taken as the n-th of <c>AudioTracks</c>, and from then on found by that track's <c>Id</c> (W1 §6: the order of timed
/// metadata tracks changed between opens, each <c>Id</c> stayed). That Media Foundation lists audio tracks in the container's order is
/// not verified (the owner's device check, W7).</item>
/// <item>Rates: <c>IsSupportedPlaybackRateRange(0.5, 2)</c> after every open and audio track change (W1 §8: false with E-AC-3 Atmos,
/// whose speed changes are ignored).</item>
/// <item>HLS through <c>AdaptiveMediaSource</c> (W1 §7), with a deadline of its own for the first segment (<see
/// cref="HlsFirstSegmentDeadline"/>).</item>
/// <item>The system media controls stay ours (MediaControls): <c>CommandManager.IsEnabled = false</c>, as W1 §8 checked.</item>
/// <item>No subtitles yet (W5): the core lists none for this engine (platform_caps.rs), so a selected subtitle goes to libmpv; a
/// subtitle command other than off is refused.</item>
/// </list>
/// Threads (PlatformEngineHost): the core's calls hand their work to the UI thread and return at once. Media Foundation's events come
/// on its own threads with the source they are about, and are handed to the UI thread, where an event whose source is not the open
/// file's is dropped; all state here is the UI thread's, except the track hooks (below).
/// </summary>
internal sealed class NativeEngine : PlatformEngineHost, IDisposable
{
    /// <summary>The failure kinds this engine reports (the core reads any other as unexpected).</summary>
    internal static class Kinds
    {
        public const string LoadingFailed = "loading_failed";
        public const string UnsupportedFormat = "unsupported_format";
        public const string DecoderFailed = "decoder_failed";
        /// <summary>No decoder for a selected track (<c>UnsupportedSubtype</c>, or <c>MF_E_TOPO_CODEC_NOT_FOUND</c>).</summary>
        public const string DecoderMissing = "decoder_missing";
        /// <summary>A selected track's <c>DecoderStatus</c> is <c>Degraded</c>.</summary>
        public const string DecoderDegraded = "decoder_degraded";
        /// <summary>An HLS stream's first segment after a load or a far seek did not come within <see
        /// cref="HlsFirstSegmentDeadline"/>.</summary>
        public const string StreamTimeout = "stream_timeout";
        /// <summary>A subtitle command before W5 (the core sends none: it lists no native subtitles).</summary>
        public const string SubtitlesNotYet = "subtitles_not_yet";
    }

    /// <summary>
    /// After the first frame, the selected tracks' <c>DecoderStatus</c> is read again this long later: W1 §2 read it "at open and
    /// later", and saw H.264 High 10 report <c>FullySupported</c> at <c>MediaOpened</c> and then fail with <c>MediaFailed</c>
    /// (<c>DecodingError</c>, E_FAIL) at once. That <c>MediaFailed</c> is a Failed whenever it comes; the window catches a status
    /// that changes after opening. 5 s is ours, not measured.
    /// </summary>
    internal static readonly TimeSpan DecodabilityWindow = TimeSpan.FromSeconds(5);

    /// <summary>
    /// How long an HLS stream may take to show its first segment after a load or a seek outside what is buffered, before the file is
    /// reported Failed and the core moves it on (to libmpv). W1 §7: a far seek into a server's remux waited more than 20 s for the
    /// first segment, and Media Foundation gave up after about 10–12 s with <c>MF_E_NET_TIMEOUT</c> (0xC00D4278) and asked again; 8 s
    /// comes before that (ADR 0025, caveat for W4). The segment has come when the position moves while playing, or a buffered range
    /// reaches past the position (paused).
    /// </summary>
    internal static readonly TimeSpan HlsFirstSegmentDeadline = TimeSpan.FromSeconds(8);

    /// <summary>How often the position and the buffered range are reported while a file is open.</summary>
    private static readonly TimeSpan PositionEvery = TimeSpan.FromMilliseconds(250);

    /// <summary>The range W1 §8 checked rates over.</summary>
    private const double SlowestRate = 0.5;
    private const double FastestRate = 2;

    /// <summary>Media Foundation's HRESULTs this engine names (<c>um/mferror.h</c>, Windows SDK 10.0.26100.0, as W1 read it;
    /// mingw-w64's <c>mferror.h</c>, read 2026-10-29, has the same values).</summary>
    internal static class Codes
    {
        /// <summary><c>MF_E_TOPO_CODEC_NOT_FOUND</c>.</summary>
        public const int TopoCodecNotFound = unchecked((int)0xC00D5212);
        /// <summary><c>MF_E_INVALIDMEDIATYPE</c> (W1 §5: Opus 5.1 in MKV).</summary>
        public const int InvalidMediaType = unchecked((int)0xC00D36B4);
        /// <summary><c>MF_E_UNSUPPORTED_BYTESTREAM_TYPE</c>.</summary>
        public const int UnsupportedByteStreamType = unchecked((int)0xC00D36C4);
        /// <summary><c>MF_E_NO_AUDIO_PLAYBACK_DEVICE</c>.</summary>
        public const int NoAudioPlaybackDevice = unchecked((int)0xC00D4E84);
    }

    private readonly DispatcherQueue _ui;
    private readonly Func<IReadOnlyList<uint>> _audioStreams;
    private readonly DispatcherQueueTimer _positionTimer;
    private readonly DispatcherQueueTimer _windowTimer;
    private readonly DispatcherQueueTimer _deadlineTimer;
    /// <summary>The file open in the player; UI thread only.</summary>
    private NativeFile? _file;
    private bool _paused;
    private double _volume = 1;
    private bool _muted;
    private double _speed = 1;
    private bool _disposed;

    /// <param name="ui">The UI thread's queue: the player is made and used there.</param>
    /// <param name="audioStreams">The server's indices of the file's own audio streams, in order (the player state's tracks that are
    /// not external); read on the UI thread when a file opens.</param>
    /// <exception cref="Exception">Windows' player cannot be made (an edition without Media Foundation): open without it.</exception>
    public NativeEngine(DispatcherQueue ui, Func<IReadOnlyList<uint>> audioStreams)
    {
        _ui = ui;
        _audioStreams = audioStreams;
        Player = new MediaPlayer { AutoPlay = false };
        Player.CommandManager.IsEnabled = false;
        Player.MediaOpened += (sender, _) => FromPlayer(sender.Source, Opened);
        Player.MediaFailed += (sender, args) =>
        {
            var code = args.ExtendedErrorCode?.HResult;
            var kind = KindOf(code, args.Error switch
            {
                MediaPlayerError.DecodingError => Kinds.DecoderFailed,
                MediaPlayerError.SourceNotSupported => Kinds.UnsupportedFormat,
                _ => Kinds.LoadingFailed,
            });
            FromPlayer(sender.Source, file => Fail(file, new PlatformEngineEvent.Failed(kind), code));
        };
        Player.MediaEnded += (sender, _) => FromPlayer(sender.Source, Ended);
        Player.PlaybackSession.PlaybackStateChanged += (_, _) => FromPlayer(Player.Source, StateChanged);
        Player.PlaybackSession.SeekCompleted += (_, _) => FromPlayer(Player.Source, SeekCompleted);
        Player.PlaybackSession.NaturalDurationChanged += (_, _) => FromPlayer(Player.Source, ReportDuration);

        _positionTimer = ui.CreateTimer();
        _positionTimer.Interval = PositionEvery;
        _positionTimer.Tick += (_, _) =>
        {
            if (_file is { Opened: true, Failed: false } file)
            {
                ReportPosition(file);
            }
        };
        _windowTimer = ui.CreateTimer();
        _windowTimer.Interval = DecodabilityWindow;
        _windowTimer.IsRepeating = false;
        _windowTimer.Tick += (_, _) =>
        {
            if (_file is { } file)
            {
                CheckTracks(file);
            }
        };
        _deadlineTimer = ui.CreateTimer();
        _deadlineTimer.Interval = HlsFirstSegmentDeadline;
        _deadlineTimer.IsRepeating = false;
        _deadlineTimer.Tick += (_, _) =>
        {
            if (_file is { AwaitingSegment: true } file)
            {
                Fail(file, new PlatformEngineEvent.Failed(Kinds.StreamTimeout), null);
            }
        };
    }

    /// <summary>The player the page's MediaPlayerElement shows.</summary>
    public MediaPlayer Player { get; }

    /// <summary>The last failure, for the smoke run's line: its kind and Media Foundation's HRESULT, nothing else.</summary>
    internal string? LastFailure { get; private set; }

    /// <summary>The HRESULT of the last failure, if Media Foundation gave one (the smoke run's skip rule).</summary>
    internal int? LastFailureCode { get; private set; }

    /// <summary>One load: its source, its item and what is known about it. UI thread only, but <see cref="Hooked"/> (its lock).</summary>
    private sealed class NativeFile(PlatformLoad load)
    {
        public ulong Generation { get; } = load.Generation;
        public bool Hls { get; } = load.Hls;
        public PlatformTrack Audio { get; set; } = load.Audio;
        public TimeSpan? PendingSeek { get; set; }
        public MediaSource? Source { get; set; }
        public MediaPlaybackItem? Item { get; set; }
        public bool Opened { get; set; }
        public bool Started { get; set; }
        public bool Failed { get; set; }
        public bool Ended { get; set; }
        /// <summary>An HLS load or far seek waits for its first segment (<see cref="HlsFirstSegmentDeadline"/>).</summary>
        public bool AwaitingSegment { get; set; }
        /// <summary>Where the position was when that wait began.</summary>
        public TimeSpan AwaitedFrom { get; set; }
        /// <summary>The track Id of each server audio index, from the open.</summary>
        public Dictionary<uint, string> AudioIds { get; } = [];
        /// <summary>The tracks whose OpenFailed is hooked ("a" or "v" and the Id); taken from Media Foundation's threads too.</summary>
        public HashSet<string> Hooked { get; } = [];
        /// <summary>The tracks that raised OpenFailed, with the kind it means.</summary>
        public Dictionary<string, string> FailedTracks { get; } = [];
        /// <summary>Unhooks every handler on the item and its tracks, so nothing keeps the load alive after it closes.</summary>
        public List<Action> Detach { get; } = [];
    }

    // The core's commands (its threads) -----------------------------------------------------------------------------------------

    protected override void Load(PlatformLoad file)
    {
        var generation = file.Generation;
        if (!_ui.TryEnqueue(() => _ = OpenAsync(file)))
        {
            Report(generation, new PlatformEngineEvent.Failed(Kinds.LoadingFailed));
        }
    }

    protected override void AddSubtitle(PlatformSubtitle subtitle) => throw new PlatformEngineFailure(Kinds.SubtitlesNotYet);

    protected override void SetPaused(bool paused) => Post(() =>
    {
        _paused = paused;
        if (_file is { Opened: true, Failed: false })
        {
            if (paused)
            {
                Player.Pause();
            }
            else
            {
                Player.Play();
            }
        }
    });

    protected override void SeekTo(TimeSpan position) => Post(() =>
    {
        if (_file is not { } file)
        {
            return;
        }
        if (!file.Opened)
        {
            file.PendingSeek = position;
            return;
        }
        Seek(file, position);
    });

    protected override void SelectAudio(PlatformTrack track) => Post(() =>
    {
        if (_file is not { } file)
        {
            return;
        }
        file.Audio = track;
        if (file.Opened)
        {
            ApplyAudio(file);
        }
    });

    protected override void SelectSubtitle(PlatformTrack track)
    {
        if (track is PlatformTrack.Stream)
        {
            throw new PlatformEngineFailure(Kinds.SubtitlesNotYet);
        }
    }

    protected override void SetVolume(byte volume) => Post(() =>
    {
        _volume = Math.Clamp(volume, (byte)0, (byte)100) / 100.0;
        Player.Volume = _volume;
    });

    protected override void SetMuted(bool muted) => Post(() =>
    {
        _muted = muted;
        Player.IsMuted = muted;
    });

    protected override void SetSpeed(double speed) => Post(() =>
    {
        _speed = speed;
        if (_file is { Opened: true })
        {
            Player.PlaybackSession.PlaybackRate = speed;
        }
    });

    protected override void Stop() => Post(Unload);

    /// <summary>Runs a command on the UI thread. One that throws is dropped, as the bridge drops a command that fails (PlatformEngineHost:
    /// only a failed load moves the file on); the log has the exception's type only.</summary>
    private void Post(Action work) => _ui.TryEnqueue(() =>
    {
        if (_disposed)
        {
            return;
        }
        try
        {
            work();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a native engine command failed: {error.GetType().Name}");
        }
    });

    /// <summary>An event of the player (Media Foundation's thread) about <paramref name="source"/>, its Source then, on the UI thread;
    /// dropped unless that is still the open file's item.</summary>
    private void FromPlayer(IMediaPlaybackSource? source, Action<NativeFile> handle)
    {
        if (source is null)
        {
            return;
        }
        _ui.TryEnqueue(() =>
        {
            if (_disposed || _file is not { } file || !ReferenceEquals(source, file.Item))
            {
                return;
            }
            try
            {
                handle(file);
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: the native engine's event failed: {error.GetType().Name}");
            }
        });
    }

    /// <summary>An event of one item or track of <paramref name="file"/>, on the UI thread while that file is open.</summary>
    private void FromItem(NativeFile file, Action handle) => _ui.TryEnqueue(() =>
    {
        if (_disposed || _file != file)
        {
            return;
        }
        try
        {
            handle();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the native engine's track event failed: {error.GetType().Name}");
        }
    });

    // Opening and closing (UI thread) ---------------------------------------------------------------------------------------------

    private async Task OpenAsync(PlatformLoad load)
    {
        if (_disposed)
        {
            return;
        }
        Unload();
        var file = new NativeFile(load);
        _file = file;
        // A load starts playing; the core sends the session's pause after it.
        _paused = false;
        try
        {
            var uri = new Uri(load.Url);
            MediaSource source;
            if (load.Hls)
            {
                // The deadline covers the playlists too: they are the start of the first segment's wait.
                AwaitSegment(file, load.Start);
                var created = await AdaptiveMediaSource.CreateFromUriAsync(uri);
                if (_file != file)
                {
                    return;
                }
                if (created.Status != AdaptiveMediaSourceCreationStatus.Success || created.MediaSource is null)
                {
                    Trace.TraceWarning($"Current: the native engine could not open the HLS stream ({created.Status})");
                    Fail(file, new PlatformEngineEvent.Failed(Kinds.LoadingFailed), created.ExtendedError?.HResult);
                    return;
                }
                source = MediaSource.CreateFromAdaptiveMediaSource(created.MediaSource);
            }
            else
            {
                source = MediaSource.CreateFromUri(uri);
            }
            file.Source = source;
            var item = new MediaPlaybackItem(source, load.Start);
            file.Item = item;
            Hook(file, item);
            Player.Volume = _volume;
            Player.IsMuted = _muted;
            Player.Source = item;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the native engine could not open the file: {error.GetType().Name}");
            if (_file == file)
            {
                Fail(file, new PlatformEngineEvent.Failed(Kinds.LoadingFailed), error.HResult);
            }
            else
            {
                Report(load.Generation, new PlatformEngineEvent.Failed(Kinds.LoadingFailed));
            }
        }
    }

    /// <summary>The item's own events, each unhooked again by <see cref="NativeFile.Detach"/>. The tracks' OpenFailed is hooked on
    /// Media Foundation's thread as their list changes, before any of them can raise it.</summary>
    private void Hook(NativeFile file, MediaPlaybackItem item)
    {
        Windows.Foundation.TypedEventHandler<MediaPlaybackItem, Windows.Foundation.Collections.IVectorChangedEventArgs> tracks =
            (_, _) => HookTracks(file, item);
        Windows.Foundation.TypedEventHandler<MediaPlaybackItem, Windows.Foundation.Collections.IVectorChangedEventArgs> metadata =
            (_, _) => FromItem(file, () => HideTimedMetadata(item));
        Windows.Foundation.TypedEventHandler<ISingleSelectMediaTrackList, object> selected =
            (_, _) => FromItem(file, () => AudioChanged(file));
        item.AudioTracksChanged += tracks;
        item.VideoTracksChanged += tracks;
        item.TimedMetadataTracksChanged += metadata;
        item.AudioTracks.SelectedIndexChanged += selected;
        file.Detach.Add(() =>
        {
            item.AudioTracksChanged -= tracks;
            item.VideoTracksChanged -= tracks;
            item.TimedMetadataTracksChanged -= metadata;
            item.AudioTracks.SelectedIndexChanged -= selected;
        });
    }

    /// <summary>OpenFailed of every track, hooked once per track; any thread.</summary>
    private void HookTracks(NativeFile file, MediaPlaybackItem item)
    {
        lock (file.Hooked)
        {
            foreach (var track in item.AudioTracks)
            {
                if (file.Hooked.Add($"a{track.Id}"))
                {
                    var key = $"a{track.Id}";
                    Windows.Foundation.TypedEventHandler<AudioTrack, AudioTrackOpenFailedEventArgs> failed = (_, args) =>
                    {
                        var code = args.ExtendedError?.HResult;
                        FromItem(file, () => TrackOpenFailed(file, key, code));
                    };
                    track.OpenFailed += failed;
                    file.Detach.Add(() => track.OpenFailed -= failed);
                }
            }
            foreach (var track in item.VideoTracks)
            {
                if (file.Hooked.Add($"v{track.Id}"))
                {
                    var key = $"v{track.Id}";
                    Windows.Foundation.TypedEventHandler<VideoTrack, VideoTrackOpenFailedEventArgs> failed = (_, args) =>
                    {
                        var code = args.ExtendedError?.HResult;
                        FromItem(file, () => TrackOpenFailed(file, key, code));
                    };
                    track.OpenFailed += failed;
                    file.Detach.Add(() => track.OpenFailed -= failed);
                }
            }
        }
    }

    /// <summary>Closes the file open, if any; nothing more is reported about it.</summary>
    private void Unload()
    {
        _positionTimer.Stop();
        _windowTimer.Stop();
        _deadlineTimer.Stop();
        if (_file is not { } file)
        {
            return;
        }
        _file = null;
        try
        {
            Player.Source = null;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the native engine could not let go of its file: {error.GetType().Name}");
        }
        lock (file.Hooked)
        {
            foreach (var detach in file.Detach)
            {
                try
                {
                    detach();
                }
                catch (Exception error)
                {
                    Trace.TraceError($"Current: unhooking a native engine event failed: {error.GetType().Name}");
                }
            }
            file.Detach.Clear();
        }
        file.Source?.Dispose();
    }

    private void Opened(NativeFile file)
    {
        if (file.Opened || file.Failed || file.Item is not { } item)
        {
            return;
        }
        file.Opened = true;
        Report(file.Generation, new PlatformEngineEvent.Loaded());
        ReportDuration(file);
        HideTimedMetadata(item);
        HookTracks(file, item);
        // The n-th of the file's own audio streams is the n-th audio track, from now on by its Id. A server stream (HLS) carries the
        // one audio track the server was asked for.
        if (!file.Hls)
        {
            var indices = _audioStreams();
            for (var i = 0; i < indices.Count && i < item.AudioTracks.Count; i++)
            {
                file.AudioIds[indices[i]] = item.AudioTracks[i].Id;
            }
        }
        ApplyAudio(file);
        CheckTracks(file);
        if (file.Failed)
        {
            return;
        }
        ReportRates(file);
        Player.PlaybackSession.PlaybackRate = _speed;
        if (file.PendingSeek is { } seek)
        {
            file.PendingSeek = null;
            Seek(file, seek);
        }
        _positionTimer.Start();
        if (_paused)
        {
            Player.Pause();
        }
        else
        {
            Player.Play();
        }
    }

    private void Ended(NativeFile file)
    {
        if (file.Failed || file.Ended)
        {
            return;
        }
        file.Ended = true;
        _positionTimer.Stop();
        _windowTimer.Stop();
        _deadlineTimer.Stop();
        Report(file.Generation, new PlatformEngineEvent.EndOfFile());
    }

    // Facts (UI thread) -----------------------------------------------------------------------------------------------------------

    private void StateChanged(NativeFile file)
    {
        if (!file.Opened || file.Failed || file.Ended)
        {
            return;
        }
        switch (Player.PlaybackSession.PlaybackState)
        {
            case MediaPlaybackState.Playing:
                Report(file.Generation, new PlatformEngineEvent.Buffering(false));
                Report(file.Generation, new PlatformEngineEvent.Paused(false));
                Shown(file);
                break;
            // Only a pause the core asked for: the player passes through Paused on its way to playing after opening (AutoPlay off).
            case MediaPlaybackState.Paused when _paused:
                Report(file.Generation, new PlatformEngineEvent.Buffering(false));
                Report(file.Generation, new PlatformEngineEvent.Paused(true));
                Shown(file);
                break;
            case MediaPlaybackState.Buffering:
                Report(file.Generation, new PlatformEngineEvent.Buffering(true));
                break;
        }
    }

    /// <summary>The file shows a frame after opening: Restarted once, and the decodability window starts.</summary>
    private void Shown(NativeFile file)
    {
        if (file.Started)
        {
            return;
        }
        file.Started = true;
        Report(file.Generation, new PlatformEngineEvent.Restarted());
        _windowTimer.Stop();
        _windowTimer.Start();
    }

    private void SeekCompleted(NativeFile file)
    {
        if (!file.Opened || file.Failed)
        {
            return;
        }
        ReportPosition(file);
        Report(file.Generation, new PlatformEngineEvent.Restarted());
    }

    private void Seek(NativeFile file, TimeSpan position)
    {
        var session = Player.PlaybackSession;
        // Only a seek outside what is buffered waits for a first segment.
        if (file.Hls && BufferedAhead(session, position) is null)
        {
            AwaitSegment(file, position);
        }
        session.Position = position;
    }

    private void AwaitSegment(NativeFile file, TimeSpan from)
    {
        file.AwaitingSegment = true;
        file.AwaitedFrom = from;
        _deadlineTimer.Stop();
        _deadlineTimer.Start();
    }

    private void ReportDuration(NativeFile file)
    {
        var duration = Player.PlaybackSession.NaturalDuration;
        if (file.Opened && duration > TimeSpan.Zero && duration != TimeSpan.MaxValue)
        {
            Report(file.Generation, new PlatformEngineEvent.Duration(duration));
        }
    }

    /// <summary>The position and the buffered range; and an HLS wait ends once the segment has come: the position moves while
    /// playing, or a buffered range reaches past it.</summary>
    private void ReportPosition(NativeFile file)
    {
        var session = Player.PlaybackSession;
        var position = session.Position;
        Report(file.Generation, new PlatformEngineEvent.Position(position));
        var ahead = BufferedAhead(session, position);
        if (ahead is { } buffered)
        {
            Report(file.Generation, new PlatformEngineEvent.Buffered(buffered));
        }
        if (file.AwaitingSegment
            && ((session.PlaybackState == MediaPlaybackState.Playing && position > file.AwaitedFrom) || ahead > TimeSpan.Zero))
        {
            file.AwaitingSegment = false;
            _deadlineTimer.Stop();
        }
    }

    /// <summary>How far past <paramref name="position"/> the buffered range holding it reaches; null when none holds it.</summary>
    private static TimeSpan? BufferedAhead(MediaPlaybackSession session, TimeSpan position)
    {
        foreach (var range in session.GetBufferedRanges())
        {
            if (range.Start <= position && position <= range.End)
            {
                return range.End - position;
            }
        }
        return null;
    }

    private void ReportRates(NativeFile file) => Report(file.Generation,
        new PlatformEngineEvent.RatesSupported(Player.PlaybackSession.IsSupportedPlaybackRateRange(SlowestRate, FastestRate)));

    // Tracks (UI thread) ----------------------------------------------------------------------------------------------------------

    /// <summary>The core's audio choice: a server index found by its track's Id; Auto keeps Windows' own choice. Off is not offered by
    /// Media Foundation's track list (not verified) and keeps the track playing.</summary>
    private static void ApplyAudio(NativeFile file)
    {
        if (file.Item is not { } item || file.Audio is not PlatformTrack.Stream stream || file.Hls)
        {
            return;
        }
        if (!file.AudioIds.TryGetValue(stream.Index, out var id))
        {
            Trace.TraceWarning("Current: the native engine has no track for the audio stream asked for");
            return;
        }
        for (var i = 0; i < item.AudioTracks.Count; i++)
        {
            if (item.AudioTracks[i].Id == id && item.AudioTracks.SelectedIndex != i)
            {
                item.AudioTracks.SelectedIndex = i;
                return;
            }
        }
    }

    private void AudioChanged(NativeFile file)
    {
        if (!file.Opened)
        {
            return;
        }
        CheckTracks(file);
        if (!file.Failed)
        {
            ReportRates(file);
        }
    }

    private void TrackOpenFailed(NativeFile file, string key, int? code)
    {
        var kind = KindOf(code, Kinds.DecoderFailed);
        file.FailedTracks[key] = kind;
        if (key.StartsWith('v') || (SelectedAudio(file) is { } audio && key == $"a{audio.Id}"))
        {
            Fail(file, new PlatformEngineEvent.TrackFailed(kind), code);
        }
    }

    private static AudioTrack? SelectedAudio(NativeFile file) =>
        file.Item is { } item && item.AudioTracks.SelectedIndex is var index && index >= 0 && index < item.AudioTracks.Count
            ? item.AudioTracks[index]
            : null;

    /// <summary>The selected tracks: the video tracks and the selected audio track, by their decoder status and any OpenFailed.</summary>
    private void CheckTracks(NativeFile file)
    {
        if (file.Failed || file.Item is not { } item)
        {
            return;
        }
        foreach (var video in item.VideoTracks)
        {
            var kind = Unplayable(video.SupportInfo.DecoderStatus) ?? file.FailedTracks.GetValueOrDefault($"v{video.Id}");
            if (kind is not null)
            {
                Fail(file, new PlatformEngineEvent.TrackFailed(kind), null);
                return;
            }
        }
        if (SelectedAudio(file) is { } audio)
        {
            var kind = Unplayable(audio.SupportInfo.DecoderStatus) ?? file.FailedTracks.GetValueOrDefault($"a{audio.Id}");
            if (kind is not null)
            {
                Fail(file, new PlatformEngineEvent.TrackFailed(kind), null);
            }
        }
    }

    /// <summary>Anything but <c>FullySupported</c> is a failed start (ADR 0025; W1 §2).</summary>
    private static string? Unplayable(MediaDecoderStatus status) => status switch
    {
        MediaDecoderStatus.FullySupported => null,
        MediaDecoderStatus.UnsupportedSubtype => Kinds.DecoderMissing,
        MediaDecoderStatus.UnsupportedEncoderProperties => Kinds.UnsupportedFormat,
        MediaDecoderStatus.Degraded => Kinds.DecoderDegraded,
        _ => Kinds.DecoderFailed,
    };

    /// <summary>The kind an HRESULT means, else <paramref name="otherwise"/>.</summary>
    private static string KindOf(int? code, string otherwise) => code switch
    {
        Codes.TopoCodecNotFound => Kinds.DecoderMissing,
        Codes.InvalidMediaType or Codes.UnsupportedByteStreamType => Kinds.UnsupportedFormat,
        _ => otherwise,
    };

    /// <summary>Timed metadata tracks (subtitles, chapters) stay hidden until W5 draws them.</summary>
    private static void HideTimedMetadata(MediaPlaybackItem item)
    {
        for (var i = 0; i < item.TimedMetadataTracks.Count; i++)
        {
            item.TimedMetadataTracks.SetPresentationMode((uint)i, TimedMetadataTrackPresentationMode.Disabled);
        }
    }

    /// <summary>The file cannot go on: reported once; the core stops this engine and moves the file on.</summary>
    private void Fail(NativeFile file, PlatformEngineEvent failure, int? code)
    {
        if (file.Failed)
        {
            return;
        }
        file.Failed = true;
        file.AwaitingSegment = false;
        var kind = failure switch
        {
            PlatformEngineEvent.Failed failed => failed.Kind,
            PlatformEngineEvent.TrackFailed failed => $"track {failed.Kind}",
            _ => failure.GetType().Name,
        };
        LastFailure = code is { } hr ? $"{kind}, 0x{hr:X8}" : kind;
        LastFailureCode = code;
        _positionTimer.Stop();
        _deadlineTimer.Stop();
        _windowTimer.Stop();
        Trace.TraceWarning($"Current: the native engine reports {LastFailure}");
        Report(file.Generation, failure);
    }

    /// <summary>Closes the file and the player, after the core closed the player; on the UI thread (handed there from another).</summary>
    public void Dispose()
    {
        if (!_ui.HasThreadAccess)
        {
            _ui.TryEnqueue(Dispose);
            return;
        }
        if (_disposed)
        {
            return;
        }
        Unload();
        _disposed = true;
        Player.Dispose();
    }
}
