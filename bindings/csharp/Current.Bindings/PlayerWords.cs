using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// What the window measures for the playback info panel's last line (<see cref="PlayerSession.PlaybackInfoLinesAsync"/>): the swap
/// chain panel's size in physical pixels, the view's and the content's sizes in physical pixels (device-independent size × the
/// window's scale), and the window's client size. The content's and the window's are left out when null.
/// </summary>
public sealed record PanelSizes(
    uint CompositionWidth,
    uint CompositionHeight,
    double ViewWidth,
    double ViewHeight,
    double? ContentWidth,
    double? ContentHeight,
    int? WindowWidth,
    int? WindowHeight);

/// <summary>
/// The player's words, on <see cref="CoreHost"/>: its times, shown and spoken, the route and its reasons, the plan's notes, track names,
/// the skip button, the next-episode card, the track menus' fixed items and its problems, all the core's (ADR 0019), so every app
/// says the same. The UI only shows them.
/// </summary>
public static class PlayerWordsExtensions
{
    /// <summary>A time as the player shows it: "1:02:03" from an hour on, else "2:03"; below zero "0:00".</summary>
    public static string TimeLabel(this CoreHost core, TimeSpan time) => core.Core.TimeLabel(PlayerWordsMap.Ms(time));

    /// <summary>A time as a screen reader should say it: "1 hour 2 minutes 3 seconds", "0 seconds".</summary>
    public static string SpokenTime(this CoreHost core, TimeSpan time) => core.Core.SpokenTime(PlayerWordsMap.Ms(time));

    /// <summary>The elapsed time's spoken name: "Time elapsed 12 minutes 3 seconds".</summary>
    public static string ElapsedLabel(this CoreHost core, TimeSpan position) => core.Core.ElapsedLabel(PlayerWordsMap.Ms(position));

    /// <summary>The remaining time's spoken name: "Time remaining 12 minutes 3 seconds".</summary>
    public static string RemainingLabel(this CoreHost core, TimeSpan remaining) =>
        core.Core.RemainingLabel(PlayerWordsMap.Ms(remaining));

    /// <summary>The seek bar's spoken status: "1 minute of 1 hour 30 minutes", or the position alone without a duration.</summary>
    public static string PositionLabel(this CoreHost core, TimeSpan position, TimeSpan? duration) =>
        core.Core.PositionLabel(PlayerWordsMap.Ms(position), duration is { } total ? PlayerWordsMap.Ms(total) : null);

    /// <summary>The route's label: "Direct play", "Remux", "Direct stream", "Transcode".</summary>
    public static string RouteName(this CoreHost core, RouteKind route) => core.Core.RouteName(PlayerWordsMap.ToFfi(route));

    /// <summary>Why the server streams rather than plays the file directly; a reason no release knows is its own name.</summary>
    public static string ReasonText(this CoreHost core, string reason)
    {
        ArgumentNullException.ThrowIfNull(reason);
        return core.Core.ReasonText(reason);
    }

    /// <summary>What the server's permissions say about the plan, its bitrate with the platform's decimal separator.</summary>
    public static string PlanNoteText(this CoreHost core, PlanNote note)
    {
        ArgumentNullException.ThrowIfNull(note);
        return core.Core.PlanNoteText(PlayerWordsMap.ToFfi(note));
    }

    /// <summary>A track as the server names it, else by its language and codec ("eng · AC3"), else its number ("Track 3").</summary>
    public static string TrackName(this CoreHost core, TrackInfo track)
    {
        ArgumentNullException.ThrowIfNull(track);
        return core.Core.TrackName(PlayerWordsMap.ToFfi(track));
    }

    /// <summary>The skip button for a segment kind: "Skip intro", "Skip credits", ...; "Skip" for a kind no release knows.</summary>
    public static string SkipLabel(this CoreHost core, string kind)
    {
        ArgumentNullException.ThrowIfNull(kind);
        return core.Core.SkipLabel(kind);
    }

    /// <summary>The next-episode card's line: "S1:E3 Name", or the name alone.</summary>
    public static string EpisodeLine(this CoreHost core, NextEpisodeInfo next)
    {
        ArgumentNullException.ThrowIfNull(next);
        return core.Core.EpisodeLine(next.Number, next.Name);
    }

    /// <summary>The next-episode card's countdown: "Starts in 5 seconds".</summary>
    public static string CountdownText(this CoreHost core, uint seconds) => core.Core.CountdownText(seconds);

    /// <summary>The audio menu's only item when the item has no audio track: "No audio tracks".</summary>
    public static string NoAudioTracksLabel(this CoreHost core) => core.Core.NoAudioTracksLabel();

    /// <summary>The subtitles menu's first item: "Off".</summary>
    public static string SubtitlesOffLabel(this CoreHost core) => core.Core.SubtitlesOffLabel();

    /// <summary>Why playback stopped, in plain words.</summary>
    public static string PlaybackProblemText(this CoreHost core, PlaybackProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return core.Core.PlaybackProblemText(PlayerWordsMap.ToFfi(problem));
    }

    /// <summary>Why the player couldn't open or act, in plain words.</summary>
    public static string PlayerProblemText(this CoreHost core, PlayerProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return core.Core.PlayerProblemText(PlayerWordsMap.ToFfi(problem));
    }
}

internal static class PlayerWordsMap
{
    /// <summary>Whole milliseconds, toward zero (below zero reads as zero in the core).</summary>
    public static long Ms(TimeSpan time) => (long)time.TotalMilliseconds;

    public static Ffi.PanelSizes ToFfi(PanelSizes panel) => new(
        panel.CompositionWidth, panel.CompositionHeight, panel.ViewWidth, panel.ViewHeight, panel.ContentWidth, panel.ContentHeight,
        panel.WindowWidth, panel.WindowHeight);

    public static Ffi.RouteKind ToFfi(RouteKind route) => route switch
    {
        RouteKind.Remux => Ffi.RouteKind.Remux,
        RouteKind.DirectStream => Ffi.RouteKind.DirectStream,
        RouteKind.Transcode => Ffi.RouteKind.Transcode,
        _ => Ffi.RouteKind.DirectPlay,
    };

    public static Ffi.PlanNoteInfo ToFfi(PlanNote note) => note switch
    {
        PlanNote.RemoteBitrateLimit limit => new Ffi.PlanNoteInfo.RemoteBitrateLimit(limit.BitsPerSecond),
        PlanNote.TranscodingNotAllowed => new Ffi.PlanNoteInfo.TranscodingNotAllowed(),
        PlanNote.RemuxingNotAllowed => new Ffi.PlanNoteInfo.RemuxingNotAllowed(),
        _ => new Ffi.PlanNoteInfo.RemoteSourceTranscodingForced(),
    };

    public static Ffi.TrackInfo ToFfi(TrackInfo track) => new(
        track.Index, track.Label, track.Language, track.Codec, track.IsDefault, track.IsForced, track.IsHearingImpaired,
        track.IsExternal);

    public static Ffi.PlaybackProblem ToFfi(PlaybackProblem problem) => problem switch
    {
        PlaybackProblem.NotPlayable notPlayable => new Ffi.PlaybackProblem.NotPlayable(notPlayable.Reasons.ToArray()),
        PlaybackProblem.Engine engine => new Ffi.PlaybackProblem.Engine(engine.Kind),
        PlaybackProblem.Network => new Ffi.PlaybackProblem.Network(),
        PlaybackProblem.GatewaySignInNeeded => new Ffi.PlaybackProblem.GatewaySignInNeeded(),
        PlaybackProblem.SignedOut => new Ffi.PlaybackProblem.SignedOut(),
        PlaybackProblem.Server server => new Ffi.PlaybackProblem.Server(server.Status),
        PlaybackProblem.PlaylistElsewhere => new Ffi.PlaybackProblem.PlaylistElsewhere(),
        PlaybackProblem.LocalRelay => new Ffi.PlaybackProblem.LocalRelay(),
        _ => new Ffi.PlaybackProblem.NotFound(),
    };

    public static Ffi.PlayerProblem ToFfi(PlayerProblem problem) => problem switch
    {
        PlayerProblem.EngineUnavailable => new Ffi.PlayerProblem.EngineUnavailable(),
        PlayerProblem.Busy => new Ffi.PlayerProblem.Busy(),
        PlayerProblem.EngineFailed failed => new Ffi.PlayerProblem.EngineFailed(failed.Kind),
        PlayerProblem.NotSignedIn => new Ffi.PlayerProblem.NotSignedIn(),
        PlayerProblem.Replaced => new Ffi.PlayerProblem.Replaced(),
        _ => new Ffi.PlayerProblem.Closed(),
    };
}
