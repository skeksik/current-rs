//! What the browse screens say about items, worded once for every platform (CLAUDE.md: all logic lives in the core): a tile's lines
//! and the name a screen reader reads for it, a person's part, the notices for a failed mark and a library that went away, a size on
//! disk, a grid's item count, a detail page's facts line and the search page's status line. Never an address or a server's message:
//! the records hold none. Item names appear only on screen and in screen readers' names, never in a log.

use std::time::Duration;

use current_model::{ItemDetails, ItemSummary, MediaKind, PersonKind};

use super::labels::{runtime_label, time_left_label};
use super::present::{SeriesRun, series_run};
use super::states::ItemAction;
use crate::numbers::NumberFormat;
use crate::problems::{BrowseProblem, Platform, browse_problem_text};

/// The first line under a tile: an episode's series (with its own name on the second line), else the item's name. On its series'
/// own page (`with_series` false) an episode shows its own name.
pub fn tile_title(item: &ItemSummary, with_series: bool) -> String {
    match series_name(item) {
        Some(series) if with_series && item.kind == MediaKind::EPISODE => series.to_owned(),
        _ => item.name.clone(),
    }
}

/// The second line under a tile: an episode's number and name ("S1:E2 · Name"), a series' new episodes ("3 new episodes"), a
/// year, or nothing.
pub fn tile_subtitle(item: &ItemSummary, with_series: bool) -> String {
    if item.kind == MediaKind::EPISODE {
        let number = episode_number(item);
        let shows_series = with_series && series_name(item).is_some();
        return match (shows_series, number.is_empty()) {
            (true, false) => format!("{number} · {}", item.name),
            (true, true) => item.name.clone(),
            (false, _) => number,
        };
    }
    match item.child_count {
        Some(count) if count > 1 && item.kind == MediaKind::SERIES => {
            format!("{count} new episodes")
        }
        _ => item.year.map(|year| year.to_string()).unwrap_or_default(),
    }
}

fn series_name(item: &ItemSummary) -> Option<&str> {
    item.episode
        .as_ref()
        .and_then(|links| links.series_name.as_deref())
        .filter(|name| !name.is_empty())
}

fn episode_number(item: &ItemSummary) -> String {
    item.episode
        .as_ref()
        .map(current_model::EpisodeLinks::number_label)
        .unwrap_or_default()
}

/// What a screen reader reads for a tile: its lines, then its marks ("played", "in progress, 12 min left", "in progress",
/// "3 unplayed", "favourite"), so a list of tiles is told apart and its state heard without opening an item.
pub fn tile_name(item: &ItemSummary, with_series: bool) -> String {
    let mut parts = vec![tile_title(item, with_series)];
    let subtitle = tile_subtitle(item, with_series);
    if !subtitle.is_empty() {
        parts.push(subtitle);
    }
    if item.user.played {
        parts.push("played".to_owned());
    } else if let Some(left) = item.time_left() {
        parts.push(format!("in progress, {}", time_left_label(left)));
    } else if item.user.progress.is_some_and(|progress| progress > 0.0) {
        parts.push("in progress".to_owned());
    }
    if let Some(unplayed) = item.user.unplayed_count.filter(|&count| count > 0)
        && !item.user.played
    {
        parts.push(format!("{unplayed} unplayed"));
    }
    if item.user.favourite {
        parts.push("favourite".to_owned());
    }
    parts.join(", ")
}

/// The name of a tile whose item is still loading.
pub const LOADING_TILE_NAME: &str = "Loading";

/// The name of a tile whose page failed to load.
pub const FAILED_TILE_NAME: &str = "Couldn't load. Use Retry above the grid.";

/// A person's part when the server gives no role: what they did, or nothing for an actor or a part Current has no name for.
pub fn person_kind_name(kind: &PersonKind) -> &'static str {
    match kind.as_str() {
        "director" => "Director",
        "writer" => "Writer",
        "producer" => "Producer",
        "composer" => "Composer",
        "guest_star" => "Guest star",
        _ => "",
    }
}

/// The title of the notice for a played or favourite mark that failed: "Couldn't mark Name as played.". "the item" when the name
/// is not known.
pub fn action_failed_title(action: ItemAction, item_name: Option<&str>) -> String {
    let name = item_name
        .filter(|name| !name.is_empty())
        .unwrap_or("the item");
    match action {
        ItemAction::SetPlayed(true) => format!("Couldn't mark {name} as played."),
        ItemAction::SetPlayed(false) => format!("Couldn't mark {name} as unplayed."),
        ItemAction::SetFavourite(true) => format!("Couldn't add {name} to favourites."),
        ItemAction::SetFavourite(false) => format!("Couldn't remove {name} from favourites."),
    }
}

/// The title of the notice shown when the library on screen left its server's list.
pub const LIBRARY_GONE_TITLE: &str = "This library is no longer available";

/// The message of the notice shown when the library on screen left its server's list.
pub const LIBRARY_GONE_MESSAGE: &str =
    "It was removed from the server, or you no longer have access to it.";

/// A size on disk in binary units with at most one decimal below 10 ("512 bytes", "1.5 KB", "12 MB"), as the Windows app wrote
/// sizes: .NET's custom formats "0.#" and "0" on a double (not verified against File Explorer).
pub fn size_on_disk(bytes: u64, format: &NumberFormat) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    #[expect(
        clippy::cast_precision_loss,
        reason = "as the Windows app did: a double from the byte count, then divided"
    )]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        let decimals = usize::from(value < 10.0);
        format!("{} {}", format.custom(value, 15, 0, decimals), UNITS[unit])
    }
}

/// A library grid's count: "1 item", "12,345 items" (digits grouped as the UI's region groups them, as .NET's "N0" does), or
/// nothing for none.
pub fn item_count_label(total: u64, format: &NumberFormat) -> String {
    match total {
        0 => String::new(),
        1 => "1 item".to_owned(),
        _ => format!("{} items", format.grouped(total)),
    }
}

/// A detail page's facts line: the year, or a series' run ("2019–2023", "2019–" while it continues); the runtime; the age rating;
/// and the community rating ("7.3/10", one decimal as .NET's "0.0" writes a float), joined by " · ".
pub fn details_facts(details: &ItemDetails, format: &NumberFormat) -> String {
    facts_line(
        &DetailsFacts {
            year: details.summary.year,
            run: series_run(details),
            runtime: details.summary.runtime,
            official_rating: details.official_rating.clone(),
            community_rating: details.community_rating,
        },
        format,
    )
}

/// What a detail page's facts line says, as the bridge's detail records hold it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailsFacts {
    pub year: Option<i32>,
    /// A series' run ([`series_run`]); shown instead of the year.
    pub run: Option<SeriesRun>,
    pub runtime: Option<Duration>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
}

/// The facts line of [`details_facts`].
pub fn facts_line(facts: &DetailsFacts, format: &NumberFormat) -> String {
    let mut line = Vec::new();
    if let Some(run) = &facts.run {
        line.push(match (run.end_year, run.continuing) {
            (Some(end), _) => format!("{}\u{2013}{end}", run.start_year),
            (None, true) => format!("{}\u{2013}", run.start_year),
            (None, false) => run.start_year.to_string(),
        });
    } else if let Some(year) = facts.year {
        line.push(year.to_string());
    }
    if let Some(runtime) = facts.runtime.filter(|runtime| !runtime.is_zero()) {
        line.push(runtime_label(runtime));
    }
    if let Some(rating) = facts
        .official_rating
        .as_deref()
        .filter(|rating| !rating.is_empty())
    {
        line.push(rating.to_owned());
    }
    if let Some(community) = facts.community_rating {
        // A float: .NET takes its first 7 significant digits before rounding.
        line.push(format!(
            "{}/10",
            format.custom(f64::from(community), 7, 1, 1)
        ));
    }
    line.join(" · ")
}

/// The search page's status line: "Searching…" until the first results, nothing once there are results, "No results", or
/// "Couldn't search. " and why. On a view in rows (the Favourites tab, `favourites`), the Favourites tab's lines instead
/// ([`super::favourites_status`]), with why it failed.
pub fn search_status(
    found: usize,
    loading: bool,
    failed: Option<&BrowseProblem>,
    favourites: bool,
    platform: Platform,
) -> String {
    let why = |problem: &BrowseProblem| browse_problem_text(problem, platform);
    if favourites {
        return match (
            super::favourites_status(found, loading, failed.is_some()),
            failed,
        ) {
            (None, _) => String::new(),
            (Some(status), Some(problem)) => format!("{status} {}", why(problem)),
            (Some(status), None) => status.to_owned(),
        };
    }
    if found > 0 {
        String::new()
    } else if loading {
        "Searching\u{2026}".to_owned()
    } else if let Some(problem) = failed {
        format!("Couldn't search. {}", why(problem))
    } else {
        "No results".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use current_model::{
        EpisodeLinks, ItemDetails, ItemId, ItemRef, ItemSummary, MediaKind, ServerKey, ServerRef,
        ServiceType, UserState,
    };

    use super::*;
    use crate::problems::NetProblem;

    fn item(kind: MediaKind, name: &str) -> ItemSummary {
        ItemSummary {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new("57a2d1e0000000000000000000001101"),
            ),
            kind,
            name: name.to_owned(),
            year: None,
            runtime: None,
            artwork: Vec::new(),
            user: UserState::default(),
            episode: None,
            child_count: None,
        }
    }

    fn details_of(summary: ItemSummary) -> ItemDetails {
        ItemDetails {
            summary,
            original_title: None,
            overview: None,
            taglines: Vec::new(),
            genres: Vec::new(),
            studios: Vec::new(),
            people: Vec::new(),
            official_rating: None,
            community_rating: None,
            critic_rating: None,
            premiere: None,
            end: None,
            status: None,
            backdrops: Vec::new(),
            logo: None,
            chapters: Vec::new(),
            media: None,
        }
    }

    fn episode(series: Option<&str>) -> ItemSummary {
        ItemSummary {
            episode: Some(EpisodeLinks {
                series_name: series.map(str::to_owned),
                number: Some(2),
                season_number: Some(1),
                ..EpisodeLinks::default()
            }),
            ..item(MediaKind::EPISODE, "Pilot")
        }
    }

    #[test]
    fn an_episode_tile_shows_its_series_then_its_number_and_name() {
        let shown = episode(Some("A Series"));
        assert_eq!(tile_title(&shown, true), "A Series");
        assert_eq!(tile_subtitle(&shown, true), "S1:E2 · Pilot");
        // On its series' own page.
        assert_eq!(tile_title(&shown, false), "Pilot");
        assert_eq!(tile_subtitle(&shown, false), "S1:E2");
        // No series name: its own name, then its number.
        let alone = episode(None);
        assert_eq!(tile_title(&alone, true), "Pilot");
        assert_eq!(tile_subtitle(&alone, true), "S1:E2");
        let unnumbered = ItemSummary {
            episode: Some(EpisodeLinks {
                series_name: Some("A Series".into()),
                ..EpisodeLinks::default()
            }),
            ..item(MediaKind::EPISODE, "Pilot")
        };
        assert_eq!(tile_subtitle(&unnumbered, true), "Pilot");
    }

    #[test]
    fn other_tiles_show_new_episodes_or_a_year() {
        let series = ItemSummary {
            child_count: Some(3),
            year: Some(2019),
            ..item(MediaKind::SERIES, "A Series")
        };
        assert_eq!(tile_subtitle(&series, true), "3 new episodes");
        let one = ItemSummary {
            child_count: Some(1),
            ..series.clone()
        };
        assert_eq!(tile_subtitle(&one, true), "2019");
        let film = item(MediaKind::MOVIE, "A Film");
        assert_eq!(tile_subtitle(&film, true), "");
    }

    #[test]
    fn a_tiles_name_reads_its_lines_then_its_marks() {
        let mut shown = episode(Some("A Series"));
        shown.user.favourite = true;
        assert_eq!(
            tile_name(&shown, true),
            "A Series, S1:E2 · Pilot, favourite"
        );
        shown.runtime = Some(Duration::from_secs(45 * 60));
        shown.user.position = Some(Duration::from_secs(30 * 60));
        assert_eq!(
            tile_name(&shown, true),
            "A Series, S1:E2 · Pilot, in progress, 15 min left, favourite"
        );
        shown.user.played = true;
        shown.user.unplayed_count = Some(2);
        assert_eq!(
            tile_name(&shown, true),
            "A Series, S1:E2 · Pilot, played, favourite"
        );
        let mut series = item(MediaKind::SERIES, "A Series");
        series.user.progress = Some(0.5);
        series.user.unplayed_count = Some(3);
        assert_eq!(
            tile_name(&series, true),
            "A Series, in progress, 3 unplayed"
        );
    }

    #[test]
    fn parts_and_notices_are_worded() {
        assert_eq!(person_kind_name(&PersonKind::GUEST_STAR), "Guest star");
        assert_eq!(person_kind_name(&PersonKind::ACTOR), "");
        assert_eq!(person_kind_name(&PersonKind::new("lyricist")), "");
        assert_eq!(
            action_failed_title(ItemAction::SetPlayed(false), Some("A Film")),
            "Couldn't mark A Film as unplayed."
        );
        assert_eq!(
            action_failed_title(ItemAction::SetFavourite(true), None),
            "Couldn't add the item to favourites."
        );
        assert_eq!(
            action_failed_title(ItemAction::SetFavourite(false), Some("")),
            "Couldn't remove the item from favourites."
        );
    }

    #[test]
    fn sizes_use_binary_units_and_one_decimal_below_ten() {
        let format = NumberFormat::default();
        assert_eq!(size_on_disk(0, &format), "0 bytes");
        assert_eq!(size_on_disk(1023, &format), "1023 bytes");
        assert_eq!(size_on_disk(1024, &format), "1 KB");
        assert_eq!(size_on_disk(1536, &format), "1.5 KB");
        assert_eq!(size_on_disk(10 * 1024 * 1024 - 1, &format), "10 MB");
        assert_eq!(
            size_on_disk(15 * 1024 * 1024 + 512 * 1024, &format),
            "16 MB"
        );
        assert_eq!(size_on_disk(3 * 1024 * 1024 * 1024, &format), "3 GB");
        let comma = NumberFormat {
            decimal_separator: ",".into(),
            ..NumberFormat::default()
        };
        assert_eq!(size_on_disk(1536, &comma), "1,5 KB");
    }

    #[test]
    fn counts_group_their_digits() {
        let format = NumberFormat::default();
        assert_eq!(item_count_label(0, &format), "");
        assert_eq!(item_count_label(1, &format), "1 item");
        assert_eq!(item_count_label(12_345, &format), "12,345 items");
    }

    #[test]
    fn a_detail_pages_facts_join_with_dots() {
        let mut film = item(MediaKind::MOVIE, "A Film");
        film.year = Some(2019);
        film.runtime = Some(Duration::from_secs(90 * 60));
        let details = ItemDetails {
            official_rating: Some("PG-13".into()),
            community_rating: Some(7.25),
            ..details_of(film)
        };
        assert_eq!(
            details_facts(&details, &NumberFormat::default()),
            "2019 · 1 h 30 min · PG-13 · 7.3/10"
        );
        let bare = details_of(item(MediaKind::MOVIE, "A Film"));
        assert_eq!(details_facts(&bare, &NumberFormat::default()), "");
        let run = |end_year, continuing| DetailsFacts {
            year: Some(2019),
            run: Some(SeriesRun {
                start_year: 2019,
                end_year,
                continuing,
            }),
            runtime: None,
            official_rating: None,
            community_rating: Some(8.0),
        };
        let format = NumberFormat::default();
        assert_eq!(
            facts_line(&run(Some(2023), false), &format),
            "2019–2023 · 8.0/10"
        );
        assert_eq!(facts_line(&run(None, true), &format), "2019– · 8.0/10");
        assert_eq!(facts_line(&run(None, false), &format), "2019 · 8.0/10");
    }

    #[test]
    fn the_search_status_follows_the_results() {
        let failed = BrowseProblem::Network {
            problem: NetProblem::Unreachable,
        };
        let win = Platform::Windows;
        assert_eq!(search_status(0, true, None, false, win), "Searching…");
        assert_eq!(search_status(2, true, None, false, win), "");
        assert_eq!(search_status(0, false, None, false, win), "No results");
        assert_eq!(
            search_status(0, false, Some(&failed), false, win),
            "Couldn't search. Can't reach the server."
        );
        assert_eq!(search_status(0, true, None, true, win), "");
        assert_eq!(
            search_status(0, false, None, true, win),
            "No favourites yet"
        );
        assert_eq!(
            search_status(0, false, Some(&failed), true, win),
            "Couldn't load favourites. Can't reach the server."
        );
    }
}
