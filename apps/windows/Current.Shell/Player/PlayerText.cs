using System.Globalization;
using Current.Bindings;

namespace Current.Shell.Player;

/// <summary>
/// What the player says: times, the route and its reasons, track and quality names, the skip button, the next-episode card, and
/// problems in plain words. Never an address or a server's message: the records hold none. When the controls show and what they
/// offer is the core's (PlayerState); this only words it.
/// </summary>
internal static class PlayerText
{
    /// <summary>"1:02:03" from an hour on, else "2:03", as Media Player writes its times (owner's PC, 2026-10-25).</summary>
    public static string Time(TimeSpan time)
    {
        if (time < TimeSpan.Zero)
        {
            time = TimeSpan.Zero;
        }
        return time.TotalHours >= 1
            ? time.ToString(@"h\:mm\:ss", CultureInfo.CurrentCulture)
            : time.ToString(@"m\:ss", CultureInfo.CurrentCulture);
    }

    /// <summary>"1 hour 2 minutes 3 seconds", for Narrator: "1:02:03" is read as a time of day.</summary>
    public static string Spoken(TimeSpan time)
    {
        if (time < TimeSpan.Zero)
        {
            time = TimeSpan.Zero;
        }
        var parts = new List<string>();
        void Add(int count, string unit)
        {
            if (count > 0)
            {
                parts.Add(count == 1 ? $"1 {unit}" : $"{count} {unit}s");
            }
        }
        Add((int)time.TotalHours, "hour");
        Add(time.Minutes, "minute");
        Add(time.Seconds, "second");
        return parts.Count > 0 ? string.Join(" ", parts) : "0 seconds";
    }

    /// <summary>Where the position is, as the seek bar's status and the elapsed time's name ("Time elapsed 12 minutes 3 seconds", as
    /// Media Player names its own, owner's PC, 2026-10-25).</summary>
    public static string Elapsed(TimeSpan position) => $"Time elapsed {Spoken(position)}";

    public static string Remaining(TimeSpan remaining) => $"Time remaining {Spoken(remaining)}";

    /// <summary>The seek bar's status: where it is of how long.</summary>
    public static string Position(TimeSpan position, TimeSpan? duration) =>
        duration is { } total ? $"{Spoken(position)} of {Spoken(total)}" : Spoken(position);

    /// <summary>ADR 0019's labels: a remux and a direct stream are transcode jobs that copy streams.</summary>
    public static string Route(RouteKind route) => route switch
    {
        RouteKind.DirectPlay => "Direct play",
        RouteKind.Remux => "Remux",
        RouteKind.DirectStream => "Direct stream",
        _ => "Transcode",
    };

    /// <summary>Why the server streams rather than plays the file directly, in plain words; a reason no release knows is shown by the
    /// server's own name (ADR 0019, "Reasons").</summary>
    public static string Reason(string reason) => reason switch
    {
        "container_not_supported" => "The player can't read this container.",
        "video_codec_not_supported" => "The player can't decode this video codec.",
        "audio_codec_not_supported" => "The player can't decode this audio codec.",
        "subtitle_codec_not_supported" => "The player can't show these subtitles, so they're burned into the video.",
        "audio_is_external" => "The audio is in a separate file.",
        "secondary_audio_not_supported" => "This audio track can't be played directly.",
        "stream_count_exceeds_limit" => "The file has more streams than the player takes.",
        "video_profile_not_supported" => "The player can't decode this video profile.",
        "video_range_type_not_supported" => "The player can't show this video's dynamic range.",
        "video_codec_tag_not_supported" => "The player can't read this video codec tag.",
        "video_level_not_supported" => "The player can't decode this video level.",
        "video_resolution_not_supported" => "The video's resolution is too high for the player.",
        "video_bit_depth_not_supported" => "The player can't decode this video bit depth.",
        "video_framerate_not_supported" => "The player can't play this frame rate.",
        "video_rotation_not_supported" => "The player can't rotate this video.",
        "ref_frames_not_supported" => "The video uses more reference frames than the player takes.",
        "anamorphic_video_not_supported" => "The player can't show anamorphic video.",
        "interlaced_video_not_supported" => "The player can't show interlaced video.",
        "audio_channels_not_supported" => "The player can't play this many audio channels.",
        "audio_profile_not_supported" => "The player can't decode this audio profile.",
        "audio_sample_rate_not_supported" => "The player can't play this audio sample rate.",
        "audio_bit_depth_not_supported" => "The player can't play this audio bit depth.",
        "container_bitrate_exceeds_limit" => "The file's bitrate is above the quality limit.",
        "video_bitrate_not_supported" => "The video's bitrate is above the quality limit.",
        "audio_bitrate_not_supported" => "The audio's bitrate is above the quality limit.",
        "unknown_video_stream_info" => "The server doesn't know enough about the video.",
        "unknown_audio_stream_info" => "The server doesn't know enough about the audio.",
        "direct_play_error" => "Direct play failed on the server.",
        "quality_limit" => "The quality you chose is below the file's.",
        "fallback_after_engine_error" => "The player couldn't play the file directly.",
        "user_chose_transcode" => "You chose a converted stream.",
        _ => reason,
    };

    /// <summary>What the server's permissions say about the plan.</summary>
    public static string Note(PlanNote note) => note switch
    {
        PlanNote.RemoteBitrateLimit limit => $"The server limits streaming outside your network to {Bitrate(limit.BitsPerSecond)}.",
        PlanNote.TranscodingNotAllowed => "The server doesn't allow you to convert video.",
        PlanNote.RemuxingNotAllowed => "The server doesn't allow you to repackage video.",
        _ => "The server converts every stream from a remote source.",
    };

    /// <summary>"40 Mbit/s", "1.5 Mbit/s"; below 1 Mbit/s in kbit/s, "720 kbit/s", which one decimal of Mbit/s rounded to "0.7".</summary>
    public static string Bitrate(uint bitsPerSecond) => bitsPerSecond < 1_000_000
        ? $"{(bitsPerSecond / 1_000.0).ToString("0", CultureInfo.CurrentCulture)} kbit/s"
        : $"{(bitsPerSecond / 1_000_000.0).ToString("0.#", CultureInfo.CurrentCulture)} Mbit/s";

    /// <summary>
    /// A quality menu entry: "Original · 1080p · 5.2 Mbit/s", "720p · 4 Mbit/s", or without the resolution when Settings hides it ("Show
    /// resolution in quality menu"); the parts the core does not know are left out.
    /// </summary>
    public static string Quality(QualityOption option, bool withResolution)
    {
        var parts = new List<string>();
        if (option.Quality is QualityChoice.Maximum)
        {
            parts.Add("Original");
        }
        if (withResolution && option.Resolution is { Length: > 0 } resolution)
        {
            parts.Add(resolution);
        }
        if (option.Bitrate is { } bitrate)
        {
            parts.Add(Bitrate(bitrate));
        }
        return string.Join(" · ", parts);
    }

    /// <summary>A track as the server names it (its display title), else by its language and codec, else its number.</summary>
    public static string Track(TrackInfo track)
    {
        if (track.Label is { Length: > 0 } label)
        {
            return label;
        }
        var parts = new[] { track.Language, track.Codec?.ToUpperInvariant() }.Where(part => !string.IsNullOrEmpty(part));
        var text = string.Join(" · ", parts);
        return text.Length > 0 ? text : $"Track {track.Index}";
    }

    /// <summary>The skip button for the segment under the position (ADR 0019, choice 8); "Skip" for a kind no release knows.</summary>
    public static string Skip(string kind) => kind switch
    {
        "intro" => "Skip intro",
        "outro" => "Skip credits",
        "recap" => "Skip recap",
        "preview" => "Skip preview",
        "commercial" => "Skip ad",
        _ => "Skip",
    };

    /// <summary>"S1:E3 Name", or the name alone.</summary>
    public static string Episode(NextEpisodeInfo next) => next.Number.Length > 0 ? $"{next.Number} {next.Name}" : next.Name;

    public static string Countdown(uint seconds) => seconds == 1 ? "Starts in 1 second" : $"Starts in {seconds} seconds";

    /// <summary>Why playback stopped, in plain words (ADR 0019, "Errors").</summary>
    public static string Problem(PlaybackProblem problem) => problem switch
    {
        PlaybackProblem.NotPlayable => "The server offers no way to play this item.",
        PlaybackProblem.Engine { Kind: "unsupported_format" } => "The player can't play this file's format.",
        PlaybackProblem.Engine { Kind: "decoder_failed" } => "The player couldn't decode this file.",
        PlaybackProblem.Engine { Kind: "engine_gone" } => "The player stopped working. Go back and play the item again.",
        PlaybackProblem.Engine { Kind: "gateway_playlist_unchecked" } or PlaybackProblem.PlaylistElsewhere =>
            "The server's stream points to another address, so Current won't send it your gateway sign-in.",
        PlaybackProblem.LocalRelay => "Current could not start its local stream connection.",
        PlaybackProblem.Engine => "The player couldn't open the stream.",
        PlaybackProblem.Network => "The server can't be reached. Check your connection and try again.",
        PlaybackProblem.GatewaySignInNeeded => "Sign in to your server's gateway again in Settings, then play again.",
        PlaybackProblem.SignedOut => "The server ended your session. Sign in again in Settings.",
        PlaybackProblem.Server server => $"The server couldn't play this item (error {server.Status}).",
        _ => "The server can't find this item anymore.",
    };

    /// <summary>Why the player couldn't open or act.</summary>
    public static string Player(PlayerProblem problem) => problem switch
    {
        PlayerProblem.EngineUnavailable => "This build of Current has no player.",
        PlayerProblem.Busy => "Another video is still closing. Try again in a moment.",
        PlayerProblem.EngineFailed => "The player couldn't start.",
        PlayerProblem.NotSignedIn => "Sign in to this item's server first.",
        _ => ProblemText.Unexpected,
    };
}
