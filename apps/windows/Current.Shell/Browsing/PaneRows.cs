using Current.Bindings;

namespace Current.Shell.Browsing;

/// <summary>
/// The rows of the arrangement editors (ADR 0024: the pane's edit mode, Settings' "Navigation pane" and "Home", the Edit Home
/// dialog), made from the core's arrangement, with the names and icons the pane shows.
/// </summary>
internal static class PaneRows
{
    /// <summary>Why a library starts hidden: the server keeps it out of "My media".</summary>
    public const string HiddenOnServer = "Hidden on the server";

    public static IReadOnlyList<ArrangeRow> Tabs(PaneLayout layout, bool compact = false) =>
        [.. layout.Tabs.Select(tab => new ArrangeRow(
            tab.Key, Views.Title(BrowseView.Everything, tab.Key), !tab.Hidden, KindTabGlyph(tab.Key), compact: compact))];

    public static IReadOnlyList<ArrangeRow> ViewItems(PaneLayout layout, bool compact = false) =>
        [.. layout.Views.Select(view => new ArrangeRow(
            view.Key, Views.Title(ViewOf(view.Key)), !view.Hidden, ViewGlyph(view.Key), compact: compact))];

    public static IReadOnlyList<ArrangeRow> Libraries(PaneLayout layout, bool compact = false) =>
        [.. layout.Libraries.Select(entry => new ArrangeRow(
            entry.Library.Item.Id,
            entry.Library.Name,
            !entry.Hidden,
            LibraryGlyph(entry.Library.Collection),
            entry.HiddenOnServer ? HiddenOnServer : "",
            compact))];

    public static IReadOnlyList<ArrangeRow> HomeRows(IReadOnlyList<HomeRowSetting> rows) =>
        [.. rows.Select(setting => new ArrangeRow(setting.Row.Key, Views.ForHomeRow(setting.Row).Title, !setting.Hidden))];

    /// <summary>The view a key of the Views group names.</summary>
    public static BrowseView ViewOf(string key) =>
        key == PaneEntry.Favourites ? BrowseView.Favourites : BrowseView.RecentlyAdded;

    /// <summary>A view's icon: Segoe Fluent Icons EB51 "Heart" for Favourites, E823 "Recent" for Recently added (MainWindow.xaml).</summary>
    public static string ViewGlyph(string key) => key == PaneEntry.Favourites ? "" : "";

    /// <summary>A kind tab's icon, as the library of its type's (LibraryGlyph): E8B2 Movies, E7F4 TVMonitor.</summary>
    public static string KindTabGlyph(string kind) => kind switch
    {
        "movie" => "",
        "series" => "",
        _ => "",
    };

    /// <summary>
    /// A library's icon by what it holds, from Segoe Fluent Icons ("Segoe Fluent Icons font", learn.microsoft.com, ms.date
    /// 2026-10-13, read 2026-10-25): E8B2 Movies, E7F4 TVMonitor, E8D6 Audio, E714 Video, E91B Photo, E82D Dictionary, E8F1
    /// Library, E8B7 Folder.
    /// </summary>
    public static string LibraryGlyph(string? collection) => collection switch
    {
        "movies" => "",
        "tv_shows" => "",
        "music" => "",
        "music_videos" or "home_videos" => "",
        "photos" => "",
        "books" => "",
        "box_sets" or "playlists" => "",
        _ => "",
    };
}
