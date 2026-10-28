//! What the browse screens call things, worded once for every platform (CLAUDE.md: all logic lives in the core): media kinds as
//! search groups and Filter menu entries, a library grid's sorts and filters, runtimes and the time left on a tile, and the Favourites
//! tab's empty and failed lines. The UIs show these as they are, with their own icons.

use std::time::Duration;

use current_model::{ItemFilter, MediaKind, SortKey};

/// A media kind's group name, as search groups, the Favourites tab's rows and a kind's page name them ("Movies", "Series"). Open: a
/// kind Current has no name for shows as the server named it, each word capitalised ("live_tv" reads "Live Tv").
pub fn kind_group(kind: &MediaKind) -> String {
    let name = match kind.as_str() {
        "movie" => "Movies",
        "series" => "Series",
        "season" => "Seasons",
        "episode" => "Episodes",
        "person" => "People",
        "collection" | "box_set" => "Collections",
        "music_album" => "Albums",
        "music_artist" => "Artists",
        "audio" => "Songs",
        "music_video" => "Music videos",
        "video" => "Videos",
        "folder" => "Folders",
        other => return title_case(&other.replace('_', " ")),
    };
    name.to_owned()
}

/// Each word with its first letter capitalised and the rest in lower case, a word in capitals kept (an acronym), as the Windows app
/// did with .NET's `TextInfo.ToTitleCase`, which "converts the first character of each word to uppercase and the rest of the
/// characters to lowercase", words "entirely in uppercase" kept as acronyms (learn.microsoft.com, read 2026-10-27). Words are split at
/// spaces; other separators and the culture's own case rules are not verified against .NET.
fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.chars().any(char::is_alphabetic) && !word.chars().any(char::is_lowercase) {
                return word.to_owned();
            }
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A media type in the Filter menu of a view across every library ([`super::filter_menu`]): "All types" for every type (`None`),
/// else the type's name; a kind Current has no name for shows as the core gave it.
pub fn kind_choice_name(kind: Option<&MediaKind>) -> String {
    match kind.map(MediaKind::as_str) {
        None => "All types".to_owned(),
        Some("movie") => "Movies".to_owned(),
        Some("series") => "Series".to_owned(),
        Some(other) => other.to_owned(),
    }
}

/// A sort's name in a library grid's Sort menu ([`super::library_sorts`]); a key Current has no name for shows as it is.
pub fn library_sort_name(key: &SortKey) -> String {
    match key.as_str() {
        "sort_name" => "Name",
        "date_created" => "Date added",
        "premiere_date" => "Release date",
        "community_rating" => "Rating",
        other => other,
    }
    .to_owned()
}

/// A filter's name in a library grid's Filter menu ([`super::library_filters`], [`super::library_toggle_filters`]): "All" for no
/// filter (`None`); a filter Current has no name for shows as it is.
pub fn library_filter_name(filter: Option<&ItemFilter>) -> String {
    match filter.map(ItemFilter::as_str) {
        None => "All",
        Some("unplayed") => "Unplayed",
        Some("played") => "Played",
        Some("favourite") => "Favourites",
        Some(other) => other,
    }
    .to_owned()
}

/// Whole minutes in `time`, a half rounded to the even one, as the Windows app's `Math.Round(Double)`, which "rounds midpoint values
/// to the nearest even number" (learn.microsoft.com, "Math.Round Method", ms.date 2025-07-01, read 2026-10-27).
fn minutes(time: Duration) -> u64 {
    // Milliseconds, as the bridge carries times; a runtime's are far below 2^53, so the division is the Windows app's own.
    #[expect(
        clippy::cast_precision_loss,
        reason = "milliseconds of a runtime, far below 2^53"
    )]
    let total = time.as_millis() as f64 / 60_000.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a duration's minutes: never negative, far below u64::MAX"
    )]
    let rounded = total.round_ties_even() as u64;
    rounded
}

/// An item's runtime: "2 h 12 min", "45 min".
pub fn runtime_label(runtime: Duration) -> String {
    let minutes = minutes(runtime);
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// The time left on a tile, short: "1h 12m", "45m"; under a minute counts as one.
pub fn time_left_short(left: Duration) -> String {
    let minutes = minutes(left).max(1);
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// The time left in words, as a screen reader reads a tile: "1 h 12 min left", "45 min left"; under a minute counts as one.
pub fn time_left_label(left: Duration) -> String {
    format!("{} left", runtime_label(left.max(Duration::from_secs(60))))
}

/// The Favourites tab's line when it shows no row and is not loading: no favourites.
pub const FAVOURITES_EMPTY: &str = "No favourites yet";
/// The Favourites tab's line when its rows failed to load, before the UI's words for why.
pub const FAVOURITES_FAILED: &str = "Couldn't load favourites.";

/// The Favourites tab's line above its rows: none while it shows `rows` rows or is `loading`; else [`FAVOURITES_FAILED`] when it
/// `failed` (the UI adds why), else [`FAVOURITES_EMPTY`].
pub fn favourites_status(rows: usize, loading: bool, failed: bool) -> Option<&'static str> {
    if rows > 0 || loading {
        None
    } else if failed {
        Some(FAVOURITES_FAILED)
    } else {
        Some(FAVOURITES_EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_named_and_an_unknown_kind_is_title_cased() {
        let cases = [
            ("movie", "Movies"),
            ("series", "Series"),
            ("season", "Seasons"),
            ("episode", "Episodes"),
            ("person", "People"),
            ("collection", "Collections"),
            ("box_set", "Collections"),
            ("music_album", "Albums"),
            ("music_artist", "Artists"),
            ("audio", "Songs"),
            ("music_video", "Music videos"),
            ("video", "Videos"),
            ("folder", "Folders"),
            ("scene", "Scene"),
            ("live_tv_channel", "Live Tv Channel"),
            ("trailer_HD", "Trailer HD"),
            ("mIxEd", "Mixed"),
        ];
        for (kind, name) in cases {
            assert_eq!(kind_group(&MediaKind::new(kind)), name, "{kind}");
        }
    }

    #[test]
    fn menu_entries_are_named() {
        assert_eq!(kind_choice_name(None), "All types");
        assert_eq!(kind_choice_name(Some(&MediaKind::MOVIE)), "Movies");
        assert_eq!(kind_choice_name(Some(&MediaKind::SERIES)), "Series");
        assert_eq!(kind_choice_name(Some(&MediaKind::VIDEO)), "video");
        let sorts: Vec<String> = super::super::library_sorts()
            .iter()
            .map(|sort| library_sort_name(&sort.key))
            .collect();
        assert_eq!(sorts, ["Name", "Date added", "Release date", "Rating"]);
        assert_eq!(library_sort_name(&SortKey::RANDOM), "random");
        let filters: Vec<String> = super::super::library_filters()
            .iter()
            .map(|filter| library_filter_name(filter.as_ref()))
            .collect();
        assert_eq!(filters, ["All", "Unplayed", "Played"]);
        assert_eq!(
            library_filter_name(Some(&ItemFilter::FAVOURITE)),
            "Favourites"
        );
        assert_eq!(
            library_filter_name(Some(&ItemFilter::RESUMABLE)),
            "resumable"
        );
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn runtimes_round_to_whole_minutes() {
        assert_eq!(runtime_label(ms(132 * 60_000)), "2 h 12 min");
        assert_eq!(runtime_label(ms(45 * 60_000)), "45 min");
        assert_eq!(runtime_label(ms(60 * 60_000)), "1 h 0 min");
        assert_eq!(runtime_label(ms(44 * 60_000 + 29_999)), "44 min");
        assert_eq!(runtime_label(ms(44 * 60_000 + 30_001)), "45 min");
        // A half rounds to the even minute, as Math.Round did: 90.5 stays 90, 89.5 goes to 90.
        assert_eq!(runtime_label(ms(90 * 60_000 + 30_000)), "1 h 30 min");
        assert_eq!(runtime_label(ms(89 * 60_000 + 30_000)), "1 h 30 min");
        assert_eq!(runtime_label(ms(0)), "0 min");
    }

    #[test]
    fn long_runtimes_count_whole_hours_without_wrapping() {
        assert_eq!(runtime_label(ms(24 * 3_600_000)), "24 h 0 min");
        assert_eq!(
            runtime_label(ms(25 * 3_600_000 + 12 * 60_000)),
            "25 h 12 min"
        );
        assert_eq!(time_left_short(ms(25 * 3_600_000 + 12 * 60_000)), "25h 12m");
        assert_eq!(time_left_label(ms(100 * 3_600_000)), "100 h 0 min left");
    }

    #[test]
    fn time_left_counts_under_a_minute_as_one() {
        assert_eq!(time_left_short(ms(72 * 60_000)), "1h 12m");
        assert_eq!(time_left_short(ms(45 * 60_000)), "45m");
        assert_eq!(time_left_short(ms(20_000)), "1m");
        assert_eq!(time_left_short(ms(0)), "1m");
        assert_eq!(time_left_short(ms(90 * 60_000 + 30_000)), "1h 30m");
        assert_eq!(time_left_short(ms(150_000)), "2m");
        assert_eq!(time_left_label(ms(72 * 60_000)), "1 h 12 min left");
        assert_eq!(time_left_label(ms(45 * 60_000)), "45 min left");
        assert_eq!(time_left_label(ms(20_000)), "1 min left");
        assert_eq!(time_left_label(ms(90_000)), "2 min left");
    }

    #[test]
    fn the_favourites_line_shows_only_without_rows() {
        assert_eq!(favourites_status(2, false, false), None);
        assert_eq!(favourites_status(0, true, true), None);
        assert_eq!(favourites_status(1, false, true), None);
        assert_eq!(
            favourites_status(0, false, false),
            Some("No favourites yet")
        );
        assert_eq!(
            favourites_status(0, false, true),
            Some("Couldn't load favourites.")
        );
    }
}
