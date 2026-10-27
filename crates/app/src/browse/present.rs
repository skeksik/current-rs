//! What the browse screens show, decided once for every platform (CLAUDE.md: all logic lives in the core): which picture a tile
//! takes, a series' run, the sort and filter choices of a library grid and what its search turns off, and when a list shown again
//! refreshes. The UIs only name and
//! format what these return.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use current_model::{
    Artwork, ImageType, ItemDetails, ItemFilter, ItemQuery, ItemSummary, MediaKind, SortKey,
    SortOrder,
};

use super::lock;

/// The shape of a tile's picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileShape {
    /// 2:3, for films and series.
    Poster,
    /// 16:9, for episodes and videos.
    Wide,
}

/// The artwork a tile of `shape` shows for `item`: its primary image for a poster; for a wide tile its thumb, else a primary image
/// that is wide (an episode's still), else its backdrop, else its primary image. `None` if it has none of them.
pub fn tile_artwork(item: &ItemSummary, shape: TileShape) -> Option<&Artwork> {
    let of = |kind: &ImageType| item.artwork.iter().find(|art| &art.kind == kind);
    match shape {
        TileShape::Poster => of(&ImageType::PRIMARY),
        TileShape::Wide => of(&ImageType::THUMB)
            .or_else(|| {
                item.artwork.iter().find(|art| {
                    art.kind == ImageType::PRIMARY
                        && art
                            .aspect_ratio
                            .is_some_and(|ratio| ratio.width() > ratio.height())
                })
            })
            .or_else(|| of(&ImageType::BACKDROP))
            .or_else(|| of(&ImageType::PRIMARY)),
    }
}

/// A series' run, for its detail page: the year it started, the year it ended if it did, and whether it is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeriesRun {
    pub start_year: i32,
    /// Only when it ended in another year than it started.
    pub end_year: Option<i32>,
    /// The server says it continues ("Continuing").
    pub continuing: bool,
}

/// `details`' run, for a series with a premiere date; `None` for anything else.
pub fn series_run(details: &ItemDetails) -> Option<SeriesRun> {
    if details.summary.kind != MediaKind::SERIES {
        return None;
    }
    let start_year = details.premiere?.year;
    let end_year = details
        .end
        .map(|end| end.year)
        .filter(|year| *year != start_year);
    Some(SeriesRun {
        start_year,
        end_year,
        // Jellyfin writes the series status as "Continuing" or "Ended" (docs/notes/jellyfin-browse.md); anything else is not running.
        continuing: end_year.is_none() && details.status.as_deref() == Some("Continuing"),
    })
}

/// One way to sort a library grid: its key and the order it goes in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortChoice {
    pub key: SortKey,
    pub order: SortOrder,
}

/// The sorts a library grid offers, in the order the UI lists them: names A to Z (the grid's first sort, [`super::library_query`]),
/// then the newest added, the newest released and the best rated first.
pub fn library_sorts() -> Vec<SortChoice> {
    [
        (SortKey::SORT_NAME, SortOrder::Ascending),
        (SortKey::DATE_CREATED, SortOrder::Descending),
        (SortKey::PREMIERE_DATE, SortOrder::Descending),
        (SortKey::COMMUNITY_RATING, SortOrder::Descending),
    ]
    .into_iter()
    .map(|(key, order)| SortChoice { key, order })
    .collect()
}

/// The filters a library grid offers, in the order the UI lists them: everything first (`None`), then unplayed and played.
/// Favourites have their own tab across every library, so a grid offers no favourites filter (owner, 2026-10-26).
pub fn library_filters() -> Vec<Option<ItemFilter>> {
    vec![None, Some(ItemFilter::UNPLAYED), Some(ItemFilter::PLAYED)]
}

/// How many matches a library grid's search can show: Jellyfin's search answers at most three times a request's limit and counts only
/// those in its total (`docs/notes/jellyfin-browse.md`, "Search"), and a grid asks for [`super::PAGE_SIZE`] items at a time.
pub const LIBRARY_SEARCH_WINDOW: usize = 3 * super::PAGE_SIZE;

/// The tooltip of a library grid's Sort button while a search makes it unavailable (owner, 2026-10-27).
pub const SORT_OFF_WHILE_SEARCHING: &str = "Sorted by relevance while searching";
/// The tooltip of a library grid's Filter button while a search makes it unavailable.
pub const FILTERS_OFF_WHILE_SEARCHING: &str = "Filters are off while searching";

/// What a library grid's Sort and Filter buttons offer for the query it shows (owner, 2026-10-27): neither while it searches, as its
/// results come in the server's relevance order, unfiltered ([`super::library_search`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridTools {
    pub sort_available: bool,
    /// Why Sort is unavailable, for its tooltip; `None` while it is available.
    pub sort_tooltip: Option<String>,
    pub filter_available: bool,
    /// Why Filter is unavailable, for its tooltip; `None` while it is available.
    pub filter_tooltip: Option<String>,
}

/// The Sort and Filter buttons of a grid showing `query`.
pub fn grid_tools(query: &ItemQuery) -> GridTools {
    let searching = query.search_term.is_some();
    let off = |text: &str| searching.then(|| text.to_owned());
    GridTools {
        sort_available: !searching,
        sort_tooltip: off(SORT_OFF_WHILE_SEARCHING),
        filter_available: !searching,
        filter_tooltip: off(FILTERS_OFF_WHILE_SEARCHING),
    }
}

/// The line a grid showing `query` shows above its items once it holds `total` (owner, 2026-10-27): while it searches and its total
/// reaches [`LIBRARY_SEARCH_WINDOW`], the server left out whatever matched less well, so the user is told to narrow the search.
/// `None` otherwise.
pub fn library_search_note(query: &ItemQuery, total: u32) -> Option<String> {
    let full = usize::try_from(total).is_ok_and(|total| total >= LIBRARY_SEARCH_WINDOW);
    (query.search_term.is_some() && full).then(|| {
        format!("Showing the best {LIBRARY_SEARCH_WINDOW} matches. Type more to narrow it.")
    })
}

/// "The page is shown again after five minutes" refreshes a list (ADR 0018, section 3).
pub const REFRESH_WHEN_SHOWN_AFTER: Duration = Duration::from_secs(5 * 60);

/// When the page showing a list was left, so that showing it again can tell whether it is stale.
#[derive(Debug, Default)]
pub(crate) struct ShownAgain {
    hidden_at: Mutex<Option<Instant>>,
}

impl ShownAgain {
    /// The page showing the list was left now.
    pub(crate) fn hidden(&self) {
        self.hidden_at(Instant::now());
    }

    fn hidden_at(&self, now: Instant) {
        *lock(&self.hidden_at) = Some(now);
    }

    /// The page is shown now: whether it was left [`REFRESH_WHEN_SHOWN_AFTER`] or longer ago, so the list refreshes. A list shown
    /// for the first time, or again without having been left, is not stale.
    pub(crate) fn shown(&self) -> bool {
        self.shown_at(Instant::now())
    }

    fn shown_at(&self, now: Instant) -> bool {
        lock(&self.hidden_at)
            .take()
            .is_some_and(|hidden| now.saturating_duration_since(hidden) >= REFRESH_WHEN_SHOWN_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use current_model::{
        AspectRatio, CalendarDate, ImageRef, ItemId, ItemRef, ServerKey, ServerRef, ServiceType,
        UserState,
    };

    use super::*;

    fn art(kind: ImageType, ratio: Option<(u32, u32)>) -> Artwork {
        let item = ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new("57a2d1e0000000000000000000001101"),
        );
        Artwork {
            kind: kind.clone(),
            image: ImageRef {
                item,
                kind,
                index: 0,
                tag: "57a2d1e0a0000001".into(),
            },
            aspect_ratio: ratio.and_then(|(width, height)| AspectRatio::new(width, height)),
            blurhash: None,
        }
    }

    fn summary(kind: MediaKind, artwork: Vec<Artwork>) -> ItemSummary {
        ItemSummary {
            item: artwork
                .first()
                .map(|art| art.image.item.clone())
                .unwrap_or_else(|| {
                    ItemRef::new(
                        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                        ItemId::new("57a2d1e0000000000000000000001101"),
                    )
                }),
            kind,
            name: "Example Item".into(),
            year: Some(2010),
            runtime: None,
            artwork,
            user: UserState::default(),
            episode: None,
            child_count: None,
        }
    }

    #[test]
    fn a_tile_takes_the_artwork_that_fits_its_shape() {
        let poster = art(ImageType::PRIMARY, Some((2, 3)));
        let still = art(ImageType::PRIMARY, Some((16, 9)));
        let thumb = art(ImageType::THUMB, Some((16, 9)));
        let backdrop = art(ImageType::BACKDROP, Some((16, 9)));

        let film = summary(
            MediaKind::MOVIE,
            vec![poster.clone(), backdrop.clone(), thumb.clone()],
        );
        assert_eq!(tile_artwork(&film, TileShape::Poster), Some(&poster));
        assert_eq!(tile_artwork(&film, TileShape::Wide), Some(&thumb));

        // An episode with its own still and no thumb: the still, not the series' poster.
        let episode = summary(MediaKind::EPISODE, vec![still.clone(), backdrop.clone()]);
        assert_eq!(tile_artwork(&episode, TileShape::Wide), Some(&still));
        // Only a tall primary and a backdrop: the backdrop.
        let tall = summary(MediaKind::MOVIE, vec![poster.clone(), backdrop.clone()]);
        assert_eq!(tile_artwork(&tall, TileShape::Wide), Some(&backdrop));
        // Only a tall primary: that, cropped by the UI.
        let only = summary(MediaKind::MOVIE, vec![poster.clone()]);
        assert_eq!(tile_artwork(&only, TileShape::Wide), Some(&poster));
        assert_eq!(
            tile_artwork(&summary(MediaKind::MOVIE, Vec::new()), TileShape::Poster),
            None
        );
    }

    fn details(
        kind: MediaKind,
        premiere: Option<i32>,
        end: Option<i32>,
        status: Option<&str>,
    ) -> ItemDetails {
        ItemDetails {
            summary: summary(kind, Vec::new()),
            original_title: None,
            overview: None,
            taglines: Vec::new(),
            genres: Vec::new(),
            studios: Vec::new(),
            people: Vec::new(),
            official_rating: None,
            community_rating: None,
            critic_rating: None,
            premiere: premiere.and_then(|year| CalendarDate::new(year, 1, 1)),
            end: end.and_then(|year| CalendarDate::new(year, 12, 31)),
            status: status.map(Into::into),
            backdrops: Vec::new(),
            logo: None,
            chapters: Vec::new(),
            media: None,
        }
    }

    #[test]
    fn a_series_run_comes_from_its_dates_and_status() {
        let run =
            |premiere, end, status| series_run(&details(MediaKind::SERIES, premiere, end, status));
        assert_eq!(
            run(Some(2008), Some(2013), Some("Ended")),
            Some(SeriesRun {
                start_year: 2008,
                end_year: Some(2013),
                continuing: false
            })
        );
        assert_eq!(
            run(Some(2019), None, Some("Continuing")),
            Some(SeriesRun {
                start_year: 2019,
                end_year: None,
                continuing: true
            })
        );
        // Ended within its first year: one year, not "2020-2020".
        assert_eq!(
            run(Some(2020), Some(2020), Some("Ended")),
            Some(SeriesRun {
                start_year: 2020,
                end_year: None,
                continuing: false
            })
        );
        // A status Current does not know is not running.
        assert_eq!(
            run(Some(2020), None, Some("Paused")).map(|run| run.continuing),
            Some(false)
        );
        assert_eq!(run(None, None, Some("Continuing")), None);
        assert_eq!(
            series_run(&details(MediaKind::MOVIE, Some(2010), None, None)),
            None
        );
    }

    #[test]
    fn the_library_choices_start_with_the_first_query() {
        let sorts = library_sorts();
        assert_eq!(
            sorts[0],
            SortChoice {
                key: SortKey::SORT_NAME,
                order: SortOrder::Ascending
            }
        );
        assert!(
            sorts[1..]
                .iter()
                .all(|sort| sort.order == SortOrder::Descending)
        );
        assert_eq!(library_filters()[0], None);
        assert_eq!(library_filters().len(), 3);
        assert!(!library_filters().contains(&Some(ItemFilter::FAVOURITE)));
    }

    #[test]
    fn a_searching_grid_offers_no_sort_or_filter_and_says_why() {
        let browsing = ItemQuery {
            sort: vec![SortKey::SORT_NAME],
            ..ItemQuery::default()
        };
        assert_eq!(
            grid_tools(&browsing),
            GridTools {
                sort_available: true,
                sort_tooltip: None,
                filter_available: true,
                filter_tooltip: None,
            }
        );
        let searching = ItemQuery {
            search_term: Some("Example".into()),
            ..ItemQuery::default()
        };
        let tools = grid_tools(&searching);
        assert!(!tools.sort_available && !tools.filter_available);
        assert_eq!(
            tools.sort_tooltip.as_deref(),
            Some("Sorted by relevance while searching")
        );
        assert_eq!(
            tools.filter_tooltip.as_deref(),
            Some("Filters are off while searching")
        );
    }

    #[test]
    fn a_search_that_fills_the_server_s_window_asks_for_more_text() {
        // Three pages of a grid: what one Jellyfin search can answer.
        assert_eq!(LIBRARY_SEARCH_WINDOW, 300);
        let window = u32::try_from(LIBRARY_SEARCH_WINDOW).unwrap();
        let searching = ItemQuery {
            search_term: Some("Example".into()),
            ..ItemQuery::default()
        };
        assert_eq!(library_search_note(&searching, window - 1), None);
        assert_eq!(
            library_search_note(&searching, window).as_deref(),
            Some("Showing the best 300 matches. Type more to narrow it.")
        );
        assert!(library_search_note(&searching, u32::MAX).is_some());
        // A grid that does not search shows every item: no note, however many.
        assert_eq!(library_search_note(&ItemQuery::default(), window), None);
        assert_eq!(library_search_note(&ItemQuery::default(), 0), None);
    }

    #[test]
    fn a_list_shown_again_after_five_minutes_is_stale() {
        let start = Instant::now();
        let shown = ShownAgain::default();
        // Never left: not stale.
        assert!(!shown.shown_at(start + Duration::from_secs(3600)));
        shown.hidden_at(start);
        assert!(!shown.shown_at(start + REFRESH_WHEN_SHOWN_AFTER - Duration::from_secs(1)));
        // Showing again forgot the earlier leave.
        assert!(!shown.shown_at(start + REFRESH_WHEN_SHOWN_AFTER + Duration::from_secs(1)));
        shown.hidden_at(start);
        assert!(shown.shown_at(start + REFRESH_WHEN_SHOWN_AFTER));
    }
}
