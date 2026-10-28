//! The views across every library, what a grid opens with, its Filter menu, how Home's and the search page's rows show and link, and
//! the browse screens' names and times, across the bridge (`current-app`'s rules and texts): the UIs lay them out with their own
//! icons. Media kinds, sort keys and filters cross as their open names (ADR 0005).

use std::time::Duration;

use current_model as model;

use crate::{Core, CoreError, ServerRef};

use super::{HomeRow, HomeRowKind, ItemQuery, ItemRef, Library};

/// A view across every library of a server. Current's own, so closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum BrowseView {
    /// The Favourites tab: a row per media kind.
    Favourites,
    /// The Recently added tab: a grid, the newest first.
    RecentlyAdded,
    /// Every item of a kind, by name: the page a Favourites row's header opens.
    Everything,
}

impl From<BrowseView> for current_app::BrowseView {
    fn from(view: BrowseView) -> Self {
        match view {
            BrowseView::Favourites => Self::Favourites,
            BrowseView::RecentlyAdded => Self::RecentlyAdded,
            BrowseView::Everything => Self::Everything,
        }
    }
}

impl From<current_app::BrowseView> for BrowseView {
    fn from(view: current_app::BrowseView) -> Self {
        match view {
            current_app::BrowseView::Favourites => Self::Favourites,
            current_app::BrowseView::RecentlyAdded => Self::RecentlyAdded,
            current_app::BrowseView::Everything => Self::Everything,
        }
    }
}

/// How a view is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewLayout {
    /// Titled rows that scroll sideways, as search results: the Favourites tab.
    Rows,
    /// A grid with Sort, Filter and View, as a library's.
    Grid,
}

/// The shape of a row's tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TileShape {
    /// 2:3, for films and series.
    Poster,
    /// 16:9, for episodes and videos.
    Wide,
}

/// What a grid opens with when it is opened anew (not by Back or Forward): its query, and its view.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct GridOpening {
    pub query: ItemQuery,
    pub wide: bool,
    pub show_titles: bool,
}

impl From<current_app::GridOpening> for GridOpening {
    fn from(opening: current_app::GridOpening) -> Self {
        Self {
            query: (&opening.query).into(),
            wide: opening.wide,
            show_titles: opening.show_titles,
        }
    }
}

/// What a grid's Filter menu lists besides its filters.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FilterMenu {
    /// The media types at the top of the menu, every type (`None`) first ([`kind_choice_name`]); empty for none.
    pub kinds: Vec<Option<String>>,
    /// Whether the played choice and the favourites tick show.
    pub filters: bool,
}

/// Where a row's header goes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RowTarget {
    /// The library's grid, the newest added first ([`super::recently_added_query`]). Not `Library { library }`: a C# class cannot
    /// hold a member of its own name.
    OpenLibrary { library: ItemRef },
    /// A view across every library: [`view_query`] with these.
    OpenView {
        view: BrowseView,
        kind: Option<String>,
        favourite: bool,
    },
}

/// A row header that is a link: where it goes and its tooltip.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RowLink {
    pub target: RowTarget,
    pub tooltip: String,
}

/// What opening an item of a row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ItemOpens {
    /// Its detail page.
    Details,
    /// A person's titles: [`person_query`](super::person_query) in a grid titled with their name.
    PersonTitles,
}

/// The line under a row once it holds `full_at` items or more.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RowNote {
    pub full_at: u32,
    pub text: String,
}

/// How a row of tiles shows: its title, its tiles' shape, and its header's link (`None`: plain text); whether its tiles are cast
/// cards (people), show their titles and wrap into a grid; what opening one does; whether it has the item menu; and the line once
/// it is full.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RowDisplay {
    pub title: String,
    pub shape: TileShape,
    pub link: Option<RowLink>,
    pub cards: bool,
    pub show_titles: bool,
    pub wraps: bool,
    pub opens: ItemOpens,
    pub item_menu: bool,
    pub note: Option<RowNote>,
}

impl From<current_app::RowDisplay> for RowDisplay {
    fn from(display: current_app::RowDisplay) -> Self {
        Self {
            title: display.title,
            shape: match display.shape {
                current_app::TileShape::Poster => TileShape::Poster,
                current_app::TileShape::Wide => TileShape::Wide,
            },
            link: display.link.map(|link| RowLink {
                target: match link.target {
                    current_app::RowTarget::Library(library) => RowTarget::OpenLibrary {
                        library: (&library).into(),
                    },
                    current_app::RowTarget::View {
                        view,
                        kind,
                        favourite,
                    } => RowTarget::OpenView {
                        view: view.into(),
                        kind: kind.map(|kind| kind.as_str().to_owned()),
                        favourite,
                    },
                },
                tooltip: link.tooltip,
            }),
            cards: display.cards,
            show_titles: display.show_titles,
            wraps: display.wraps,
            opens: match display.opens {
                current_app::ItemOpens::Details => ItemOpens::Details,
                current_app::ItemOpens::PersonTitles => ItemOpens::PersonTitles,
            },
            item_menu: display.item_menu,
            note: display.note.map(|note| RowNote {
                full_at: note.full_at,
                text: note.text,
            }),
        }
    }
}

fn kind_of(kind: Option<String>) -> Option<model::MediaKind> {
    kind.map(model::MediaKind::new)
}

/// The query of `view`, narrowed to `kind` (a media kind) when given: the Favourites tab's, a kind's page (with `favourite`, its
/// favourites tick on, as a Favourites row's header opens it), or Recently added.
#[uniffi::export]
pub fn view_query(view: BrowseView, kind: Option<String>, favourite: bool) -> ItemQuery {
    (&current_app::view_query(view.into(), kind_of(kind).as_ref(), favourite)).into()
}

/// How `view` narrowed to `kind` is laid out: the Favourites tab as rows, any other view as a grid.
#[uniffi::export]
pub fn view_layout(view: BrowseView, kind: Option<String>) -> ViewLayout {
    match current_app::view_layout(view.into(), kind_of(kind).as_ref()) {
        current_app::ViewLayout::Rows => ViewLayout::Rows,
        current_app::ViewLayout::Grid => ViewLayout::Grid,
    }
}

/// What a view's grid opens with when nothing is remembered: [`view_query`], posters with titles. A kind tab's page opens with
/// [`Core::view_opening`], which adds what it remembers.
#[uniffi::export]
pub fn view_opening(view: BrowseView, kind: Option<String>, favourite: bool) -> GridOpening {
    current_app::view_opening(
        view.into(),
        kind_of(kind).as_ref(),
        favourite,
        &current_store::LibraryChoices::default(),
    )
    .into()
}

/// The media kinds whose tab a server with `libraries` (its list, in the user's order) shows in the navigation, in the tabs' order
/// (owner, 2026-10-28): `movie` while a film library is listed, `series` while a TV library is. Each tab is [`BrowseView::Everything`]
/// of its kind, titled by [`view_title`].
#[uniffi::export]
pub fn kind_tabs(libraries: Vec<Library>) -> Vec<String> {
    let libraries: Vec<model::Library> = libraries.iter().map(Into::into).collect();
    current_app::kind_tabs(&libraries)
        .into_iter()
        .map(|kind| kind.as_str().to_owned())
        .collect()
}

/// Whether `view` narrowed to `kind` remembers its choices, as a library does: a kind tab's page, also when a Favourites row's header
/// opened it.
#[uniffi::export]
pub fn view_remembers(view: BrowseView, kind: Option<String>) -> bool {
    current_app::view_remembers(view.into(), kind_of(kind).as_ref())
}

/// The Filter menu of a grid showing `view` (`None`: a library) narrowed to `kind`: the media types only for a view without a kind.
#[uniffi::export]
pub fn grid_filter_menu(view: Option<BrowseView>, kind: Option<String>) -> FilterMenu {
    let menu = current_app::filter_menu(view.map(Into::into), kind_of(kind).as_ref());
    FilterMenu {
        kinds: menu
            .kinds
            .into_iter()
            .map(|kind| kind.map(|kind| kind.as_str().to_owned()))
            .collect(),
        filters: menu.filters,
    }
}

/// `view`'s title, as its tab or page names it: "Favourites", "Recently added", or a kind's page by its kind.
#[uniffi::export]
pub fn view_title(view: BrowseView, kind: Option<String>) -> String {
    current_app::view_title(view.into(), kind_of(kind).as_ref())
}

/// The tooltip of a row header that opens `view` narrowed to `kind`.
#[uniffi::export]
pub fn view_link_tooltip(view: BrowseView, kind: Option<String>) -> String {
    current_app::view_link_tooltip(view.into(), kind_of(kind).as_ref())
}

/// How a Home row shows: its title, its tiles' shape and its header's link.
#[uniffi::export]
pub fn home_row_display(row: HomeRow) -> RowDisplay {
    let row = current_app::HomeRow {
        key: row.key,
        kind: match row.kind {
            HomeRowKind::ContinueWatching => current_app::HomeRowKind::ContinueWatching,
            HomeRowKind::NextUp => current_app::HomeRowKind::NextUp,
            HomeRowKind::Latest => current_app::HomeRowKind::Latest,
            HomeRowKind::RecentlyAdded => current_app::HomeRowKind::RecentlyAdded,
            HomeRowKind::Favourites => current_app::HomeRowKind::Favourites,
            HomeRowKind::SeenRecently => current_app::HomeRowKind::SeenRecently,
        },
        title: row.title,
        library: row.library.as_ref().map(Into::into),
    };
    current_app::home_row_display(&row).into()
}

/// How a row of `kind` on the search page shows (a search group, or with `link` a Favourites tab's row, whose header opens that
/// view of its kind with the favourites tick on).
#[uniffi::export]
pub fn search_group_display(kind: String, link: Option<BrowseView>) -> RowDisplay {
    current_app::search_group_display(&model::MediaKind::new(kind), link.map(Into::into)).into()
}

/// A media kind's group name ("Movies", "Series"); a kind Current has no name for shows as the server named it, each word
/// capitalised.
#[uniffi::export]
pub fn kind_group_name(kind: String) -> String {
    current_app::kind_group(&model::MediaKind::new(kind))
}

/// A media type in a view's Filter menu: "All types" for every type (`None`), else its name.
#[uniffi::export]
pub fn kind_choice_name(kind: Option<String>) -> String {
    current_app::kind_choice_name(kind_of(kind).as_ref())
}

/// A sort key's name in a library grid's Sort menu; one Current has no name for shows as it is.
#[uniffi::export]
pub fn library_sort_name(key: String) -> String {
    current_app::library_sort_name(&model::SortKey::new(key))
}

/// A filter's name in a library grid's Filter menu: "All" for no filter (`None`); one Current has no name for shows as it is.
#[uniffi::export]
pub fn library_filter_name(filter: Option<String>) -> String {
    current_app::library_filter_name(filter.map(model::ItemFilter::new).as_ref())
}

/// An item's runtime, from milliseconds: "2 h 12 min", "45 min".
#[uniffi::export]
pub fn runtime_label(runtime_ms: u64) -> String {
    current_app::runtime_label(Duration::from_millis(runtime_ms))
}

/// The time left on a tile, short, from milliseconds: "1h 12m", "45m"; under a minute counts as one.
#[uniffi::export]
pub fn time_left_short(left_ms: u64) -> String {
    current_app::time_left_short(Duration::from_millis(left_ms))
}

/// The time left in words, as a screen reader reads a tile, from milliseconds: "1 h 12 min left"; under a minute counts as one.
#[uniffi::export]
pub fn time_left_label(left_ms: u64) -> String {
    current_app::time_left_label(Duration::from_millis(left_ms))
}

/// The Favourites tab's line above its `rows` rows: none while it shows rows or is `loading`; else "Couldn't load favourites." when it
/// `failed` (the UI adds why), else "No favourites yet".
#[uniffi::export]
pub fn favourites_status(rows: u32, loading: bool, failed: bool) -> Option<String> {
    current_app::favourites_status(usize::try_from(rows).unwrap_or(usize::MAX), loading, failed)
        .map(str::to_owned)
}

#[uniffi::export]
impl Core {
    /// What `library`'s grid opens with when opened anew: `home_query` as it is when a Home row's header opened it, posters with
    /// titles, nothing remembered; else the library's first query with its remembered sort, filters (only if the Filter menu still
    /// offers every one) and view, each while Settings remembers it.
    pub fn library_opening(&self, library: Library, home_query: Option<ItemQuery>) -> GridOpening {
        self.app
            .library_opening(&(&library).into(), home_query.as_ref().map(Into::into))
            .into()
    }

    /// What `view` narrowed to `kind` of `server` opens with when opened anew: a kind tab's page ([`view_remembers`]) opened from its
    /// tab with the sort, filters (only if the Filter menu still offers every one) and view remembered there, each while Settings
    /// remembers it; opened from a Favourites row's header (`favourite`), or any other view, [`view_opening`].
    pub fn view_opening(
        &self,
        server: ServerRef,
        view: BrowseView,
        kind: Option<String>,
        favourite: bool,
    ) -> GridOpening {
        self.app
            .view_opening(
                &(&server).into(),
                view.into(),
                kind_of(kind).as_ref(),
                favourite,
            )
            .into()
    }

    /// What `view` narrowed to `kind` of `server` remembers, as [`Core::library_choices`]: nothing for a view that does not remember
    /// ([`view_remembers`]).
    pub fn view_choices(
        &self,
        server: ServerRef,
        view: BrowseView,
        kind: Option<String>,
    ) -> super::LibraryChoices {
        self.app
            .view_choices(&(&server).into(), view.into(), kind_of(kind).as_ref())
            .into()
    }

    /// Remembers the user's choices in `view` narrowed to `kind` of `server`, as [`Core::remember_library_choices`]; nothing is
    /// written for a view that does not remember ([`view_remembers`]). Not cancellable.
    ///
    /// # Errors
    /// As [`Core::set_remember_sort`].
    pub async fn remember_view_choices(
        &self,
        server: ServerRef,
        view: BrowseView,
        kind: Option<String>,
        choices: super::LibraryChoices,
    ) -> Result<(), CoreError> {
        let server: model::ServerRef = (&server).into();
        let view: current_app::BrowseView = view.into();
        let kind = kind_of(kind);
        self.save_settings(move |app| {
            app.remember_view_choices(&server, view, kind.as_ref(), choices.into())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_and_rows_cross_the_bridge() {
        let query = view_query(BrowseView::Everything, Some("movie".into()), true);
        assert_eq!(query.kinds, ["movie"]);
        assert!(query.filters.contains(&"favourite".to_owned()));
        assert_eq!(view_layout(BrowseView::Favourites, None), ViewLayout::Rows);
        assert_eq!(
            grid_filter_menu(Some(BrowseView::RecentlyAdded), None).kinds,
            [None, Some("movie".to_owned()), Some("series".to_owned())]
        );
        assert_eq!(
            view_title(BrowseView::Everything, Some("series".into())),
            "Series"
        );
        let display = search_group_display("episode".into(), Some(BrowseView::Everything));
        assert_eq!(display.shape, TileShape::Wide);
        assert_eq!(
            display.link.map(|link| link.target),
            Some(RowTarget::OpenView {
                view: BrowseView::Everything,
                kind: Some("episode".into()),
                favourite: true
            })
        );
        let row = HomeRow {
            key: "seen".into(),
            kind: HomeRowKind::SeenRecently,
            title: String::new(),
            library: None,
        };
        assert_eq!(home_row_display(row).title, "Seen recently");
        assert_eq!(time_left_short(20_000), "1m");
        assert_eq!(runtime_label(90 * 60_000 + 30_000), "1 h 30 min");
        assert_eq!(
            favourites_status(0, false, false).as_deref(),
            Some("No favourites yet")
        );
        assert_eq!(library_filter_name(None), "All");
        assert_eq!(kind_group_name("box_set".into()), "Collections");
    }
}
