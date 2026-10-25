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
    /// Sort keys, most significant first. The service crate adds the name as the last key, so equal keys keep a stable order.
    pub sort: Vec<SortKey>,
    pub order: SortOrder,
    pub filters: Vec<ItemFilter>,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    /// Only items whose name starts with this.
    pub name_starts_with: Option<String>,
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
    fn queries_default_to_the_servers_order() {
        let query = ItemQuery::default();
        assert!(query.sort.is_empty());
        assert_eq!(query.order, SortOrder::Ascending);
        assert!(!query.recursive);
    }
}
