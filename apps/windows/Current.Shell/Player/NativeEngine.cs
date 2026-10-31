using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Windows.Graphics.Imaging;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.Media.Streaming.Adaptive;
using Windows.Storage.Streams;

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
/// <item>Subtitles (W5): the selected timed metadata track is <c>ApplicationPresented</c>, every other one <c>Disabled</c>, and its
/// cues go to our view (<see cref="ISubtitleSink"/>, SubtitleOverlay): text cues and PGS image cues (W1 §6). Switching tracks or
/// turning them off changes presentation modes only, with no reload. The n-th of the file's own subtitle streams by the server's index
/// is the n-th subtitle track sorted by <c>Id</c>, numerically when every <c>Id</c> is a number (W1 §6: the tracks' order changed
/// between opens, each <c>Id</c> stayed; what the Ids look like was not recorded, so this rule is not verified: the engine logs each
/// track's Id, kind and language for the owner's device check, W7; when the Ids are not all numbers, the list's order is used).
/// Subtitle files the core fetched arrive as bytes and are read by <c>TimedTextSource</c> (<see cref="AddSubtitle"/>).</item>
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
        /// <summary>A subtitle file <c>TimedTextSource</c> could not read.</summary>
        public const string SubtitleFailed = "subtitle_failed";
        /// <summary>The selected subtitle stream of the file has no track in Media Foundation (W1 §6: MP4's in-band <c>tx3g</c>
        /// gave none).</summary>
        public const string SubtitleMissing = "subtitle_missing";
        /// <summary>The selected audio stream of the file has no track in Media Foundation once a frame shows (W7, "ALAC in MP4": the
        /// picture moved and no sound came).</summary>
        public const string AudioMissing = "audio_missing";
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
    /// comes before that (ADR 0025, caveat for W4). The segment has come only when the position moves while playing. While paused, a
    /// buffered range reaching past the position stops the clock but not the wait, and the next play starts the deadline again: W7 §8
    /// saw a server stream loaded while paused report a buffered range and then never play, with no deadline left to end it.
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

    /// <summary>
    /// After a subtitle file is added to a file that is already open, this long for its track to appear in the item; otherwise the
    /// file is opened again at the position with the subtitle added first. Microsoft's example adds external timed text before the
    /// item plays ("Media items, playlists, and tracks", learn.microsoft.com, ms.date 2026-08-23, read 2026-10-29); adding it later
    /// is not documented there. 1.5 s is ours.
    /// </summary>
    internal static readonly TimeSpan LateSubtitleWait = TimeSpan.FromSeconds(1.5);

    private readonly DispatcherQueue _ui;
    private readonly Func<IReadOnlyList<uint>> _audioStreams;
    private readonly Func<IReadOnlyList<uint>> _subtitleStreams;
    private readonly ISubtitleSink _subtitles;
    private readonly DispatcherQueueTimer _positionTimer;
    private readonly DispatcherQueueTimer _windowTimer;
    private readonly DispatcherQueueTimer _deadlineTimer;
    /// <summary>Checks, <see cref="LateSubtitleWait"/> after a subtitle file was added while the file played, that its track
    /// shows.</summary>
    private readonly DispatcherQueueTimer _lateCheck;
    private (NativeFile File, uint Index)? _late;
    /// <summary>Gives each cue its own key (two cues may share a start and an empty Id).</summary>
    private readonly System.Runtime.CompilerServices.ConditionalWeakTable<IMediaCue, string> _cueKeys = new();
    /// <summary>1 once an image cue's format was logged (<see cref="ForOverlay"/>).</summary>
    private static int _imageCueLogged;
    private long _cueCount;
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
    /// <param name="subtitleStreams">The same for its own subtitle streams.</param>
    /// <param name="subtitles">Where the cues are shown.</param>
    /// <exception cref="Exception">Windows' player cannot be made (an edition without Media Foundation): open without it.</exception>
    public NativeEngine(DispatcherQueue ui, Func<IReadOnlyList<uint>> audioStreams, Func<IReadOnlyList<uint>> subtitleStreams,
        ISubtitleSink subtitles)
    {
        _ui = ui;
        _audioStreams = audioStreams;
        _subtitleStreams = subtitleStreams;
        _subtitles = subtitles;
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
        Player.PlaybackSession.NaturalVideoSizeChanged += (_, _) => FromPlayer(Player.Source, _ => ReportVideoSize());

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
                CheckSubtitle(file);
            }
        };
        _lateCheck = ui.CreateTimer();
        _lateCheck.Interval = LateSubtitleWait;
        _lateCheck.IsRepeating = false;
        _lateCheck.Tick += (_, _) => LateCheck();
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

    /// <summary>How the last subtitle file was shown: added while the file played, or with the file opened again (the smoke run
    /// prints it).</summary>
    internal string? SubtitlePath { get; private set; }

    /// <summary>For the smoke run: open the file again for every subtitle file, so that path is tested too.</summary>
    internal bool ReopenForSubtitlesForTest { get; set; }

    /// <summary>One load: its source, its item and what is known about it. UI thread only, but <see cref="Hooked"/> (its lock).</summary>
    private sealed class NativeFile(PlatformLoad load)
    {
        public PlatformLoad Load { get; } = load;
        public ulong Generation { get; } = load.Generation;
        public bool Hls { get; } = load.Hls;
        public PlatformTrack Audio { get; set; } = load.Audio;
        public PlatformTrack Subtitle { get; set; } = load.Subtitle;
        /// <summary>Opened again for a subtitle file.</summary>
        public bool Reopened { get; set; }
        /// <summary>The core was told Loaded for this load (by this open or the one before an open again): never twice.</summary>
        public bool LoadedTold { get; set; }
        /// <summary>The subtitle files the core handed over, kept for an open again.</summary>
        public List<PlatformSubtitle> Externals { get; } = [];
        /// <summary>The track each subtitle file became, by its server index.</summary>
        public Dictionary<uint, TimedMetadataTrack> ExternalTracks { get; } = [];
        /// <summary>The tracks whose cues are hooked; a cue's key starts with the track's place here.</summary>
        public List<TimedMetadataTrack> CueTracks { get; } = [];
        /// <summary>The track shown now, if any.</summary>
        public TimedMetadataTrack? Shown { get; set; }
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
        if (!_ui.TryEnqueue(() => _ = OpenAsync(file, null)))
        {
            Report(generation, new PlatformEngineEvent.Failed(Kinds.LoadingFailed));
        }
    }

    /// <summary>A subtitle file the core fetched (ADR 0025, "Subtitles on the native route"): read by <c>TimedTextSource</c> and added
    /// to the open file, or, when that does not show within <see cref="LateSubtitleWait"/>, added to the file opened again at the
    /// position.</summary>
    protected override void AddSubtitle(PlatformSubtitle subtitle) => Post(() =>
    {
        if (_file is not { } file)
        {
            return;
        }
        file.Externals.Add(subtitle);
        if (subtitle.Select)
        {
            file.Subtitle = new PlatformTrack.Stream(subtitle.Index);
        }
        if (!file.Opened || ReopenForSubtitlesForTest)
        {
            Reopen(file);
            return;
        }
        _ = AddLateAsync(file, subtitle);
    });

    protected override void SetPaused(bool paused) => Post(() =>
    {
        _paused = paused;
        if (_file is { Opened: true, Failed: false } file)
        {
            if (paused)
            {
                Player.Pause();
            }
            else
            {
                // A wait that a buffered range held while paused: only playing shows whether the segment has come.
                if (file.AwaitingSegment)
                {
                    _deadlineTimer.Stop();
                    _deadlineTimer.Start();
                }
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
        // The cues shown are of the old position; those of the new one are read again once the seek completes.
        _subtitles.Clear();
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

    protected override void SelectSubtitle(PlatformTrack track) => Post(() =>
    {
        if (_file is not { } file)
        {
            return;
        }
        file.Subtitle = track;
        if (file.Opened)
        {
            ApplySubtitle(file);
        }
    });

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

    /// <param name="again">The file this opens again for its subtitle files (<see cref="Reopen"/>), or null for the core's load.</param>
    private async Task OpenAsync(PlatformLoad load, NativeFile? again)
    {
        if (_disposed)
        {
            return;
        }
        Unload();
        var file = new NativeFile(load) { Reopened = again is not null };
        if (again is not null)
        {
            file.Audio = again.Audio;
            file.Subtitle = again.Subtitle;
            file.LoadedTold = again.LoadedTold;
            file.Externals.AddRange(again.Externals);
        }
        else
        {
            // A load starts playing; the core sends the session's pause after it. Opened again, the file keeps it.
            _paused = false;
            SubtitlePath = null;
        }
        _file = file;
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
            // Subtitle files before the item, as Microsoft's example adds them.
            foreach (var subtitle in file.Externals)
            {
                var text = await TimedTextAsync(file, subtitle);
                if (_file != file)
                {
                    return;
                }
                source.ExternalTimedTextSources.Add(text);
            }
            // Not MediaPlaybackItem(source, startTime): on the owner's PC that item counts Position and NaturalDuration from the
            // start time (a 57 s file started at 25 s read 0:08 of 0:34 eight seconds in), so the core got positions, a duration and
            // a fallback position that were not the file's, and an HLS stream started past the deadline never passed its start
            // (docs/notes/windows-native-hls.md). The start is a seek once the file opens (Opened).
            var item = new MediaPlaybackItem(source);
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
            (_, _) => FromItem(file, () => ApplySubtitle(file));
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
        _lateCheck.Stop();
        _late = null;
        if (_file is not { } file)
        {
            return;
        }
        _file = null;
        _subtitles.Clear();
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
        if (!file.LoadedTold)
        {
            file.LoadedTold = true;
            Report(file.Generation, new PlatformEngineEvent.Loaded());
        }
        ReportDuration(file);
        ReportVideoSize();
        _subtitles.RefreshAppearance();
        LogSubtitleTracks(item);
        ApplySubtitle(file);
        HookTracks(file, item);
        MapAudio(file);
        ApplyAudio(file);
        CheckTracks(file);
        if (file.Failed)
        {
            return;
        }
        ReportRates(file);
        Player.PlaybackSession.PlaybackRate = _speed;
        if (file.PendingSeek is null && file.Load.Start > TimeSpan.Zero)
        {
            // The load's start; an HLS load already waits for its first segment from there (OpenAsync).
            Player.PlaybackSession.Position = file.Load.Start;
        }
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
        // The state as it is now: a change that came before this file counted as opened was dropped, and Pause on a player that is
        // already paused raises none. Without this a file loaded while paused never told the core it was shown, so the core stayed
        // Loading and ignored the play that followed (W7 §8, seen on the owner's PC with a server stream).
        StateChanged(file);
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
        // The tracks have settled once a frame shows: an audio stream asked for with no track now fails it.
        MapAudio(file);
        ApplyAudio(file);
        if (file.Failed)
        {
            return;
        }
        ShowActiveCues(file);
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
        ShowActiveCues(file);
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
    /// playing. Paused, a buffered range past the position stops the deadline until the next play (<see
    /// cref="HlsFirstSegmentDeadline"/>).</summary>
    private void ReportPosition(NativeFile file)
    {
        var session = Player.PlaybackSession;
        var position = session.Position;
        Report(file.Generation, new PlatformEngineEvent.Position(position));
        _subtitles.Expire(position);
        var ahead = BufferedAhead(session, position);
        if (ahead is { } buffered)
        {
            Report(file.Generation, new PlatformEngineEvent.Buffered(buffered));
        }
        if (!file.AwaitingSegment)
        {
            return;
        }
        if (session.PlaybackState == MediaPlaybackState.Playing && position > file.AwaitedFrom)
        {
            file.AwaitingSegment = false;
            _deadlineTimer.Stop();
            Trace.TraceInformation("Current: the native engine's HLS stream plays");
        }
        else if (_paused && ahead > TimeSpan.Zero && _deadlineTimer.IsRunning)
        {
            _deadlineTimer.Stop();
            Trace.TraceInformation("Current: the native engine's HLS stream has data while paused; the deadline waits for play");
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

    /// <summary>The n-th of the file's own audio streams is the n-th audio track, from then on by its Id; a stream already mapped
    /// keeps its track. A server stream (HLS) carries the one audio track the server was asked for. When Media Foundation leaves out a
    /// stream it cannot read, the streams after it would map to the wrong tracks: the counts are logged so the device check sees
    /// it (not handled: the engine is not told each stream's codec).</summary>
    private void MapAudio(NativeFile file)
    {
        if (file.Hls || file.Item is not { } item)
        {
            return;
        }
        var indices = _audioStreams();
        for (var i = 0; i < indices.Count && i < item.AudioTracks.Count; i++)
        {
            file.AudioIds.TryAdd(indices[i], item.AudioTracks[i].Id);
        }
        if (file.Started && indices.Count != item.AudioTracks.Count)
        {
            Trace.TraceWarning(
                $"Current: the native engine has {item.AudioTracks.Count} audio tracks for {indices.Count} audio streams");
        }
    }

    /// <summary>The core's audio choice: a server index found by its track's Id; Auto keeps Windows' own choice. Off is not offered by
    /// Media Foundation's track list (not verified) and keeps the track playing. A stream with no track fails the track once a frame
    /// has shown (the tracks have settled; before that it waits), so the core moves the file to libmpv instead of playing it silently
    /// (W7, "ALAC in MP4"), as a subtitle stream with no track does (<see cref="CheckSubtitle"/>).</summary>
    private void ApplyAudio(NativeFile file)
    {
        if (file.Failed || file.Item is not { } item || file.Audio is not PlatformTrack.Stream stream || file.Hls)
        {
            return;
        }
        if (!file.AudioIds.TryGetValue(stream.Index, out var id))
        {
            Trace.TraceWarning(
                $"Current: the native engine has no track for the audio stream asked for ({item.AudioTracks.Count} audio tracks, {_audioStreams().Count} streams)");
            if (file.Started)
            {
                Fail(file, new PlatformEngineEvent.TrackFailed(Kinds.AudioMissing), null);
            }
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

    private void ReportVideoSize()
    {
        var session = Player.PlaybackSession;
        _subtitles.VideoSize(session.NaturalVideoWidth, session.NaturalVideoHeight);
    }

    // Subtitles (UI thread, but the cue hooks) ------------------------------------------------------------------------------------

    private static bool IsSubtitle(TimedMetadataTrack track) =>
        track.TimedMetadataKind is TimedMetadataKind.Subtitle or TimedMetadataKind.Caption or TimedMetadataKind.ImageSubtitle;

    /// <summary>Each subtitle track's Id, kind and language, for the owner's device check (W7); never its label, which can carry a
    /// title.</summary>
    private static void LogSubtitleTracks(MediaPlaybackItem item)
    {
        for (var i = 0; i < item.TimedMetadataTracks.Count; i++)
        {
            var track = item.TimedMetadataTracks[i];
            if (IsSubtitle(track))
            {
                Trace.TraceInformation(
                    $"Current: native subtitle track {i}: id '{track.Id}', kind {track.TimedMetadataKind}, language '{track.Language}'");
            }
        }
    }

    /// <summary>The track for a server subtitle index: a subtitle file's own track, or the n-th of the file's subtitle tracks by Id
    /// for the n-th of its own subtitle streams (the class summary).</summary>
    private TimedMetadataTrack? TrackFor(NativeFile file, uint index)
    {
        if (file.ExternalTracks.TryGetValue(index, out var external))
        {
            return external;
        }
        if (file.Hls || file.Item is not { } item)
        {
            return null;
        }
        var embedded = Enumerable.Range(0, item.TimedMetadataTracks.Count)
            .Select(i => item.TimedMetadataTracks[i])
            .Where(track => IsSubtitle(track) && !file.ExternalTracks.Values.Any(added => ReferenceEquals(added, track)))
            .ToList();
        if (embedded.All(track => long.TryParse(track.Id, out _)))
        {
            embedded = [.. embedded.OrderBy(track => long.Parse(track.Id, System.Globalization.CultureInfo.InvariantCulture))];
        }
        var streams = _subtitleStreams();
        var place = streams.ToList().IndexOf(index);
        return place >= 0 && place < embedded.Count ? embedded[place] : null;
    }

    /// <summary>The selected track ApplicationPresented and its cues hooked, every other one Disabled: no reload.</summary>
    private void ApplySubtitle(NativeFile file)
    {
        if (file.Item is not { } item)
        {
            return;
        }
        var target = file.Subtitle is PlatformTrack.Stream stream ? TrackFor(file, stream.Index) : null;
        var changed = false;
        for (var i = 0; i < item.TimedMetadataTracks.Count; i++)
        {
            var mode = ReferenceEquals(item.TimedMetadataTracks[i], target)
                ? TimedMetadataTrackPresentationMode.ApplicationPresented
                : TimedMetadataTrackPresentationMode.Disabled;
            if (item.TimedMetadataTracks.GetPresentationMode((uint)i) != mode)
            {
                item.TimedMetadataTracks.SetPresentationMode((uint)i, mode);
                changed = true;
            }
        }
        // Any change of what is presented (a track switch, Off, a track turned on again) starts from nothing: no cue of a track no
        // longer presented stays, and the cues of the one presented now are read again (W7 §13: a cue from that track's previous
        // presentation was shown after Off and back).
        if (changed || !ReferenceEquals(file.Shown, target))
        {
            file.Shown = target;
            _subtitles.Clear();
            if (target is not null)
            {
                HookCues(file, target);
                ShowActiveCues(file);
            }
        }
        else if (target is not null)
        {
            HookCues(file, target);
        }
        else if (file.Started)
        {
            // The tracks have settled once a frame shows; before that the decodability window checks.
            CheckSubtitle(file);
        }
    }

    /// <summary>A selected subtitle stream of the file itself with no track in Media Foundation fails the track: the core moves
    /// the file to libmpv. The counts are logged, nothing else.</summary>
    private void CheckSubtitle(NativeFile file)
    {
        if (file.Failed || file.Item is not { } item || file.Subtitle is not PlatformTrack.Stream stream
            || file.Externals.Any(external => external.Index == stream.Index) || TrackFor(file, stream.Index) is not null)
        {
            return;
        }
        var tracks = Enumerable.Range(0, item.TimedMetadataTracks.Count).Count(i => IsSubtitle(item.TimedMetadataTracks[i]));
        Trace.TraceWarning(
            $"Current: the native engine has no track for the subtitle asked for ({tracks} subtitle tracks, {_subtitleStreams().Count} streams)");
        Fail(file, new PlatformEngineEvent.TrackFailed(Kinds.SubtitleMissing), null);
    }

    /// <summary>A track's CueEntered and CueExited, once per track: the cue is read on Media Foundation's thread and shown on the UI
    /// thread while its track is still the one shown. No cue text is ever logged.</summary>
    private void HookCues(NativeFile file, TimedMetadataTrack track)
    {
        if (file.CueTracks.Any(hooked => ReferenceEquals(hooked, track)))
        {
            return;
        }
        file.CueTracks.Add(track);
        var prefix = file.CueTracks.Count;
        Windows.Foundation.TypedEventHandler<TimedMetadataTrack, MediaCueEventArgs> entered = (_, args) =>
        {
            var key = $"{prefix}:{CueKey(args.Cue)}";
            var start = args.Cue.StartTime;
            var end = EndOf(args.Cue);
            switch (args.Cue)
            {
                case TimedTextCue text:
                    var lines = text.Lines.Select(ToLine).ToList();
                    FromItem(file, () => Show(file, track, start, end, () => _subtitles.ShowText(key, start, end, lines)));
                    break;
                case ImageCue image when image.SoftwareBitmap is { } bitmap:
                    var copy = ForOverlay(bitmap);
                    var placement = new SubtitlePlacement(image.Position.X, image.Position.Y,
                        image.Position.Unit == TimedTextUnit.Percentage, image.Extent.Width, image.Extent.Height,
                        image.Extent.Unit == TimedTextUnit.Percentage);
                    // The overlay owns the copy once shown; every way it is not shown disposes it here.
                    var queued = _ui.TryEnqueue(() =>
                    {
                        if (_disposed || _file != file || !Showable(file, track, start, end))
                        {
                            copy.Dispose();
                            return;
                        }
                        _subtitles.ShowImage(key, start, end, copy, placement);
                    });
                    if (!queued)
                    {
                        copy.Dispose();
                    }
                    break;
            }
        };
        Windows.Foundation.TypedEventHandler<TimedMetadataTrack, MediaCueEventArgs> exited = (_, args) =>
        {
            var key = $"{prefix}:{CueKey(args.Cue)}";
            FromItem(file, () => _subtitles.Hide(key));
        };
        track.CueEntered += entered;
        track.CueExited += exited;
        lock (file.Hooked)
        {
            file.Detach.Add(() =>
            {
                track.CueEntered -= entered;
                track.CueExited -= exited;
            });
        }
    }

    /// <summary>
    /// An image cue's bitmap as the overlay shows it: a copy in premultiplied BGRA, the format a SoftwareBitmap shown in XAML must have
    /// ("A SoftwareBitmap displayed in a XAML app must be in BGRA pixel format with pre-multiplied alpha values", SoftwareBitmapSource
    /// Class, Remarks, learn.microsoft.com, ms.date 2017-02-08, read 2026-10-31). W1 §6 saw PGS cues arrive as <c>Bgra8</c>
    /// premultiplied, yet W7 §11 saw a cue FFmpeg and libmpv draw yellow (255, 215, 0) shown cyan: red and blue swapped, with nothing
    /// in our path but this copy. So a cue that says <c>Bgra8</c> carries its pixels in RGBA order, and its red and blue are swapped
    /// here; any other format is left to <c>SoftwareBitmap.Convert</c>. Why Media Foundation labels it so is not known (not verified:
    /// seen with Windows 11 build 26300 only). The format is logged for the device check.
    /// </summary>
    private static SoftwareBitmap ForOverlay(SoftwareBitmap cue)
    {
        var format = cue.BitmapPixelFormat;
        var copy = SoftwareBitmap.Convert(cue, BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied);
        if (Interlocked.Exchange(ref _imageCueLogged, 1) == 0)
        {
            Trace.TraceInformation($"Current: native image cue {cue.PixelWidth}x{cue.PixelHeight}, {format}, {cue.BitmapAlphaMode}");
        }
        if (format == BitmapPixelFormat.Bgra8)
        {
            SwapRedAndBlue(copy);
        }
        return copy;
    }

    /// <summary>Swaps the first and third byte of every 4-byte pixel. Premultiplied alpha stays right: both channels were multiplied by
    /// the same alpha.</summary>
    private static void SwapRedAndBlue(SoftwareBitmap bitmap)
    {
        var bytes = new byte[4 * bitmap.PixelWidth * bitmap.PixelHeight];
        var buffer = bytes.AsBuffer();
        bitmap.CopyToBuffer(buffer);
        for (var i = 0; i + 3 < bytes.Length; i += 4)
        {
            (bytes[i], bytes[i + 2]) = (bytes[i + 2], bytes[i]);
        }
        bitmap.CopyFromBuffer(buffer);
    }

    /// <summary>The same key for a cue's enter and exit, and a different one for every cue.</summary>
    private string CueKey(IMediaCue cue) =>
        _cueKeys.GetValue(cue, _ => Interlocked.Increment(ref _cueCount).ToString(System.Globalization.CultureInfo.InvariantCulture));

    /// <summary>Shows a cue only while its track is the one presented and the position is within the cue's time: a cue event handed
    /// to the UI thread late (an enter after its exit, or one from the track's previous presentation) shows nothing.</summary>
    private void Show(NativeFile file, TimedMetadataTrack track, TimeSpan start, TimeSpan end, Action show)
    {
        if (Showable(file, track, start, end))
        {
            show();
        }
    }

    private bool Showable(NativeFile file, TimedMetadataTrack track, TimeSpan start, TimeSpan end) =>
        ReferenceEquals(file.Shown, track) && SubtitleOverlay.Holds(start, end, Player.PlaybackSession.Position);

    /// <summary>A cue's end; a PGS cue's Duration is the maximum (W1 §6), which would overflow.</summary>
    private static TimeSpan EndOf(IMediaCue cue) =>
        cue.Duration >= TimeSpan.MaxValue - cue.StartTime ? TimeSpan.MaxValue : cue.StartTime + cue.Duration;

    /// <summary>The text cues active now on the track presented (<c>TimedMetadataTrack.ActiveCues</c>: "the list of media cues in the
    /// TimedMetadataTrack that are currently active", learn.microsoft.com, ms.date 2017-02-08, read 2026-10-31), shown again after the
    /// view was cleared for a track change or a seek, with the keys their exits will have. Image cues are left to their next cue:
    /// converting each active one again costs a full-canvas copy, and PGS cues stay active until the next (W1 §6).</summary>
    /// <remarks>Only once a frame has shown (<see cref="Shown"/> reads them then): <see cref="ApplySubtitle"/> also runs from
    /// <see cref="Opened"/>, before <c>Play</c>, and anything that throws there ends the open without playing (CI's smoke run: the
    /// second play, opened again with its subtitle file, never played after <c>ActiveCues</c> was read from <see cref="Opened"/>;
    /// that <c>ActiveCues</c> threw is inferred, not traced). It never throws: a failure is logged by its type and the cues come
    /// with their next enter.</remarks>
    private void ShowActiveCues(NativeFile file)
    {
        if (!file.Started || file.Shown is not { } track)
        {
            return;
        }
        var prefix = file.CueTracks.FindIndex(hooked => ReferenceEquals(hooked, track)) + 1;
        if (prefix == 0)
        {
            return;
        }
        try
        {
            foreach (var cue in track.ActiveCues)
            {
                if (cue is TimedTextCue text)
                {
                    var start = text.StartTime;
                    var end = EndOf(text);
                    var lines = text.Lines.Select(ToLine).ToList();
                    Show(file, track, start, end, () => _subtitles.ShowText($"{prefix}:{CueKey(text)}", start, end, lines));
                }
            }
        }
        catch (Exception error)
        {
            Trace.TraceWarning($"Current: the native engine could not read the active cues: {error.GetType().Name}");
        }
    }

    /// <summary>A text line as runs: italic and bold from its subformats (W1 §6), the rest left to the caption settings.</summary>
    private static SubtitleLine ToLine(TimedTextLine line)
    {
        var text = line.Text ?? "";
        var italic = new bool[text.Length];
        var bold = new bool[text.Length];
        foreach (var format in line.Subformats)
        {
            var style = format.SubformatStyle;
            var isItalic = style?.FontStyle is TimedTextFontStyle.Italic or TimedTextFontStyle.Oblique;
            var isBold = style?.FontWeight == TimedTextWeight.Bold;
            for (var i = Math.Max(0, format.StartIndex); i < Math.Min(text.Length, format.StartIndex + format.Length); i++)
            {
                italic[i] |= isItalic;
                bold[i] |= isBold;
            }
        }
        var spans = new List<SubtitleSpan>();
        var start = 0;
        for (var i = 1; i <= text.Length; i++)
        {
            if (i == text.Length || italic[i] != italic[start] || bold[i] != bold[start])
            {
                spans.Add(new SubtitleSpan(text[start..i], italic[start], bold[start]));
                start = i;
            }
        }
        return new SubtitleLine(spans);
    }

    /// <summary>A subtitle file's bytes as a <c>TimedTextSource</c>, its Resolved hooked: the track it becomes is the file's, by
    /// its server index; one it cannot read fails the track (the core moves the file on).</summary>
    private async Task<TimedTextSource> TimedTextAsync(NativeFile file, PlatformSubtitle subtitle)
    {
        var stream = new InMemoryRandomAccessStream();
        using (var writer = new DataWriter(stream))
        {
            writer.WriteBytes(subtitle.Data);
            await writer.StoreAsync();
            writer.DetachStream();
        }
        stream.Seek(0);
        var text = string.IsNullOrEmpty(subtitle.Language)
            ? TimedTextSource.CreateFromStream(stream)
            : TimedTextSource.CreateFromStream(stream, subtitle.Language);
        var index = subtitle.Index;
        Windows.Foundation.TypedEventHandler<TimedTextSource, TimedTextSourceResolveResultEventArgs> resolved = (_, args) =>
        {
            var failure = args.Error?.ExtendedError?.HResult;
            var failed = args.Error is not null;
            var track = args.Tracks.FirstOrDefault();
            FromItem(file, () =>
            {
                if (failed || track is null)
                {
                    Fail(file, new PlatformEngineEvent.TrackFailed(Kinds.SubtitleFailed), failure);
                    return;
                }
                file.ExternalTracks[index] = track;
                ApplySubtitle(file);
            });
        };
        text.Resolved += resolved;
        lock (file.Hooked)
        {
            file.Detach.Add(() => text.Resolved -= resolved);
        }
        return text;
    }

    /// <summary>Adds a subtitle file to the open file; when its track is not shown within <see cref="LateSubtitleWait"/>, the file
    /// is opened again with it.</summary>
    private async Task AddLateAsync(NativeFile file, PlatformSubtitle subtitle)
    {
        try
        {
            var text = await TimedTextAsync(file, subtitle);
            if (_file != file || file.Source is not { } source)
            {
                return;
            }
            source.ExternalTimedTextSources.Add(text);
        }
        catch (Exception error)
        {
            Trace.TraceWarning($"Current: a subtitle file could not be added while playing: {error.GetType().Name}");
            Reopen(file);
            return;
        }
        _late = (file, subtitle.Index);
        _lateCheck.Stop();
        _lateCheck.Start();
    }

    /// <summary>The late add's check: its track shows, or the file opens again with it, unless the subtitle was turned off or
    /// another one chosen meanwhile.</summary>
    private void LateCheck()
    {
        if (_late is not { } late)
        {
            return;
        }
        _late = null;
        var (file, index) = late;
        if (_file != file || file.Failed || file.Item is not { } item)
        {
            return;
        }
        var shown = file.ExternalTracks.TryGetValue(index, out var track)
            && Enumerable.Range(0, item.TimedMetadataTracks.Count).Any(i => ReferenceEquals(item.TimedMetadataTracks[i], track));
        if (shown)
        {
            SubtitlePath = "added while playing";
            Trace.TraceInformation("Current: the native engine added a subtitle file while playing");
        }
        else if (file.Subtitle is PlatformTrack.Stream stream && stream.Index == index)
        {
            Reopen(file);
        }
    }

    /// <summary>Opens the file again at the position, its subtitle files added before the item (Microsoft's order): the core is told
    /// Restarted only, as after a seek.</summary>
    private void Reopen(NativeFile file)
    {
        if (_file != file || file.Failed)
        {
            return;
        }
        SubtitlePath = "reopened with the file";
        Trace.TraceInformation("Current: the native engine opens the file again for a subtitle file");
        var position = file.PendingSeek ?? (file.Opened ? Player.PlaybackSession.Position : file.Load.Start);
        _ = OpenAsync(file.Load with { Start = position }, file);
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
