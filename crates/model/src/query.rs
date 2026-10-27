//! What a library grid asks for: which items, in which order (ADR 0018). Each service crate maps these to its server's parameters.

use crate::identity::{ItemRef, MediaKind};
use crate::open::open_value;

open_value! {
    /// What a list is sorted by. Open: a key Current does not know is sent as it is.
    SortKey {
        SORT_NAME = "sort_name",
        DATE_CREATED = "date_created",
        PREMIERE_DATE = "premiere_date",
        PRODUCTION_YEAR = "production_year",
        COMMUNITY_RATING = "community_rating",
        RUNTIME = "runtime",
        DATE_PLAYED = "date_played",
        PLAY_COUNT = "play_count",
        RANDOM = "random",
    }
}

/// Which way a list is sorted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

open_value! {
    /// A condition every listed item meets. Open: a filter Current does not know is sent as it is.
    ItemFilter {
        PLAYED = "played",
        UNPLAYED = "unplayed",
        FAVOURITE = "favourite",
        /// Started and not finished.
        RESUMABLE = "resumable",
    }
}

/// Which items a list holds and in which order. Empty fields leave the server's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemQuery {
    /// The library or folder to list.
    pub parent: Option<ItemRef>,
    /// Only items of these kinds.
    pub kinds: Vec<MediaKind>,
    /// Whether items inside folders below `parent` are listed too.
    pub recursive: bool,
    /// Sort keys, most significant first. The service crate adds the name as the last key, so equal keys keep a stable order. None
    /// with a search term: the server's relevance order.
    pub sort: Vec<SortKey>,
    pub order: SortOrder,
    pub filters: Vec<ItemFilter>,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    /// Only items whose name starts with this.
    pub name_starts_with: Option<String>,
    /// Only items the server finds for this, as its search does (a library's own search box).
    pub search_term: Option<String>,
}

impl ItemQuery {
    /// Whether the user's marks decide which items the list holds or their order: a filter on played or favourite marks, or the date
    /// played. Such a list must be read again when a mark changes, as an item can join or leave it.
    pub fn depends_on_marks(&self) -> bool {
        !self.filters.is_empty() || self.sort.contains(&SortKey::DATE_PLAYED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_and_filters_pass_through() {
        assert!(SortKey::from("date_created").is_known());
        let key = SortKey::new("SeriesDatePlayed");
        assert!(!key.is_known());
        assert_eq!(key.as_str(), "SeriesDatePlayed");
        assert!(!ItemFilter::new("Likes").is_known());
    }

    #[test]
    fn a_list_filtered_or_ordered_by_marks_depends_on_them() {
        assert!(!ItemQuery::default().depends_on_marks());
        let favourites = ItemQuery {
            filters: vec![ItemFilter::FAVOURITE],
            ..ItemQuery::default()
        };
        assert!(favourites.depends_on_marks());
        let seen = ItemQuery {
            sort: vec![SortKey::DATE_PLAYED],
            ..ItemQuery::default()
        };
        assert!(seen.depends_on_marks());
        let names = ItemQuery {
            sort: vec![SortKey::SORT_NAME],
            ..ItemQuery::default()
        };
        assert!(!names.depends_on_marks());
    }

    #[test]
    fn queries_default_to_the_servers_order() {
        let query = ItemQuery::default();
        assert!(query.sort.is_empty());
        assert_eq!(query.order, SortOrder::Ascending);
        assert!(!query.recursive);
    }
}
