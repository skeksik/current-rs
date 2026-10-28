//! The views across every library (the Favourites and Recently added tabs, and a kind's page: the Movies and Series tabs), which kind
//! tabs show, what a grid opens with, its Filter menu's entries, and how Home's rows and the Favourites tab's rows show and link (owner,
//! 2026-10-26, 2026-10-27 and 2026-10-28; ADR 0018), decided once for every platform (CLAUDE.md: all logic lives in the core). The UIs
//! lay these out, with their own icons.

use current_model::{CollectionKind, ItemFilter, ItemQuery, ItemRef, Library, MediaKind};
use current_store::LibraryChoices;

use super::home::{HomeRow, HomeRowKind};
use super::labels::kind_group;
use super::present::{TileShape, library_filters, library_toggle_filters};
use super::{
    favourites_query, kind_query, library_query, recently_added_everywhere_query, view_kinds,
};

/// A view across every library of a server (owner, 2026-10-26 and 2026-10-27). Closed: the views are Current's own, not a server's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BrowseView {
    /// The Favourites tab: rows of a media kind each ([`FAVOURITE_ROW_KINDS`](super::FAVOURITE_ROW_KINDS)).
    Favourites,
    /// The Recently added tab: a grid, the newest first.
    RecentlyAdded,
    /// Every item of a kind, by name: the Movies and Series tabs ([`KIND_TABS`]), and the page a Favourites row's header opens.
    Everything,
}

/// The kinds with a tab of their own in the navigation, in its order (owner, 2026-10-28): every film and every series of a server,
/// [`BrowseView::Everything`] of that kind. Episodes and collections have none.
pub const KIND_TABS: [MediaKind; 2] = [MediaKind::MOVIE, MediaKind::SERIES];

/// The collection type of a library that holds `kind`'s items, for [`kind_tabs`]: Jellyfin's `movies` and `tvshows`.
fn tab_collection(kind: &MediaKind) -> Option<CollectionKind> {
    if *kind == MediaKind::MOVIE {
        Some(CollectionKind::MOVIES)
    } else if *kind == MediaKind::SERIES {
        Some(CollectionKind::TV_SHOWS)
    } else {
        None
    }
}

/// The kind tabs a server with `libraries` (the user's, [`super::Browse::library_list`]) shows, in [`KIND_TABS`]' order: Movies
/// while a film library is listed, Series while a TV library is (owner, 2026-10-28). Read from the list the core already keeps, so a
/// tab comes and goes with its libraries and costs no request; a library without a collection type (mixed content) shows no tab.
pub fn kind_tabs(libraries: &[Library]) -> Vec<MediaKind> {
    KIND_TABS
        .into_iter()
        .filter(|kind| {
            let collection = tab_collection(kind);
            libraries
                .iter()
                .any(|library| library.collection.is_some() && library.collection == collection)
        })
        .collect()
}

/// Whether a view's grid remembers its choices, as a library's does: a kind tab's page ([`KIND_TABS`]), also when a Favourites row's
/// header opened it. The other views remember nothing.
pub fn view_remembers(view: BrowseView, kind: Option<&MediaKind>) -> bool {
    view == BrowseView::Everything && kind.is_some_and(|kind| KIND_TABS.contains(kind))
}

/// How a view is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewLayout {
    /// Titled rows that scroll sideways, as search results: the Favourites tab.
    Rows,
    /// A grid with Sort, Filter and View, as a library's.
    Grid,
}

/// The query of `view`, narrowed to `kind` when given: the Favourites tab's ([`favourites_query`]), a kind's page ([`kind_query`];
/// with `favourite`, its favourites tick on, as a Favourites row's header opens it) or Recently added
/// ([`recently_added_everywhere_query`], also for [`BrowseView::Everything`] without a kind, which no UI opens).
pub fn view_query(view: BrowseView, kind: Option<&MediaKind>, favourite: bool) -> ItemQuery {
    match (view, kind) {
        (_, Some(kind)) if favourite => favourites_query(Some(kind.clone())),
        (BrowseView::Favourites, kind) => favourites_query(kind.cloned()),
        (BrowseView::Everything, Some(kind)) => kind_query(kind.clone()),
        (_, kind) => recently_added_everywhere_query(kind.cloned()),
    }
}

/// How `view` is laid out: the Favourites tab as rows, any other view (a Favourites kind's own list too) as a grid.
pub fn view_layout(view: BrowseView, kind: Option<&MediaKind>) -> ViewLayout {
    if view == BrowseView::Favourites && kind.is_none() {
        ViewLayout::Rows
    } else {
        ViewLayout::Grid
    }
}

/// What a grid opens with when it is opened anew (not by Back or Forward, which return to where it was left): its query, and its
/// view (wide tiles or posters, with or without titles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridOpening {
    pub query: ItemQuery,
    pub wide: bool,
    pub show_titles: bool,
}

/// The remembered view's tile for wide tiles (`current-store`'s open value).
const WIDE_TILE: &str = "wide";

/// What `library`'s grid opens with: `home_query` as it is when a Home row's header opened it, posters with titles, nothing
/// remembered; else [`library_query`] with what `remembered` holds ([`crate::App::library_choices`]): its sort, its filters if
/// the Filter menu still offers every one of them (one remembered from an older version drops them all), and its view.
pub fn grid_opening(
    library: &Library,
    home_query: Option<ItemQuery>,
    remembered: &LibraryChoices,
) -> GridOpening {
    if let Some(query) = home_query {
        return GridOpening {
            query,
            wide: false,
            show_titles: true,
        };
    }
    with_remembered(library_query(library), remembered)
}

/// `query` with what `remembered` holds: its sort, its filters if the Filter menu still offers every one of them, and its view.
fn with_remembered(mut query: ItemQuery, remembered: &LibraryChoices) -> GridOpening {
    if let Some(sort) = &remembered.sort {
        query.sort = vec![sort.key.clone()];
        query.order = sort.order;
    }
    if let Some(filters) = &remembered.filters
        && filters.iter().all(offered)
    {
        query.filters.clone_from(filters);
    }
    GridOpening {
        query,
        wide: remembered
            .view
            .as_ref()
            .is_some_and(|view| view.tile == WIDE_TILE),
        show_titles: remembered.view.as_ref().is_none_or(|view| view.titles),
    }
}

/// Whether a library grid's Filter menu offers `filter`, as a choice or a tick.
fn offered(filter: &ItemFilter) -> bool {
    library_filters().contains(&Some(filter.clone())) || library_toggle_filters().contains(filter)
}

/// What a view's grid opens with: its query ([`view_query`]), posters with titles; with `remembered` ([`crate::App::view_opening`]),
/// for a view that remembers ([`view_remembers`]) opened from its tab, not from a Favourites row's header, that query with what
/// `remembered` holds, as a library's ([`grid_opening`]). A Favourites row's header opens its query as it is, as a Home row's header
/// opens a library.
pub fn view_opening(
    view: BrowseView,
    kind: Option<&MediaKind>,
    favourite: bool,
    remembered: &LibraryChoices,
) -> GridOpening {
    let query = view_query(view, kind, favourite);
    if favourite || !view_remembers(view, kind) {
        return GridOpening {
            query,
            wide: false,
            show_titles: true,
        };
    }
    with_remembered(query, remembered)
}

/// What a grid's Filter menu lists besides its filters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterMenu {
    /// The media types at the top of the menu, every type (`None`) first ([`kind_choice_name`](super::kind_choice_name)); empty for
    /// none. A view across every library without a kind of its own narrows by type (owner, 2026-10-26); a library or a kind's page
    /// is one type already.
    pub kinds: Vec<Option<MediaKind>>,
    /// Whether the played choice and the favourites tick show ([`library_filters`], [`library_toggle_filters`]): on every grid.
    pub filters: bool,
}

/// The Filter menu of a grid showing `view` (`None`: a library) narrowed to `kind`.
pub fn filter_menu(view: Option<BrowseView>, kind: Option<&MediaKind>) -> FilterMenu {
    let kinds = if view.is_some() && kind.is_none() {
        std::iter::once(None)
            .chain(view_kinds().into_iter().map(Some))
            .collect()
    } else {
        Vec::new()
    };
    FilterMenu {
        kinds,
        filters: true,
    }
}

/// `view`'s title, as its tab or page names it: "Favourites", "Recently added", or a kind's page by its kind ("Movies").
pub fn view_title(view: BrowseView, kind: Option<&MediaKind>) -> String {
    match (view, kind) {
        (BrowseView::Favourites, _) => "Favourites".to_owned(),
        (BrowseView::Everything, Some(kind)) => kind_group(kind),
        _ => "Recently added".to_owned(),
    }
}

/// The tooltip of a row header that opens `view`: "Open Recently added", or a kind's page "Open Movies, favourites ticked".
pub fn view_link_tooltip(view: BrowseView, kind: Option<&MediaKind>) -> String {
    let title = view_title(view, kind);
    if view == BrowseView::Everything {
        format!("Open {title}, favourites ticked")
    } else {
        format!("Open {title}")
    }
}

/// Where a row's header goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowTarget {
    /// The library's grid, the newest added first ([`super::recently_added_query`]).
    Library(ItemRef),
    /// A view across every library: [`view_query`] with these.
    View {
        view: BrowseView,
        kind: Option<MediaKind>,
        favourite: bool,
    },
}

/// A row header that is a link: where it goes and its tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowLink {
    pub target: RowTarget,
    pub tooltip: String,
}

/// What opening an item of a row shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemOpens {
    /// Its detail page.
    #[default]
    Details,
    /// A person's titles: the grid of [`super::person_query`] (owner, 2026-10-27).
    PersonTitles,
}

/// The line under a row once it holds `full_at` items or more: the search asked for no more, so only the best matches show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowNote {
    pub full_at: u32,
    pub text: String,
}

/// How a row of tiles shows: its title, its tiles' shape, and its header's link (`None`: plain text); and, on the search page
/// (amended 2026-10-27, global search filters), whether its tiles are cast cards, show their titles, wrap into a grid, what opening one
/// does, whether it has the item menu (Play, Mark as played, Favourite), and the line once it is full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDisplay {
    pub title: String,
    pub shape: TileShape,
    pub link: Option<RowLink>,
    /// The tiles are cast cards, a round picture and the name, as a details page's cast: people, whatever the view.
    pub cards: bool,
    pub show_titles: bool,
    /// The tiles wrap into a grid rather than scroll sideways.
    pub wraps: bool,
    pub opens: ItemOpens,
    pub item_menu: bool,
    pub note: Option<RowNote>,
}

impl RowDisplay {
    /// A row that scrolls sideways, of tiles with their titles that open their detail pages and have the item menu.
    pub(crate) fn plain(title: String, shape: TileShape, link: Option<RowLink>) -> Self {
        Self {
            title,
            shape,
            link,
            cards: false,
            show_titles: true,
            wraps: false,
            opens: ItemOpens::Details,
            item_menu: true,
            note: None,
        }
    }
}

/// How a Home row shows. Its title: "Continue watching", "Next up", "Recently added", "Favourites", "Seen recently", and a library's
/// Latest row "Recently added in" its name (owner, 2026-10-26). Wide tiles for the rows of episodes (continue watching, next up),
/// posters for the rest. A Latest row's header opens its library, newest first; the Recently added and Favourites rows' open their
/// tabs.
pub fn home_row_display(row: &HomeRow) -> RowDisplay {
    let title = match row.kind {
        HomeRowKind::ContinueWatching => "Continue watching".to_owned(),
        HomeRowKind::NextUp => "Next up".to_owned(),
        HomeRowKind::RecentlyAdded => "Recently added".to_owned(),
        HomeRowKind::Favourites => "Favourites".to_owned(),
        HomeRowKind::SeenRecently => "Seen recently".to_owned(),
        HomeRowKind::Latest if row.title.is_empty() => "Recently added".to_owned(),
        HomeRowKind::Latest => format!("Recently added in {}", row.title),
    };
    let shape = match row.kind {
        HomeRowKind::ContinueWatching | HomeRowKind::NextUp => TileShape::Wide,
        _ => TileShape::Poster,
    };
    let view = |view| {
        Some(RowLink {
            target: RowTarget::View {
                view,
                kind: None,
                favourite: false,
            },
            tooltip: view_link_tooltip(view, None),
        })
    };
    let link = match row.kind {
        HomeRowKind::Latest => row.library.clone().map(|library| RowLink {
            target: RowTarget::Library(library),
            tooltip: if row.title.is_empty() {
                "Open the library, newest first".to_owned()
            } else {
                format!("Open {}, newest first", row.title)
            },
        }),
        HomeRowKind::RecentlyAdded => view(BrowseView::RecentlyAdded),
        HomeRowKind::Favourites => view(BrowseView::Favourites),
        _ => None,
    };
    RowDisplay::plain(title, shape, link)
}

/// How a row of `kind` on the search page shows, a search group or a Favourites tab's row: its kind's name ([`kind_group`]), wide
/// tiles for episodes and videos, posters for the rest. With `link` (the Favourites tab's rows: [`BrowseView::Everything`]), its
/// header opens that view of its kind with the favourites tick on.
pub fn search_group_display(kind: &MediaKind, link: Option<BrowseView>) -> RowDisplay {
    RowDisplay::plain(
        kind_group(kind),
        if *kind == MediaKind::EPISODE || *kind == MediaKind::VIDEO {
            TileShape::Wide
        } else {
            TileShape::Poster
        },
        link.map(|view| RowLink {
            target: RowTarget::View {
                view,
                kind: Some(kind.clone()),
                favourite: true,
            },
            tooltip: view_link_tooltip(view, Some(kind)),
        }),
    )
}

#[cfg(test)]
mod tests {
    use current_model::{
        CollectionKind, ItemId, ServerKey, ServerRef, ServiceType, SortKey, SortOrder,
    };
    use current_store::{RememberedSort, RememberedView};

    use super::super::{FAVOURITE_ROW_KINDS, recently_added_query};
    use super::*;

    fn item(id: &str) -> ItemRef {
        ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new(id),
        )
    }

    fn films() -> Library {
        Library {
            item: item("a"),
            name: "Films".into(),
            collection: Some(CollectionKind::MOVIES),
            artwork: Vec::new(),
        }
    }

    #[test]
    fn each_view_has_its_query() {
        assert_eq!(
            view_query(BrowseView::Favourites, None, false),
            favourites_query(None)
        );
        assert_eq!(
            view_query(BrowseView::Favourites, Some(&MediaKind::MOVIE), false),
            favourites_query(Some(MediaKind::MOVIE))
        );
        assert_eq!(
            view_query(BrowseView::RecentlyAdded, None, false),
            recently_added_everywhere_query(None)
        );
        assert_eq!(
            view_query(BrowseView::RecentlyAdded, Some(&MediaKind::SERIES), false),
            recently_added_everywhere_query(Some(MediaKind::SERIES))
        );
        assert_eq!(
            view_query(BrowseView::Everything, Some(&MediaKind::EPISODE), false),
            kind_query(MediaKind::EPISODE)
        );
        // A Favourites row's header: the kind's page with its favourites tick on.
        let ticked = view_query(BrowseView::Everything, Some(&MediaKind::EPISODE), true);
        assert_eq!(ticked, favourites_query(Some(MediaKind::EPISODE)));
        assert!(ticked.filters.contains(&ItemFilter::FAVOURITE));
        assert_eq!(
            view_query(BrowseView::Everything, None, false),
            recently_added_everywhere_query(None)
        );
    }

    #[test]
    fn only_the_favourites_tab_is_rows() {
        assert_eq!(view_layout(BrowseView::Favourites, None), ViewLayout::Rows);
        assert_eq!(
            view_layout(BrowseView::Favourites, Some(&MediaKind::MOVIE)),
            ViewLayout::Grid
        );
        assert_eq!(
            view_layout(BrowseView::RecentlyAdded, None),
            ViewLayout::Grid
        );
        assert_eq!(
            view_layout(BrowseView::Everything, Some(&MediaKind::MOVIE)),
            ViewLayout::Grid
        );
    }

    fn remembered() -> LibraryChoices {
        LibraryChoices {
            sort: Some(RememberedSort {
                key: SortKey::DATE_CREATED,
                order: SortOrder::Descending,
            }),
            filters: Some(vec![ItemFilter::UNPLAYED, ItemFilter::FAVOURITE]),
            view: Some(RememberedView {
                tile: "wide".into(),
                titles: false,
            }),
        }
    }

    #[test]
    fn a_library_opens_with_what_is_remembered() {
        let opening = grid_opening(&films(), None, &remembered());
        let mut expected = library_query(&films());
        expected.sort = vec![SortKey::DATE_CREATED];
        expected.order = SortOrder::Descending;
        expected.filters = vec![ItemFilter::UNPLAYED, ItemFilter::FAVOURITE];
        assert_eq!(
            opening,
            GridOpening {
                query: expected,
                wide: true,
                show_titles: false
            }
        );
        // Nothing remembered: the first query, posters with titles.
        assert_eq!(
            grid_opening(&films(), None, &LibraryChoices::default()),
            GridOpening {
                query: library_query(&films()),
                wide: false,
                show_titles: true
            }
        );
        // A tile shape a later version names shows posters.
        let later = LibraryChoices {
            view: Some(RememberedView {
                tile: "list".into(),
                titles: true,
            }),
            ..LibraryChoices::default()
        };
        assert!(!grid_opening(&films(), None, &later).wide);
    }

    #[test]
    fn remembered_filters_the_menu_no_longer_offers_are_dropped() {
        let older = LibraryChoices {
            filters: Some(vec![ItemFilter::UNPLAYED, ItemFilter::new("resumable")]),
            ..remembered()
        };
        let opening = grid_opening(&films(), None, &older);
        assert!(opening.query.filters.is_empty());
        // The sort and view still apply.
        assert_eq!(opening.query.sort, vec![SortKey::DATE_CREATED]);
        assert!(opening.wide);
        // "All" remembered: no filter.
        let all = LibraryChoices {
            filters: Some(Vec::new()),
            ..LibraryChoices::default()
        };
        assert!(grid_opening(&films(), None, &all).query.filters.is_empty());
    }

    #[test]
    fn a_home_header_opens_its_own_query_and_ignores_what_is_remembered() {
        let query = recently_added_query(&films());
        assert_eq!(
            grid_opening(&films(), Some(query.clone()), &remembered()),
            GridOpening {
                query,
                wide: false,
                show_titles: true
            }
        );
    }

    #[test]
    fn a_view_opens_with_posters_and_titles() {
        assert_eq!(
            view_opening(
                BrowseView::Everything,
                Some(&MediaKind::MOVIE),
                true,
                &LibraryChoices::default()
            ),
            GridOpening {
                query: favourites_query(Some(MediaKind::MOVIE)),
                wide: false,
                show_titles: true
            }
        );
    }

    #[test]
    fn a_kind_tab_opens_with_what_is_remembered_unless_a_favourites_header_opened_it() {
        let opening = view_opening(
            BrowseView::Everything,
            Some(&MediaKind::SERIES),
            false,
            &remembered(),
        );
        let mut expected = kind_query(MediaKind::SERIES);
        expected.sort = vec![SortKey::DATE_CREATED];
        expected.order = SortOrder::Descending;
        expected.filters = vec![ItemFilter::UNPLAYED, ItemFilter::FAVOURITE];
        assert_eq!(
            opening,
            GridOpening {
                query: expected,
                wide: true,
                show_titles: false
            }
        );
        // Nothing remembered: every series by name, posters with titles.
        assert_eq!(
            view_opening(
                BrowseView::Everything,
                Some(&MediaKind::MOVIE),
                false,
                &LibraryChoices::default()
            ),
            GridOpening {
                query: kind_query(MediaKind::MOVIE),
                wide: false,
                show_titles: true
            }
        );
        // A Favourites row's header: the favourites ticked, nothing remembered.
        assert_eq!(
            view_opening(
                BrowseView::Everything,
                Some(&MediaKind::MOVIE),
                true,
                &remembered()
            ),
            GridOpening {
                query: favourites_query(Some(MediaKind::MOVIE)),
                wide: false,
                show_titles: true
            }
        );
        // Episodes have no tab: their page remembers nothing, nor do the other views.
        for (view, kind) in [
            (BrowseView::Everything, Some(&MediaKind::EPISODE)),
            (BrowseView::RecentlyAdded, None),
            (BrowseView::RecentlyAdded, Some(&MediaKind::MOVIE)),
            (BrowseView::Favourites, Some(&MediaKind::MOVIE)),
        ] {
            assert!(!view_remembers(view, kind));
            assert_eq!(
                view_opening(view, kind, false, &remembered()),
                view_opening(view, kind, false, &LibraryChoices::default())
            );
        }
        assert!(view_remembers(
            BrowseView::Everything,
            Some(&MediaKind::MOVIE)
        ));
        assert!(view_remembers(
            BrowseView::Everything,
            Some(&MediaKind::SERIES)
        ));
    }

    fn library_of(id: &str, collection: Option<CollectionKind>) -> Library {
        Library {
            item: item(id),
            name: id.into(),
            collection,
            artwork: Vec::new(),
        }
    }

    #[test]
    fn a_kind_tab_shows_while_a_library_of_its_type_is_listed() {
        assert!(kind_tabs(&[]).is_empty());
        assert_eq!(kind_tabs(&[films()]), [MediaKind::MOVIE]);
        // In the tabs' order, whatever the libraries' order; one tab for two libraries of a type.
        assert_eq!(
            kind_tabs(&[
                library_of("b", Some(CollectionKind::TV_SHOWS)),
                films(),
                library_of("c", Some(CollectionKind::MOVIES)),
            ]),
            [MediaKind::MOVIE, MediaKind::SERIES]
        );
        // Music, an unknown type, or none (mixed content): no tab.
        assert!(
            kind_tabs(&[
                library_of("d", Some(CollectionKind::MUSIC)),
                library_of("e", Some(CollectionKind::new("a_newer_type"))),
                library_of("f", None),
            ])
            .is_empty()
        );
        for kind in kind_tabs(&[films(), library_of("b", Some(CollectionKind::TV_SHOWS))]) {
            assert_eq!(
                view_layout(BrowseView::Everything, Some(&kind)),
                ViewLayout::Grid
            );
            assert!(
                filter_menu(Some(BrowseView::Everything), Some(&kind))
                    .kinds
                    .is_empty()
            );
        }
        assert_eq!(
            view_title(BrowseView::Everything, Some(&MediaKind::MOVIE)),
            "Movies"
        );
        assert_eq!(
            view_title(BrowseView::Everything, Some(&MediaKind::SERIES)),
            "Series"
        );
    }

    #[test]
    fn only_a_view_without_a_kind_narrows_by_type() {
        let every = filter_menu(Some(BrowseView::RecentlyAdded), None);
        assert_eq!(
            every.kinds,
            [None, Some(MediaKind::MOVIE), Some(MediaKind::SERIES)]
        );
        assert!(every.filters);
        for (view, kind) in [
            (None, None),
            (Some(BrowseView::Everything), Some(&MediaKind::MOVIE)),
            (Some(BrowseView::Favourites), Some(&MediaKind::SERIES)),
        ] {
            let menu = filter_menu(view, kind);
            assert!(menu.kinds.is_empty());
            assert!(menu.filters);
        }
    }

    #[test]
    fn views_are_titled_and_linked() {
        assert_eq!(view_title(BrowseView::Favourites, None), "Favourites");
        assert_eq!(
            view_title(BrowseView::Favourites, Some(&MediaKind::MOVIE)),
            "Favourites"
        );
        assert_eq!(
            view_title(BrowseView::RecentlyAdded, None),
            "Recently added"
        );
        assert_eq!(
            view_title(BrowseView::Everything, Some(&MediaKind::SERIES)),
            "Series"
        );
        assert_eq!(view_title(BrowseView::Everything, None), "Recently added");
        assert_eq!(
            view_link_tooltip(BrowseView::RecentlyAdded, None),
            "Open Recently added"
        );
        assert_eq!(
            view_link_tooltip(BrowseView::Favourites, None),
            "Open Favourites"
        );
        assert_eq!(
            view_link_tooltip(BrowseView::Everything, Some(&MediaKind::EPISODE)),
            "Open Episodes, favourites ticked"
        );
    }

    fn row(kind: HomeRowKind, title: &str, library: Option<ItemRef>) -> HomeRow {
        HomeRow {
            key: "row".into(),
            kind,
            title: title.into(),
            library,
        }
    }

    #[test]
    fn home_rows_are_titled_shaped_and_linked() {
        let plain = |kind, title: &str, shape| {
            let display = home_row_display(&row(kind, "", None));
            assert_eq!(display.title, title);
            assert_eq!(display.shape, shape);
            display.link
        };
        assert_eq!(
            plain(
                HomeRowKind::ContinueWatching,
                "Continue watching",
                TileShape::Wide
            ),
            None
        );
        assert_eq!(plain(HomeRowKind::NextUp, "Next up", TileShape::Wide), None);
        assert_eq!(
            plain(
                HomeRowKind::SeenRecently,
                "Seen recently",
                TileShape::Poster
            ),
            None
        );
        assert_eq!(
            plain(
                HomeRowKind::RecentlyAdded,
                "Recently added",
                TileShape::Poster
            ),
            Some(RowLink {
                target: RowTarget::View {
                    view: BrowseView::RecentlyAdded,
                    kind: None,
                    favourite: false
                },
                tooltip: "Open Recently added".into()
            })
        );
        assert_eq!(
            plain(HomeRowKind::Favourites, "Favourites", TileShape::Poster),
            Some(RowLink {
                target: RowTarget::View {
                    view: BrowseView::Favourites,
                    kind: None,
                    favourite: false
                },
                tooltip: "Open Favourites".into()
            })
        );

        let latest = home_row_display(&row(HomeRowKind::Latest, "Films", Some(item("a"))));
        assert_eq!(latest.title, "Recently added in Films");
        assert_eq!(latest.shape, TileShape::Poster);
        assert_eq!(
            latest.link,
            Some(RowLink {
                target: RowTarget::Library(item("a")),
                tooltip: "Open Films, newest first".into()
            })
        );
        let unnamed = home_row_display(&row(HomeRowKind::Latest, "", Some(item("a"))));
        assert_eq!(unnamed.title, "Recently added");
        assert_eq!(
            unnamed.link.map(|link| link.tooltip).as_deref(),
            Some("Open the library, newest first")
        );
        assert_eq!(
            home_row_display(&row(HomeRowKind::Latest, "Films", None)).link,
            None
        );
    }

    #[test]
    fn search_rows_are_named_by_kind_and_favourites_rows_link() {
        let episodes = search_group_display(&MediaKind::EPISODE, None);
        assert_eq!(episodes.title, "Episodes");
        assert_eq!(episodes.shape, TileShape::Wide);
        assert_eq!(episodes.link, None);
        assert_eq!(
            search_group_display(&MediaKind::VIDEO, None).shape,
            TileShape::Wide
        );
        assert_eq!(
            search_group_display(&MediaKind::MOVIE, None).shape,
            TileShape::Poster
        );
        for kind in FAVOURITE_ROW_KINDS {
            let display = search_group_display(&kind, Some(BrowseView::Everything));
            let link = display.link.unwrap();
            assert_eq!(
                link.target,
                RowTarget::View {
                    view: BrowseView::Everything,
                    kind: Some(kind.clone()),
                    favourite: true
                }
            );
            assert_eq!(
                link.tooltip,
                format!("Open {}, favourites ticked", display.title)
            );
        }
    }
}
