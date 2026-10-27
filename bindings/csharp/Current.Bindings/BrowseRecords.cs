using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// The browse records (ADR 0018, section 7). Kinds, image types, collection kinds, sort keys and filters are open values: strings passed
// through unchanged, so a value no release knows still arrives. Item ids and image tags cross the bridge; addresses and paths never do.
// Records that hold lists compare them by reference (see Capabilities).

/// <summary>An item on one server: its server and the server's id for it.</summary>
public sealed record ItemRef(ServerRef Server, string Id);

/// <summary>One image of an item, as the item's artwork names it: what <see cref="ImageRequest"/> asks for.</summary>
/// <param name="Kind">The image type: primary, backdrop, logo, thumb, banner, or one the server named.</param>
/// <param name="Index">Which image of that type: 0 for the first.</param>
/// <param name="Tag">The server's tag for this version of the image.</param>
public sealed record ImageRef(ItemRef Item, string Kind, int Index, string Tag);

/// <summary>An image's width to height, reduced.</summary>
public sealed record AspectRatio(int Width, int Height);

/// <summary>One piece of an item's artwork. <c>BlurHash</c> makes a placeholder through <see cref="ImageSource.BlurHash"/>.</summary>
public sealed record Artwork(string Kind, ImageRef Image, AspectRatio? AspectRatio, string? BlurHash);

/// <summary>The user's marks on an item.</summary>
/// <param name="Progress">How far the resume point is, from 0 to 1.</param>
/// <param name="UnplayedCount">For a series or season: how many episodes are unplayed.</param>
public sealed record UserState(
    bool Played,
    TimeSpan? Position,
    float? Progress,
    bool Favourite,
    int? UnplayedCount,
    int PlayCount);

/// <summary>Where an episode belongs. <c>NumberEnd</c> is the last episode of a double episode. <c>Label</c> is the core's label for
/// the numbers: "S1:E2", "S1:E2-E3", "E5" or empty.</summary>
public sealed record EpisodeLinks(
    ItemRef? Series,
    string? SeriesName,
    ItemRef? Season,
    string? SeasonName,
    int? Number,
    int? NumberEnd,
    int? SeasonNumber,
    string Label);

/// <summary>
/// An item as a tile or a row shows it. <c>Kind</c> is the media kind: movie, series, episode..., or one the server named.
/// <c>Poster</c> and <c>Wide</c> are the artwork a poster (2:3) and a wide (16:9) tile show, as the core picks them. <c>TimeLeft</c>
/// is what is left to watch from the resume point, as the core counts it; ignored on the way in.
/// </summary>
public sealed record ItemSummary(
    ItemRef Item,
    string Kind,
    string Name,
    int? Year,
    TimeSpan? Runtime,
    IReadOnlyList<Artwork> Artwork,
    UserState User,
    EpisodeLinks? Episode,
    int? ChildCount,
    Artwork? Poster = null,
    Artwork? Wide = null,
    TimeSpan? TimeLeft = null);

/// <summary>
/// What a Play button starts (ADR 0019): <c>Item</c> (on a series' page, its next episode) from <c>Start</c>. When <c>Resumes</c>, the
/// button says "Resume" and "Play from the beginning" goes with it.
/// </summary>
public sealed record PlayTarget(ItemSummary Item, TimeSpan Start, bool Resumes)
{
    /// <summary>
    /// Play on a tile's item (the core decides): the item from its resume point unless played; null for a kind that does not play by
    /// itself (a series plays its next episode from its page). Pass the item with the marks every list shows now.
    /// </summary>
    public static PlayTarget? ForItem(ItemSummary item)
    {
        ArgumentNullException.ThrowIfNull(item);
        return Ffi.CurrentFfiMethods.PlayTarget(MapBrowse.ToFfi(item)) is { } target ? MapBrowse.From(target) : null;
    }
}

/// <summary>One of the user's libraries, for the app's navigation. <c>Collection</c> is null for a mixed library.</summary>
public sealed record Library(ItemRef Item, string Name, string? Collection, IReadOnlyList<Artwork> Artwork);

/// <summary>A person in an item's cast or crew. <c>Kind</c>: actor, director, writer..., or one the server named.</summary>
public sealed record Person(ItemRef Item, string Name, string? Role, string Kind, Artwork? Image);

public sealed record Chapter(TimeSpan Start, string? Name);

/// <summary>One stream of the default version, as a detail page lists it.</summary>
public sealed record StreamSummary(string Label, string? Language, string? Codec);

/// <summary>The default version of an item's media.</summary>
public sealed record MediaSummary(
    int SourceCount,
    string? Container,
    StreamSummary? Video,
    IReadOnlyList<StreamSummary> Audio,
    IReadOnlyList<StreamSummary> Subtitles);

/// <summary>A series' run: the year it started, the year it ended if that is another year, and whether it still runs.</summary>
public sealed record SeriesRun(int StartYear, int? EndYear, bool Continuing);

/// <summary>One way to sort a library grid: a key (sort_name, date_created...) and the order it goes in.</summary>
public sealed record SortChoice(string Key, SortOrder Order);

/// <summary>What a detail page shows. <c>Run</c> is a series' run, from its dates and status, as the core reads them.</summary>
public sealed record ItemDetails(
    ItemSummary Summary,
    string? OriginalTitle,
    string? Overview,
    IReadOnlyList<string> Taglines,
    IReadOnlyList<string> Genres,
    IReadOnlyList<string> Studios,
    IReadOnlyList<Person> People,
    string? OfficialRating,
    float? CommunityRating,
    float? CriticRating,
    DateOnly? Premiere,
    DateOnly? End,
    string? Status,
    IReadOnlyList<Artwork> Backdrops,
    Artwork? Logo,
    IReadOnlyList<Chapter> Chapters,
    MediaSummary? Media,
    SeriesRun? Run = null);

/// <summary>
/// What a library grid's Sort and Filter buttons offer for the query it shows (<see cref="ItemQuery.Tools"/>): each tooltip says why
/// its button is unavailable, null while it is available.
/// </summary>
public sealed record GridTools(bool SortAvailable, string? SortTooltip, bool FilterAvailable, string? FilterTooltip);

public enum SortOrder
{
    Ascending,
    Descending,
}

/// <summary>What a library grid shows; its library is the parent. Empty lists mean no restriction; no sort key, the server's order.</summary>
public sealed record ItemQuery
{
    /// <summary>Media kinds to include: movie, series...</summary>
    public IReadOnlyList<string> Kinds { get; init; } = [];

    /// <summary>Include items in folders below the library, not only its top level.</summary>
    public bool Recursive { get; init; }

    /// <summary>Sort keys (sort_name, date_created..., or one the server names), most significant first.</summary>
    public IReadOnlyList<string> Sort { get; init; } = [];

    public SortOrder Order { get; init; }

    /// <summary>played, unplayed, favourite, resumable, or one the server names.</summary>
    public IReadOnlyList<string> Filters { get; init; } = [];

    public IReadOnlyList<string> Genres { get; init; } = [];

    public IReadOnlyList<int> Years { get; init; } = [];

    public string? NameStartsWith { get; init; }

    /// <summary>Only items the server finds for this, as its search does: a library's own search box (<see cref="Searching"/>).</summary>
    public string? SearchTerm { get; init; }

    /// <summary>Only items this person is credited on: a person's titles (<see cref="ForPerson"/>).</summary>
    public ItemRef? Person { get; init; }

    /// <summary>
    /// What a grid loads for the text in its search box, from this query, the one it showed before searching (keep it while the user
    /// searches; the core decides): the text trimmed, searched for with no sort and no filter, so the best matches come first; when the
    /// text is blank, this query itself, its sort and filters back.
    /// </summary>
    public ItemQuery Searching(string term)
    {
        ArgumentNullException.ThrowIfNull(term);
        return MapBrowse.From(Ffi.CurrentFfiMethods.LibrarySearch(MapBrowse.ToFfi(this), term));
    }

    /// <summary>What the Sort and Filter buttons of a grid showing this query offer (the core decides): neither while it searches.</summary>
    public GridTools Tools()
    {
        var tools = Ffi.CurrentFfiMethods.LibraryGridTools(MapBrowse.ToFfi(this));
        return new(tools.SortAvailable, tools.SortTooltip, tools.FilterAvailable, tools.FilterTooltip);
    }

    /// <summary>
    /// The line a grid showing this query shows above its items once its total is <paramref name="total"/> (the core decides): while it
    /// searches and the server's search window is full, that only the best matches show; null otherwise.
    /// </summary>
    public string? SearchNote(int total) =>
        Ffi.CurrentFfiMethods.LibrarySearchNote(MapBrowse.ToFfi(this), (uint)Math.Max(total, 0));

    /// <summary>
    /// What <paramref name="library"/>'s grid shows before the user sorts or filters it, by name (the core decides): a film library
    /// its films and a TV library its series, wherever they sit below it; any other library its top level.
    /// </summary>
    public static ItemQuery ForLibrary(Library library)
    {
        ArgumentNullException.ThrowIfNull(library);
        return MapBrowse.From(Ffi.CurrentFfiMethods.LibraryQuery(MapBrowse.ToFfi(library)));
    }

    /// <summary>What opening a person shows (the core decides): the films and series they are credited on, in every library, the
    /// newest first. Open it with <see cref="Browse.OpenItems"/>.</summary>
    public static ItemQuery ForPerson(ItemRef person)
    {
        ArgumentNullException.ThrowIfNull(person);
        return MapBrowse.From(Ffi.CurrentFfiMethods.PersonQuery(MapBrowse.ToFfi(person)));
    }

    /// <summary>The user's favourites in every library, by name (the core decides): with <paramref name="kind"/> (movie, series or
    /// episode, as a Favourites row names it), that kind only, the list a row's header opens; without, Home's Favourites row.</summary>
    public static ItemQuery Favourites(string? kind = null) => MapBrowse.From(Ffi.CurrentFfiMethods.FavouritesQuery(kind));

    /// <summary>The Recently added tab (the core decides): what was added to every library, the newest first; only
    /// <paramref name="kind"/> when given.</summary>
    public static ItemQuery RecentlyAddedEverywhere(string? kind = null) =>
        MapBrowse.From(Ffi.CurrentFfiMethods.RecentlyAddedEverywhereQuery(kind));

    /// <summary>The media kinds the Favorites and Recently added tabs can be narrowed to, in order, after every kind.</summary>
    public static IReadOnlyList<string> ViewKindChoices { get; } = Ffi.CurrentFfiMethods.ViewKindChoices();

    /// <summary>What a Home row of <paramref name="library"/>'s latest items opens (the core decides): its grid, the newest added first.</summary>
    public static ItemQuery RecentlyAdded(Library library)
    {
        ArgumentNullException.ThrowIfNull(library);
        return MapBrowse.From(Ffi.CurrentFfiMethods.RecentlyAddedQuery(MapBrowse.ToFfi(library)));
    }

    /// <summary>The sorts a library grid offers, in the order to list them, each with its order (the core decides). The UI names each key.</summary>
    public static IReadOnlyList<SortChoice> SortChoices { get; } = Array.ConvertAll(
        Ffi.CurrentFfiMethods.LibrarySortChoices(),
        choice => new SortChoice(choice.Key, choice.Order == Ffi.SortOrder.Descending ? SortOrder.Descending : SortOrder.Ascending));

    /// <summary>The filters a library grid offers as one choice, everything (null) first, then unplayed and played (the core decides).</summary>
    public static IReadOnlyList<string?> FilterChoices { get; } = Ffi.CurrentFfiMethods.LibraryFilterChoices();

    /// <summary>The filters a library grid offers as ticks beside that choice, each on or off: favourite (the core decides).</summary>
    public static IReadOnlyList<string> ToggleFilterChoices { get; } = Ffi.CurrentFfiMethods.LibraryToggleFilterChoices();

    /// <summary>This query with <paramref name="choice"/> as its filter choice (null: everything), its ticks kept (the core decides).</summary>
    public ItemQuery WithFilter(string? choice) =>
        MapBrowse.From(Ffi.CurrentFfiMethods.LibraryWithFilterChoice(MapBrowse.ToFfi(this), choice));

    /// <summary>This query with the tick <paramref name="toggle"/> on or off, its filter choice kept (the core decides).</summary>
    public ItemQuery WithToggle(string toggle, bool on)
    {
        ArgumentNullException.ThrowIfNull(toggle);
        return MapBrowse.From(Ffi.CurrentFfiMethods.LibraryWithFilterToggle(MapBrowse.ToFfi(this), toggle, on));
    }

    /// <summary>Every item of <paramref name="kind"/> in every library, by name (the core decides): the page a Favourites row opens.</summary>
    public static ItemQuery OfKind(string kind)
    {
        ArgumentNullException.ThrowIfNull(kind);
        return MapBrowse.From(Ffi.CurrentFfiMethods.KindQuery(kind));
    }
}

/// <summary>
/// One place in a grid. Never null: a slot still loading or whose page failed is a record of its own, so a tile template always has
/// an item to bind to.
/// </summary>
public abstract record Slot
{
    private Slot() { }

    public sealed record Loading : Slot;
    public sealed record Item(ItemSummary Summary) : Slot;
    /// <summary>Its page could not be loaded: the tile shows a retry glyph.</summary>
    public sealed record Failed : Slot;
}

/// <summary>What the message above a grid shows. <c>Total</c> is 0 until the first page answered.</summary>
/// <param name="Failed">Why the last page or refresh failed: one message with Retry.</param>
public sealed record PagedState(int Total, bool Loading, BrowseProblem? Failed);

/// <summary>Whether a detail page or a search is loading, and why it last failed.</summary>
public sealed record LoadState(bool Loading, BrowseProblem? Failed);

/// <summary>What Home's message shows. <c>Offline</c>: the server could not be reached and the rows are the last ones loaded.</summary>
public sealed record HomeState(bool Loading, BrowseProblem? Failed, bool Offline);

public enum HomeRowKind
{
    ContinueWatching,
    NextUp,
    Latest,
    RecentlyAdded,
    Favourites,
    SeenRecently,
}

/// <summary>
/// One row of Home. <c>Key</c> is stable across loads (<see cref="HomeFeed.ObserveRow"/>); <c>Title</c> is the library's name for a
/// Latest row and empty for the others, which the UI names. ToString leaves the title out.
/// </summary>
public sealed record HomeRow(string Key, HomeRowKind Kind, string Title, ItemRef? Library)
{
    public override string ToString() => $"HomeRow {{ Key = {Key}, Kind = {Kind} }}";
}

/// <summary>Why a browse request (a page, a row, a detail page, a played or favourite change) failed. No address, no server message.</summary>
public abstract record BrowseProblem
{
    private BrowseProblem() { }

    /// <summary>The session ended (a sign-out, a removal, a new sign-in): a new <see cref="Browse"/> is needed.</summary>
    public sealed record NotSignedIn : BrowseProblem;
    /// <summary>The server revoked the token: sign in again.</summary>
    public sealed record SessionRevoked : BrowseProblem;
    public sealed record Refused : BrowseProblem;
    /// <summary>The server's answer is being checked: try again in a moment.</summary>
    public sealed record Unconfirmed : BrowseProblem;
    /// <summary>
    /// The session goes on, but the server refused this to the user: the item or its library is gone, or hidden from the user. For a
    /// library grid, the core has read the libraries again by then: a library that left them is gone.
    /// </summary>
    public sealed record NotAvailable : BrowseProblem;
    /// <summary>An authentication gateway in front of the server answered instead (ADR 0017).</summary>
    public sealed record Gateway(GatewayProblem Answer) : BrowseProblem;
    public sealed record Network(NetProblem Problem) : BrowseProblem;
}

/// <summary>Why an image could not be loaded.</summary>
public abstract record ImageProblem
{
    private ImageProblem() { }

    public sealed record Request(BrowseProblem Problem) : ImageProblem;
    /// <summary>The server answered something that is not a JPEG or PNG image.</summary>
    public sealed record NotAnImage : ImageProblem;
    /// <summary>The request cannot be sent: an id or image type that does not fit in a URL path.</summary>
    public sealed record Invalid : ImageProblem;
    public sealed record InvalidBlurHash : ImageProblem;
    /// <summary>A width or height of 0.</summary>
    public sealed record EmptyBox : ImageProblem;
    public sealed record Stopped : ImageProblem;
}

/// <summary>A change to an item's marks.</summary>
public abstract record ItemAction
{
    private ItemAction() { }

    public sealed record SetPlayed(bool Played) : ItemAction;
    public sealed record SetFavourite(bool Favourite) : ItemAction;
}

/// <summary>Something the UI tells the user once, as a message. <c>Seq</c> counts up, so two identical notices can be told apart.</summary>
public abstract record BrowseNotice(ulong Seq)
{
    /// <summary>
    /// The server did not take a played or favourite change: it was rolled back. <c>ItemName</c> is for the message only: ToString
    /// leaves it out, so it never reaches a log.
    /// </summary>
    public sealed record ActionFailed(ulong Seq, ItemAction Action, string? ItemName, BrowseProblem Problem) : BrowseNotice(Seq)
    {
        public override string ToString() => $"ActionFailed {{ Seq = {Seq}, Action = {Action}, Problem = {Problem} }}";
    }
}

/// <summary>Where an image comes from.</summary>
public abstract record ImageSource
{
    private ImageSource() { }

    /// <summary>A server's image of an item: JPEG, or PNG when it needs transparency.</summary>
    public sealed record Server(ImageRef Image) : ImageSource;
    /// <summary>A BlurHash placeholder: a BMP at most 32 pixels on its long side, at the box's ratio.</summary>
    public sealed record BlurHash(string Hash) : ImageSource;
}

/// <summary>
/// An image at the size of the box it fills, in physical pixels: the box's size in view pixels times
/// <c>XamlRoot.RasterizationScale</c>. The core rounds the width up to a size bucket. A width or height of 0 fails with
/// <see cref="ImageProblem.EmptyBox"/>.
/// </summary>
public sealed record ImageRequest(ImageSource Source, int Width, int Height);

internal static class MapBrowse
{
    private static int Count(uint value) => (int)Math.Min(value, int.MaxValue);

    private static int? Count(uint? value) => value is { } v ? Count(v) : null;

    private static TimeSpan? Millis(ulong? value) =>
        value is { } ms ? TimeSpan.FromTicks((long)Math.Min(ms, (ulong)(long.MaxValue / TimeSpan.TicksPerMillisecond)) * TimeSpan.TicksPerMillisecond) : null;

    private static uint Size(int value, string name)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(value, name);
        return (uint)value;
    }

    public static ItemRef From(Ffi.ItemRef item) => new(Map.From(item.Server), item.Id);

    public static Ffi.ItemRef ToFfi(ItemRef item) => new(Map.ToFfi(item.Server), item.Id);

    public static ImageRef From(Ffi.ImageRef image) => new(From(image.Item), image.Kind, Count(image.Index), image.Tag);

    public static Ffi.ImageRef ToFfi(ImageRef image) =>
        new(ToFfi(image.Item), image.Kind, Size(image.Index, nameof(image.Index)), image.Tag);

    public static Artwork From(Ffi.Artwork artwork) => new(
        artwork.Kind,
        From(artwork.Image),
        artwork.AspectRatio is { } ratio ? new AspectRatio(Count(ratio.Width), Count(ratio.Height)) : null,
        artwork.Blurhash);

    public static Ffi.Artwork ToFfi(Artwork artwork) => new(
        artwork.Kind,
        ToFfi(artwork.Image),
        artwork.AspectRatio is { } ratio
            ? new Ffi.AspectRatio(Size(ratio.Width, nameof(ratio.Width)), Size(ratio.Height, nameof(ratio.Height)))
            : null,
        artwork.BlurHash);

    private static Artwork[] From(Ffi.Artwork[] artwork) => Array.ConvertAll(artwork, piece => From(piece));

    public static UserState From(Ffi.UserState state) => new(
        state.Played,
        Millis(state.PositionMs),
        state.Progress,
        state.Favourite,
        Count(state.UnplayedCount),
        Count(state.PlayCount));

    public static EpisodeLinks From(Ffi.EpisodeLinks links) => new(
        links.Series is { } series ? From(series) : null,
        links.SeriesName,
        links.Season is { } season ? From(season) : null,
        links.SeasonName,
        Count(links.Number),
        Count(links.NumberEnd),
        Count(links.SeasonNumber),
        links.Label);

    public static ItemSummary From(Ffi.ItemSummary summary) => new(
        From(summary.Item),
        summary.Kind,
        summary.Name,
        summary.Year,
        Millis(summary.RuntimeMs),
        From(summary.Artwork),
        From(summary.User),
        summary.Episode is { } episode ? From(episode) : null,
        Count(summary.ChildCount),
        summary.Poster is { } poster ? From(poster) : null,
        summary.Wide is { } wide ? From(wide) : null,
        Millis(summary.TimeLeftMs));

    private static ulong? ToMillis(TimeSpan? time) => time is { } value ? (ulong)Math.Max(0, value.TotalMilliseconds) : null;

    private static uint? Size(int? value, string name) => value is { } count ? Size(count, name) : null;

    public static Ffi.UserState ToFfi(UserState state) => new(
        state.Played,
        ToMillis(state.Position),
        state.Progress,
        state.Favourite,
        Size(state.UnplayedCount, nameof(state.UnplayedCount)),
        Size(state.PlayCount, nameof(state.PlayCount)));

    public static Ffi.EpisodeLinks ToFfi(EpisodeLinks links) => new(
        links.Series is { } series ? ToFfi(series) : null,
        links.SeriesName,
        links.Season is { } season ? ToFfi(season) : null,
        links.SeasonName,
        Size(links.Number, nameof(links.Number)),
        Size(links.NumberEnd, nameof(links.NumberEnd)),
        Size(links.SeasonNumber, nameof(links.SeasonNumber)),
        links.Label);

    public static Ffi.ItemSummary ToFfi(ItemSummary summary) => new(
        ToFfi(summary.Item),
        summary.Kind,
        summary.Name,
        summary.Year,
        ToMillis(summary.Runtime),
        Array.ConvertAll(summary.Artwork.ToArray(), piece => ToFfi(piece)),
        ToFfi(summary.User),
        summary.Episode is { } episode ? ToFfi(episode) : null,
        Size(summary.ChildCount, nameof(summary.ChildCount)),
        summary.Poster is { } poster ? ToFfi(poster) : null,
        summary.Wide is { } wide ? ToFfi(wide) : null,
        null);

    public static PlayTarget From(Ffi.PlayTarget target) =>
        new(From(target.Item), Millis(target.StartMs) ?? TimeSpan.Zero, target.Resumes);

    public static Library From(Ffi.Library library) =>
        new(From(library.Item), library.Name, library.Collection, From(library.Artwork));

    public static Ffi.Library ToFfi(Library library) => new(
        ToFfi(library.Item),
        library.Name,
        library.Collection,
        library.Artwork.Select(ToFfi).ToArray());

    private static StreamSummary From(Ffi.StreamSummary stream) => new(stream.Label, stream.Language, stream.Codec);

    private static DateOnly? From(Ffi.CalendarDate? date) =>
        date is { } d ? new DateOnly(d.Year, d.Month, d.Day) : null;

    public static ItemDetails From(Ffi.ItemDetails details) => new(
        From(details.Summary),
        details.OriginalTitle,
        details.Overview,
        details.Taglines,
        details.Genres,
        details.Studios,
        Array.ConvertAll(details.People, person => new Person(
            From(person.Item),
            person.Name,
            person.Role,
            person.Kind,
            person.Image is { } image ? From(image) : null)),
        details.OfficialRating,
        details.CommunityRating,
        details.CriticRating,
        From(details.Premiere),
        From(details.End),
        details.Status,
        From(details.Backdrops),
        details.Logo is { } logo ? From(logo) : null,
        Array.ConvertAll(details.Chapters, chapter => new Chapter(Millis(chapter.StartMs) ?? TimeSpan.Zero, chapter.Name)),
        details.Media is { } media
            ? new MediaSummary(
                Count(media.SourceCount),
                media.Container,
                media.Video is { } video ? From(video) : null,
                Array.ConvertAll(media.Audio, stream => From(stream)),
                Array.ConvertAll(media.Subtitles, stream => From(stream)))
            : null,
        details.Run is { } run ? new SeriesRun(run.StartYear, run.EndYear, run.Continuing) : null);

    public static Ffi.ItemQuery ToFfi(ItemQuery query) => new(
        query.Kinds.ToArray(),
        query.Recursive,
        query.Sort.ToArray(),
        query.Order switch
        {
            SortOrder.Ascending => Ffi.SortOrder.Ascending,
            SortOrder.Descending => Ffi.SortOrder.Descending,
            _ => throw new ArgumentOutOfRangeException(nameof(query), query.Order, "unknown sort order"),
        },
        query.Filters.ToArray(),
        query.Genres.ToArray(),
        query.Years.ToArray(),
        query.NameStartsWith,
        query.SearchTerm,
        query.Person is { } person ? ToFfi(person) : null);

    public static ItemQuery From(Ffi.ItemQuery query) => new()
    {
        Kinds = query.Kinds,
        Recursive = query.Recursive,
        Sort = query.Sort,
        Order = query.Order == Ffi.SortOrder.Descending ? SortOrder.Descending : SortOrder.Ascending,
        Filters = query.Filters,
        Genres = query.Genres,
        Years = query.Years,
        NameStartsWith = query.NameStartsWith,
        SearchTerm = query.SearchTerm,
        Person = query.Person is { } person ? From(person) : null,
    };

    public static Slot From(Ffi.Slot slot) => slot switch
    {
        Ffi.Slot.Loading => new Slot.Loading(),
        Ffi.Slot.Item item => new Slot.Item(From(item.Summary)),
        Ffi.Slot.Failed => new Slot.Failed(),
        _ => throw new ArgumentException($"unknown slot {slot.GetType().Name}", nameof(slot)),
    };

    public static PagedState From(Ffi.PagedState state) =>
        new(Count(state.Total), state.Loading, state.Failed is { } failed ? From(failed) : null);

    public static LoadState From(Ffi.LoadState state) =>
        new(state.Loading, state.Failed is { } failed ? From(failed) : null);

    public static HomeState From(Ffi.HomeState state) =>
        new(state.Loading, state.Failed is { } failed ? From(failed) : null, state.Offline);

    public static HomeRow From(Ffi.HomeRow row) => new(
        row.Key,
        row.Kind switch
        {
            Ffi.HomeRowKind.ContinueWatching => HomeRowKind.ContinueWatching,
            Ffi.HomeRowKind.NextUp => HomeRowKind.NextUp,
            Ffi.HomeRowKind.RecentlyAdded => HomeRowKind.RecentlyAdded,
            Ffi.HomeRowKind.Favourites => HomeRowKind.Favourites,
            Ffi.HomeRowKind.SeenRecently => HomeRowKind.SeenRecently,
            _ => HomeRowKind.Latest,
        },
        row.Title,
        row.Library is { } library ? From(library) : null);

    public static BrowseProblem From(Ffi.BrowseProblem problem) => problem switch
    {
        Ffi.BrowseProblem.NotSignedIn => new BrowseProblem.NotSignedIn(),
        Ffi.BrowseProblem.SessionRevoked => new BrowseProblem.SessionRevoked(),
        Ffi.BrowseProblem.Refused => new BrowseProblem.Refused(),
        Ffi.BrowseProblem.Unconfirmed => new BrowseProblem.Unconfirmed(),
        Ffi.BrowseProblem.NotAvailable => new BrowseProblem.NotAvailable(),
        Ffi.BrowseProblem.Gateway gateway => new BrowseProblem.Gateway(MapProblems.From(gateway.Answer)),
        Ffi.BrowseProblem.Network network => new BrowseProblem.Network(MapProblems.From(network.Problem)),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static ImageProblem From(Ffi.ImageProblem problem) => problem switch
    {
        Ffi.ImageProblem.Request request => new ImageProblem.Request(From(request.Problem)),
        Ffi.ImageProblem.NotAnImage => new ImageProblem.NotAnImage(),
        Ffi.ImageProblem.Invalid => new ImageProblem.Invalid(),
        Ffi.ImageProblem.InvalidBlurHash => new ImageProblem.InvalidBlurHash(),
        Ffi.ImageProblem.EmptyBox => new ImageProblem.EmptyBox(),
        Ffi.ImageProblem.Stopped => new ImageProblem.Stopped(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    private static ItemAction From(Ffi.ItemAction action) => action switch
    {
        Ffi.ItemAction.SetPlayed played => new ItemAction.SetPlayed(played.Played),
        Ffi.ItemAction.SetFavourite favourite => new ItemAction.SetFavourite(favourite.Favourite),
        _ => throw new ArgumentException($"unknown action {action.GetType().Name}", nameof(action)),
    };

    public static BrowseNotice? From(Ffi.BrowseNotice? notice)
    {
        if (notice is null)
        {
            return null;
        }
        return notice.Kind switch
        {
            Ffi.NoticeKind.ActionFailed failed =>
                new BrowseNotice.ActionFailed(notice.Seq, From(failed.Action), failed.ItemName, From(failed.Problem)),
            var kind => throw new ArgumentException($"unknown notice {kind.GetType().Name}", nameof(notice)),
        };
    }

    public static Ffi.ImageRequest ToFfi(ImageRequest request) => new(
        request.Source switch
        {
            ImageSource.Server server => new Ffi.ImageSource.Server(ToFfi(server.Image)),
            ImageSource.BlurHash blurHash => new Ffi.ImageSource.BlurHash(blurHash.Hash),
            _ => throw new ArgumentException($"unknown image source {request.Source?.GetType().Name}", nameof(request)),
        },
        Size(request.Width, nameof(request.Width)),
        Size(request.Height, nameof(request.Height)));
}
