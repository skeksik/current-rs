using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>The panel's size in physical pixels (<c>ActualSize</c> × <c>CompositionScale</c>, ADR 0007).</summary>
public sealed record CompositionSize(uint Width, uint Height);

/// <summary>What the monitor the window is on reports (ADR 0007).</summary>
public sealed record DisplayFacts(bool Hdr, float PeakNits, float SdrWhiteNits, double RefreshHz);

/// <summary>The engine this build of the core has.</summary>
public enum PlayerEngineKind
{
    /// <summary>None: no player opens.</summary>
    Unavailable,
    Libmpv,
    /// <summary>The test engine, which opens nothing: test builds only.</summary>
    Test,
}

/// <summary>Why a player could not open or act.</summary>
public abstract record PlayerProblem
{
    private PlayerProblem() { }

    /// <summary>This build has no player engine.</summary>
    public sealed record EngineUnavailable : PlayerProblem;
    /// <summary>Another player is still open (one per process for now).</summary>
    public sealed record Busy : PlayerProblem;
    public sealed record EngineFailed(string Kind) : PlayerProblem;
    /// <summary>The item's server is not signed in.</summary>
    public sealed record NotSignedIn : PlayerProblem;
    public sealed record Closed : PlayerProblem;
}

/// <summary>The player could not open or act.</summary>
public sealed class PlayerException(PlayerProblem problem) : Exception("the player could not do this")
{
    public PlayerProblem Problem { get; } = problem;
}

/// <summary>Why playback cannot start or go on (ADR 0019). No server text and no URL.</summary>
public abstract record PlaybackProblem
{
    private PlaybackProblem() { }

    /// <summary>The server offers no way to play it; <c>Reasons</c> are open names.</summary>
    public sealed record NotPlayable(IReadOnlyList<string> Reasons) : PlaybackProblem;
    /// <summary>The engine failed; <c>Kind</c> is an open name (<c>loading_failed</c>, <c>gateway_playlist_unchecked</c>, ...).</summary>
    public sealed record Engine(string Kind) : PlaybackProblem;
    public sealed record Network : PlaybackProblem;
    /// <summary>A gateway in front of the server wants the user to sign in to it again (ADR 0017).</summary>
    public sealed record GatewaySignInNeeded : PlaybackProblem;
    /// <summary>The server ended the session: sign in again.</summary>
    public sealed record SignedOut : PlaybackProblem;
    public sealed record Server(ushort Status) : PlaybackProblem;
    public sealed record NotFound : PlaybackProblem;
    /// <summary>The server's HLS playlist names another address, so it cannot play with the address's gateway headers.</summary>
    public sealed record PlaylistElsewhere : PlaybackProblem;
}

public enum PlayerStateKind
{
    Idle,
    /// <summary>Asking the server how to play.</summary>
    Preparing,
    Loading,
    Playing,
    Paused,
    Buffering,
    Seeking,
    Ended,
    Failed,
}

/// <summary>How the item reaches the engine, as the UI names it.</summary>
public enum RouteKind
{
    DirectPlay,
    Remux,
    DirectStream,
    Transcode,
}

public enum StreamProtocolKind
{
    Hls,
    Progressive,
}

public sealed record RouteInfo(RouteKind Kind, StreamProtocolKind? Protocol, string? Container, bool VideoCopied, bool AudioCopied);

/// <summary>An audio or subtitle track for the menus, by the server's index.</summary>
public sealed record TrackInfo(
    uint Index, string? Label, string? Language, string? Codec, bool IsDefault, bool IsForced, bool IsHearingImpaired, bool IsExternal);

/// <summary>A subtitle choice.</summary>
public abstract record SubtitleSelection
{
    private SubtitleSelection() { }

    /// <summary>The server's default for the user.</summary>
    public sealed record Default : SubtitleSelection;
    public sealed record Off : SubtitleSelection;
    public sealed record Stream(uint Index) : SubtitleSelection;
}

/// <summary>The most a stream may carry.</summary>
public abstract record QualityChoice
{
    private QualityChoice() { }

    /// <summary>The server's ceiling: only its own limits apply.</summary>
    public sealed record Maximum : QualityChoice;
    public sealed record Bitrate(uint BitsPerSecond) : QualityChoice;
}

/// <summary>A media segment for its skip button; <c>Kind</c> is an open name (<c>intro</c>, <c>outro</c>, ...).</summary>
public sealed record SegmentInfo(string Kind, TimeSpan Start, TimeSpan End);

/// <summary>The "Next episode" card; <c>Countdown</c> in whole seconds while it runs.</summary>
public sealed record NextEpisodeInfo(ItemRef Item, string Name, string? Series, uint? Season, uint? Episode, uint? Countdown);

/// <summary>The player's state: at most 4 changes a second for the position, at once for anything else.</summary>
public sealed record PlayerState(
    PlayerStateKind State,
    PlaybackProblem? Problem,
    ItemRef? Item,
    TimeSpan Position,
    TimeSpan? Duration,
    TimeSpan? Buffered,
    IReadOnlyList<TrackInfo> AudioTracks,
    IReadOnlyList<TrackInfo> SubtitleTracks,
    uint? Audio,
    SubtitleSelection Subtitle,
    RouteInfo? Route,
    SegmentInfo? Segment,
    NextEpisodeInfo? Next,
    QualityChoice Quality,
    byte Volume,
    bool Muted)
{
    public static PlayerState Idle { get; } = new(
        PlayerStateKind.Idle, null, null, TimeSpan.Zero, null, null, [], [], null, new SubtitleSelection.Default(), null, null, null,
        new QualityChoice.Maximum(), 100, false);
}

/// <summary>The playback status as the system media controls name it.</summary>
public enum MediaStatusKind
{
    Stopped,
    Changing,
    Playing,
    Paused,
}

/// <summary>
/// What the system media controls show (ADR 0019): copy it into <c>SystemMediaTransportControls</c> (display type video) and update
/// the timeline when <c>TimelineRevision</c> changes. Previous is a restart (seek to zero). <c>Artwork</c> is encoded image bytes.
/// </summary>
public sealed record NowPlaying(
    ItemRef? Item,
    string? Kind,
    string? Title,
    string? Series,
    uint? Season,
    uint? Episode,
    byte[]? Artwork,
    MediaStatusKind Status,
    TimeSpan Position,
    TimeSpan? Duration,
    TimeSpan? SeekableEnd,
    bool CanPlay,
    bool CanPause,
    bool CanStop,
    bool CanSeek,
    bool CanNext,
    bool CanPrevious,
    ulong TimelineRevision)
{
    public static NowPlaying Nothing { get; } = new(
        null, null, null, null, null, null, null, MediaStatusKind.Stopped, TimeSpan.Zero, null, null, false, false, false, false, false,
        false, 0);
}

public sealed record StreamInfo(
    uint Index, string Kind, string? Codec, string? Profile, string? Language, string? Label, uint? Width, uint? Height, uint? BitDepth,
    float? FrameRate, string? RangeType, uint? Channels, string? ChannelLayout, bool IsExternal);

/// <summary>The version of the item played. No path.</summary>
public sealed record SourceInfo(string? Name, string? Container, ulong? Size, uint? Bitrate, TimeSpan? Duration, IReadOnlyList<StreamInfo> Streams);

/// <summary>What explains a plan, from the user's permissions on the server.</summary>
public abstract record PlanNote
{
    private PlanNote() { }

    public sealed record RemoteBitrateLimit(uint BitsPerSecond) : PlanNote;
    public sealed record TranscodingNotAllowed : PlanNote;
    public sealed record RemuxingNotAllowed : PlanNote;
    public sealed record RemoteSourceTranscodingForced : PlanNote;
}

public sealed record TranscodingInfo(
    bool VideoDirect, bool AudioDirect, string? VideoCodec, string? AudioCodec, string? Container, uint? Bitrate, float? FrameRate,
    double? Completion, uint? Width, uint? Height, uint? AudioChannels, string? HardwareAcceleration, IReadOnlyList<string> Reasons);

public sealed record EngineOutputInfo(
    string? HardwareDecoder, uint? VideoWidth, uint? VideoHeight, string? PixelFormat, string? Primaries, string? Transfer);

/// <summary>The playback information view. <c>Reasons</c> are open names. No URL.</summary>
public sealed record PlaybackInfo(
    RouteInfo? Route, IReadOnlyList<string> Reasons, SourceInfo? Source, IReadOnlyList<PlanNote> Notes, TranscodingInfo? Transcoding,
    EngineOutputInfo Engine);

/// <summary>Settings → Playback (ADR 0019): <c>SkipsIntrosAndCredits</c> is what the switch shows.</summary>
public sealed record PlaybackSettings(IReadOnlyList<string> AutoSkip, bool SkipsIntrosAndCredits);

internal static class PlayerMap
{
    private static TimeSpan Ms(ulong ms) => TimeSpan.FromMilliseconds(ms);

    private static TimeSpan? Ms(ulong? ms) => ms is { } value ? TimeSpan.FromMilliseconds(value) : null;

    public static ulong ToMs(TimeSpan time) => time <= TimeSpan.Zero ? 0 : (ulong)time.TotalMilliseconds;

    public static Ffi.CompositionSize ToFfi(CompositionSize size) => new(size.Width, size.Height);

    public static Ffi.DisplayFacts ToFfi(DisplayFacts facts) => new(facts.Hdr, facts.PeakNits, facts.SdrWhiteNits, facts.RefreshHz);

    public static PlayerEngineKind From(Ffi.PlayerEngineKind engine) => engine switch
    {
        Ffi.PlayerEngineKind.Libmpv => PlayerEngineKind.Libmpv,
        Ffi.PlayerEngineKind.Test => PlayerEngineKind.Test,
        _ => PlayerEngineKind.Unavailable,
    };

    public static PlayerProblem From(Ffi.PlayerProblem problem) => problem switch
    {
        Ffi.PlayerProblem.EngineUnavailable => new PlayerProblem.EngineUnavailable(),
        Ffi.PlayerProblem.Busy => new PlayerProblem.Busy(),
        Ffi.PlayerProblem.EngineFailed failed => new PlayerProblem.EngineFailed(failed.Kind),
        Ffi.PlayerProblem.NotSignedIn => new PlayerProblem.NotSignedIn(),
        _ => new PlayerProblem.Closed(),
    };

    public static PlaybackProblem From(Ffi.PlaybackProblem problem) => problem switch
    {
        Ffi.PlaybackProblem.NotPlayable notPlayable => new PlaybackProblem.NotPlayable(notPlayable.Reasons),
        Ffi.PlaybackProblem.Engine engine => new PlaybackProblem.Engine(engine.Kind),
        Ffi.PlaybackProblem.Network => new PlaybackProblem.Network(),
        Ffi.PlaybackProblem.GatewaySignInNeeded => new PlaybackProblem.GatewaySignInNeeded(),
        Ffi.PlaybackProblem.SignedOut => new PlaybackProblem.SignedOut(),
        Ffi.PlaybackProblem.Server server => new PlaybackProblem.Server(server.Status),
        Ffi.PlaybackProblem.PlaylistElsewhere => new PlaybackProblem.PlaylistElsewhere(),
        _ => new PlaybackProblem.NotFound(),
    };

    private static PlayerStateKind From(Ffi.PlayerStateKind state) => state switch
    {
        Ffi.PlayerStateKind.Preparing => PlayerStateKind.Preparing,
        Ffi.PlayerStateKind.Loading => PlayerStateKind.Loading,
        Ffi.PlayerStateKind.Playing => PlayerStateKind.Playing,
        Ffi.PlayerStateKind.Paused => PlayerStateKind.Paused,
        Ffi.PlayerStateKind.Buffering => PlayerStateKind.Buffering,
        Ffi.PlayerStateKind.Seeking => PlayerStateKind.Seeking,
        Ffi.PlayerStateKind.Ended => PlayerStateKind.Ended,
        Ffi.PlayerStateKind.Failed => PlayerStateKind.Failed,
        _ => PlayerStateKind.Idle,
    };

    public static RouteInfo From(Ffi.RouteInfo route) => new(
        route.Kind switch
        {
            Ffi.RouteKind.Remux => RouteKind.Remux,
            Ffi.RouteKind.DirectStream => RouteKind.DirectStream,
            Ffi.RouteKind.Transcode => RouteKind.Transcode,
            _ => RouteKind.DirectPlay,
        },
        route.Protocol switch
        {
            Ffi.StreamProtocolKind.Hls => StreamProtocolKind.Hls,
            Ffi.StreamProtocolKind.Progressive => StreamProtocolKind.Progressive,
            _ => null,
        },
        route.Container,
        route.VideoCopied,
        route.AudioCopied);

    private static TrackInfo From(Ffi.TrackInfo track) => new(
        track.Index, track.Label, track.Language, track.Codec, track.IsDefault, track.IsForced, track.IsHearingImpaired,
        track.IsExternal);

    public static SubtitleSelection From(Ffi.SubtitleSelection subtitle) => subtitle switch
    {
        Ffi.SubtitleSelection.Off => new SubtitleSelection.Off(),
        Ffi.SubtitleSelection.Stream stream => new SubtitleSelection.Stream(stream.Index),
        _ => new SubtitleSelection.Default(),
    };

    public static Ffi.SubtitleSelection ToFfi(SubtitleSelection subtitle) => subtitle switch
    {
        SubtitleSelection.Off => new Ffi.SubtitleSelection.Off(),
        SubtitleSelection.Stream stream => new Ffi.SubtitleSelection.Stream(stream.Index),
        _ => new Ffi.SubtitleSelection.Default(),
    };

    public static QualityChoice From(Ffi.QualityChoice quality) => quality switch
    {
        Ffi.QualityChoice.Bitrate bitrate => new QualityChoice.Bitrate(bitrate.BitsPerSecond),
        _ => new QualityChoice.Maximum(),
    };

    public static Ffi.QualityChoice ToFfi(QualityChoice quality) => quality switch
    {
        QualityChoice.Bitrate bitrate => new Ffi.QualityChoice.Bitrate(bitrate.BitsPerSecond),
        _ => new Ffi.QualityChoice.Maximum(),
    };

    public static PlayerState From(Ffi.PlayerStateSnapshot state) => new(
        From(state.State),
        state.Problem is { } problem ? From(problem) : null,
        state.Item is { } item ? MapBrowse.From(item) : null,
        Ms(state.PositionMs),
        Ms(state.DurationMs),
        Ms(state.BufferedMs),
        Array.ConvertAll(state.AudioTracks, track => From(track)),
        Array.ConvertAll(state.SubtitleTracks, track => From(track)),
        state.Audio,
        From(state.Subtitle),
        state.Route is { } route ? From(route) : null,
        state.Segment is { } segment ? new SegmentInfo(segment.Kind, Ms(segment.StartMs), Ms(segment.EndMs)) : null,
        state.Next is { } next
            ? new NextEpisodeInfo(MapBrowse.From(next.Item), next.Name, next.Series, next.Season, next.Episode, next.Countdown)
            : null,
        From(state.Quality),
        state.Volume,
        state.Muted);

    public static NowPlaying From(Ffi.NowPlayingSnapshot now) => new(
        now.Item is { } item ? MapBrowse.From(item) : null,
        now.Kind,
        now.Title,
        now.Series,
        now.Season,
        now.Episode,
        now.Artwork,
        now.Status switch
        {
            Ffi.MediaStatusKind.Changing => MediaStatusKind.Changing,
            Ffi.MediaStatusKind.Playing => MediaStatusKind.Playing,
            Ffi.MediaStatusKind.Paused => MediaStatusKind.Paused,
            _ => MediaStatusKind.Stopped,
        },
        Ms(now.PositionMs),
        Ms(now.DurationMs),
        Ms(now.SeekableEndMs),
        now.CanPlay,
        now.CanPause,
        now.CanStop,
        now.CanSeek,
        now.CanNext,
        now.CanPrevious,
        now.TimelineRevision);

    private static StreamInfo From(Ffi.StreamInfo stream) => new(
        stream.Index, stream.Kind, stream.Codec, stream.Profile, stream.Language, stream.Label, stream.Width, stream.Height,
        stream.BitDepth, stream.FrameRate, stream.RangeType, stream.Channels, stream.ChannelLayout, stream.IsExternal);

    private static PlanNote From(Ffi.PlanNoteInfo note) => note switch
    {
        Ffi.PlanNoteInfo.RemoteBitrateLimit limit => new PlanNote.RemoteBitrateLimit(limit.BitsPerSecond),
        Ffi.PlanNoteInfo.TranscodingNotAllowed => new PlanNote.TranscodingNotAllowed(),
        Ffi.PlanNoteInfo.RemuxingNotAllowed => new PlanNote.RemuxingNotAllowed(),
        _ => new PlanNote.RemoteSourceTranscodingForced(),
    };

    public static PlaybackInfo From(Ffi.PlaybackInfoSnapshot info) => new(
        info.Route is { } route ? From(route) : null,
        info.Reasons,
        info.Source is { } source
            ? new SourceInfo(source.Name, source.Container, source.Size, source.Bitrate, Ms(source.DurationMs),
                Array.ConvertAll(source.Streams, stream => From(stream)))
            : null,
        Array.ConvertAll(info.Notes, note => From(note)),
        info.Transcoding is { } t
            ? new TranscodingInfo(t.VideoDirect, t.AudioDirect, t.VideoCodec, t.AudioCodec, t.Container, t.Bitrate, t.FrameRate,
                t.Completion, t.Width, t.Height, t.AudioChannels, t.HardwareAcceleration, t.Reasons)
            : null,
        new EngineOutputInfo(info.Engine.HardwareDecoder, info.Engine.VideoWidth, info.Engine.VideoHeight, info.Engine.PixelFormat,
            info.Engine.Primaries, info.Engine.Transfer));

    public static PlaybackSettings From(Ffi.PlaybackSettingsSnapshot settings) =>
        new(settings.AutoSkip, settings.SkipsIntrosAndCredits);
}
