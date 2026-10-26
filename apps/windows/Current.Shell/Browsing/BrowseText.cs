using System.Globalization;
using Current.Bindings;

namespace Current.Shell.Browsing;

/// <summary>
/// What the browse screens say: tile lines, the names Narrator reads, group and row titles, and browse problems in plain words. Never
/// an address or a server's message: the records hold none. Item names appear only on screen and in Narrator's names, never in a log.
/// </summary>
internal static class BrowseText
{
    /// <summary>
    /// The first line under a tile: an episode's series (with its own name on the second line), else the item's name. On its series'
    /// own page (<paramref name="withSeries"/> false) an episode shows its own name.
    /// </summary>
    public static string Title(ItemSummary item, bool withSeries = true) =>
        withSeries && item.Kind == "episode" && item.Episode?.SeriesName is { Length: > 0 } series ? series : item.Name;

    /// <summary>The second line under a tile: an episode's number and name, a series' or film's year, or nothing.</summary>
    public static string Subtitle(ItemSummary item, bool withSeries = true)
    {
        if (item.Kind == "episode")
        {
            var number = EpisodeNumber(item);
            var showsSeries = withSeries && item.Episode?.SeriesName is { Length: > 0 };
            return showsSeries ? (number.Length > 0 ? $"{number} · {item.Name}" : item.Name) : number;
        }
        if (item.ChildCount is > 1 && item.Kind == "series")
        {
            return $"{item.ChildCount} new episodes";
        }
        return item.Year?.ToString(CultureInfo.CurrentCulture) ?? "";
    }

    /// <summary>"S1:E2", "S1:E2-E3", "E5" or "": the core's label (EpisodeLinks.Label).</summary>
    public static string EpisodeNumber(ItemSummary item) => item.Episode?.Label ?? "";

    /// <summary>
    /// What Narrator reads for a tile: its lines, then its marks ("played", "favourite", "3 unplayed", "in progress"), so a list of
    /// tiles is told apart and its state heard without opening an item.
    /// </summary>
    public static string TileName(ItemSummary item, bool withSeries = true)
    {
        var parts = new List<string> { Title(item, withSeries) };
        if (Subtitle(item, withSeries) is { Length: > 0 } subtitle)
        {
            parts.Add(subtitle);
        }
        if (item.User.Played)
        {
            parts.Add("played");
        }
        else if (item.TimeLeft is { } left)
        {
            parts.Add($"in progress, {TimeLeft(left)}");
        }
        else if (item.User.Progress is > 0)
        {
            parts.Add("in progress");
        }
        if (item.User.UnplayedCount is > 0 and var unplayed && !item.User.Played)
        {
            parts.Add($"{unplayed} unplayed");
        }
        if (item.User.Favourite)
        {
            parts.Add("favourite");
        }
        return string.Join(", ", parts);
    }

    public const string LoadingTileName = "Loading";

    public const string FailedTileName = "Couldn't load. Use Retry above the grid.";

    /// <summary>A Home row's title: the library's name for Latest, which reads "Recently added in" (owner, 2026-10-26).</summary>
    public static string RowTitle(HomeRow row) => row.Kind switch
    {
        HomeRowKind.ContinueWatching => "Continue watching",
        HomeRowKind.NextUp => "Next up",
        _ => row.Title.Length > 0 ? $"Recently added in {row.Title}" : "Recently added",
    };

    /// <summary>A search group's title, by media kind; a kind Current has no name for shows as the server named it.</summary>
    public static string KindGroup(string kind) => kind switch
    {
        "movie" => "Movies",
        "series" => "Shows",
        "season" => "Seasons",
        "episode" => "Episodes",
        "person" => "People",
        "collection" or "box_set" => "Collections",
        "music_album" => "Albums",
        "music_artist" => "Artists",
        "audio" => "Songs",
        "music_video" => "Music videos",
        "video" => "Videos",
        "folder" => "Folders",
        _ => CultureInfo.CurrentCulture.TextInfo.ToTitleCase(kind.Replace('_', ' ')),
    };

    /// <summary>A person's part when the server gives no role: what they did, or nothing for an actor without one.</summary>
    public static string PersonKind(string kind) => kind switch
    {
        "director" => "Director",
        "writer" => "Writer",
        "producer" => "Producer",
        "composer" => "Composer",
        "guest_star" => "Guest star",
        _ => "",
    };

    /// <summary>"2 h 12 min", "45 min".</summary>
    public static string Runtime(TimeSpan runtime)
    {
        var minutes = (int)Math.Round(runtime.TotalMinutes);
        return minutes >= 60 ? $"{minutes / 60} h {minutes % 60} min" : $"{minutes} min";
    }

    /// <summary>"1h 12m", "45m": the time left on a tile; under a minute counts as one.</summary>
    public static string ShortTime(TimeSpan time)
    {
        var minutes = Math.Max(1, (int)Math.Round(time.TotalMinutes));
        return minutes >= 60 ? $"{minutes / 60}h {minutes % 60}m" : $"{minutes}m";
    }

    /// <summary>"1 h 12 min left", "45 min left", as Narrator reads it; under a minute counts as one.</summary>
    public static string TimeLeft(TimeSpan left) => $"{Runtime(left < TimeSpan.FromMinutes(1) ? TimeSpan.FromMinutes(1) : left)} left";

    public static string ActionFailedTitle(BrowseNotice.ActionFailed failed)
    {
        var name = failed.ItemName is { Length: > 0 } item ? item : "the item";
        return failed.Action switch
        {
            ItemAction.SetPlayed { Played: true } => $"Couldn't mark {name} as played.",
            ItemAction.SetPlayed => $"Couldn't mark {name} as unplayed.",
            ItemAction.SetFavourite { Favourite: true } => $"Couldn't add {name} to favourites.",
            _ => $"Couldn't remove {name} from favourites.",
        };
    }

    /// <summary>The notice shown when the library on screen left its server's list.</summary>
    public const string LibraryGoneTitle = "This library is no longer available";

    public const string LibraryGoneMessage = "It was removed from the server, or you no longer have access to it.";

    public static string Problem(BrowseProblem problem) => problem switch
    {
        BrowseProblem.NotSignedIn => "You're no longer signed in to this server.",
        BrowseProblem.SessionRevoked => "The server ended the session. Sign in again in Settings.",
        BrowseProblem.Refused => "The server refused it.",
        BrowseProblem.Unconfirmed => "The server's answer is being checked. Try again in a moment.",
        BrowseProblem.NotAvailable => "It's no longer available to you on this server.",
        BrowseProblem.Gateway => "The gateway in front of the server wants you to sign in again. Sign in to the gateway in Settings.",
        BrowseProblem.Network network => ProblemText.Net(network.Problem),
        _ => ProblemText.Unexpected,
    };

    /// <summary>A size on disk as File Explorer shows sizes: binary units, at most one decimal (not verified against Explorer).</summary>
    public static string Bytes(long bytes)
    {
        string[] units = ["bytes", "KB", "MB", "GB", "TB"];
        double value = bytes;
        var unit = 0;
        while (value >= 1024 && unit < units.Length - 1)
        {
            value /= 1024;
            unit++;
        }
        return unit == 0
            ? $"{bytes.ToString(CultureInfo.CurrentCulture)} bytes"
            : $"{value.ToString(value < 10 ? "0.#" : "0", CultureInfo.CurrentCulture)} {units[unit]}";
    }
}
