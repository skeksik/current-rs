//! What the browse screens say about items (`current-app`'s `browse::texts`): a tile's lines and screen reader name, a person's
//! part, the notices for a failed mark and a library that went away, a size on disk, a grid's item count, a detail page's facts line
//! and the search page's status line. The ones with numbers write them as the UI's region does ([`Core::set_decimal_separator`],
//! [`Core::set_digit_grouping`]).

use std::time::Duration;

use current_model as model;

use super::problems::BrowseProblem;
use super::records::{ItemAction, ItemSummary, SeriesRun};
use crate::Core;

/// The first line under a tile: an episode's series, else the item's name; on its series' own page (`with_series` false) an
/// episode's own name.
#[uniffi::export]
pub fn tile_title(item: ItemSummary, with_series: bool) -> String {
    current_app::tile_title(&(&item).into(), with_series)
}

/// The second line under a tile: an episode's number and name ("S1:E2 · Name"), a series' new episodes ("3 new episodes"), a
/// year, or empty.
#[uniffi::export]
pub fn tile_subtitle(item: ItemSummary, with_series: bool) -> String {
    current_app::tile_subtitle(&(&item).into(), with_series)
}

/// What a screen reader reads for a tile: its lines, then its marks ("played", "in progress, 12 min left", "3 unplayed",
/// "favourite").
#[uniffi::export]
pub fn tile_name(item: ItemSummary, with_series: bool) -> String {
    current_app::tile_name(&(&item).into(), with_series)
}

/// The name of a tile whose item is still loading: "Loading".
#[uniffi::export]
pub fn loading_tile_name() -> String {
    current_app::LOADING_TILE_NAME.to_owned()
}

/// The name of a tile whose page failed to load.
#[uniffi::export]
pub fn failed_tile_name() -> String {
    current_app::FAILED_TILE_NAME.to_owned()
}

/// A person's part when the server gives no role ("Director", "Guest star"); empty for an actor or a part Current has no name for.
#[uniffi::export]
pub fn person_kind_name(kind: String) -> String {
    current_app::person_kind_name(&model::PersonKind::new(kind)).to_owned()
}

/// The title of the notice for a played or favourite mark that failed ("Couldn't mark Name as played.").
#[uniffi::export]
pub fn action_failed_title(action: ItemAction, item_name: Option<String>) -> String {
    let action = match action {
        ItemAction::SetPlayed { played } => current_app::ItemAction::SetPlayed(played),
        ItemAction::SetFavourite { favourite } => current_app::ItemAction::SetFavourite(favourite),
    };
    current_app::action_failed_title(action, item_name.as_deref())
}

/// The title of the notice shown when the library on screen left its server's list.
#[uniffi::export]
pub fn library_gone_title() -> String {
    current_app::LIBRARY_GONE_TITLE.to_owned()
}

/// The message of the notice shown when the library on screen left its server's list.
#[uniffi::export]
pub fn library_gone_message() -> String {
    current_app::LIBRARY_GONE_MESSAGE.to_owned()
}

/// What a detail page's facts line says: the parts of [`super::ItemDetails`] it shows.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DetailsFacts {
    pub year: Option<i32>,
    /// A series' run ([`super::ItemDetails::run`]); shown instead of the year.
    pub run: Option<SeriesRun>,
    pub runtime_ms: Option<u64>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
}

#[uniffi::export]
impl Core {
    /// The UI's digit grouping ("12,345 items"): its group separator and group sizes, from the decimal separator leftwards, the
    /// last repeating and a last 0 leaving the rest ungrouped (as .NET's `NumberGroupSizes`). A platform fact passed once at start.
    pub fn set_digit_grouping(&self, separator: String, sizes: Vec<u32>) {
        self.app.set_digit_grouping(&separator, &sizes);
    }

    /// A size on disk in binary units, at most one decimal below 10: "512 bytes", "1.5 KB", "12 MB".
    pub fn size_on_disk(&self, bytes: u64) -> String {
        current_app::size_on_disk(bytes, &self.app.number_format())
    }

    /// A library grid's count: "1 item", "12,345 items", or empty for none.
    pub fn item_count_label(&self, total: u64) -> String {
        current_app::item_count_label(total, &self.app.number_format())
    }

    /// A detail page's facts line: "2019 · 1 h 30 min · PG-13 · 7.3/10", a series' run in place of the year ("2019–2023").
    pub fn details_facts(&self, facts: DetailsFacts) -> String {
        current_app::facts_line(
            &current_app::DetailsFacts {
                year: facts.year,
                run: facts.run.map(|run| current_app::SeriesRun {
                    start_year: run.start_year,
                    end_year: run.end_year,
                    continuing: run.continuing,
                }),
                runtime: facts.runtime_ms.map(Duration::from_millis),
                official_rating: facts.official_rating,
                community_rating: facts.community_rating,
            },
            &self.app.number_format(),
        )
    }

    /// The search page's status line: "Searching…" until the first of `found` results, empty once there are results, "No
    /// results", or "Couldn't search. " and why it `failed`. On the Favourites tab (`favourites`), its own lines with why.
    pub fn search_status(
        &self,
        found: u32,
        loading: bool,
        failed: Option<BrowseProblem>,
        favourites: bool,
    ) -> String {
        current_app::search_status(
            usize::try_from(found).unwrap_or(usize::MAX),
            loading,
            failed.map(Into::into).as_ref(),
            favourites,
            self.app.platform(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::NetProblem;

    #[test]
    fn the_browse_texts_cross_the_bridge() {
        let core = Core::new().unwrap();
        core.set_decimal_separator(",".into());
        core.set_digit_grouping(".".into(), vec![3]);
        assert_eq!(core.size_on_disk(1536), "1,5 KB");
        assert_eq!(core.item_count_label(12_345), "12.345 items");
        assert_eq!(
            core.details_facts(DetailsFacts {
                year: Some(2019),
                run: Some(SeriesRun {
                    start_year: 2019,
                    end_year: None,
                    continuing: true,
                }),
                runtime_ms: Some(45 * 60_000),
                official_rating: None,
                community_rating: Some(7.25),
            }),
            "2019– · 45 min · 7,3/10"
        );
        assert_eq!(
            core.search_status(
                0,
                false,
                Some(BrowseProblem::Network {
                    problem: NetProblem::Timeout
                }),
                false
            ),
            "Couldn't search. The server took too long to answer."
        );
        assert_eq!(person_kind_name("guest_star".into()), "Guest star");
        assert_eq!(
            action_failed_title(ItemAction::SetPlayed { played: true }, None),
            "Couldn't mark the item as played."
        );
        assert_eq!(library_gone_title(), "This library is no longer available");
    }
}
