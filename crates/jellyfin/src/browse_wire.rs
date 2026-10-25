//! Jellyfin's wire types for browsing (ADR 0018), the fields this crate reads. Private: the public API speaks `current-model`.
//!
//! Shapes from Jellyfin 12.2 (`MediaBrowser.Model/Dto/BaseItemDto.cs`, `UserItemDataDto.cs`, `MediaBrowser.Model/Querying/
//! QueryResult.cs`), serialized with PascalCase names, enums as their names and ids as 32 hex digits. Every field is optional here:
//! a list query sends only the fields it asks for, and a field that is missing maps to "not known", never to an error.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

/// Reads `null` as the empty value. Jellyfin 12.2 leaves nulls out (`JsonDefaults`: `WhenWritingNull`), but a list that arrives as
/// `null` still means "none".
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `QueryResult<BaseItemDto>`. `TotalRecordCount` is 0 when the query was sent with `enableTotalRecordCount=false`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct ItemsResult {
    #[serde(deserialize_with = "null_as_default")]
    pub items: Vec<BaseItemDto>,
    pub total_record_count: Option<i64>,
    pub start_index: Option<i64>,
}

/// `BaseItemDto`, the fields browsing reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct BaseItemDto {
    pub id: Option<String>,
    pub name: Option<String>,
    /// `BaseItemKind`.
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    /// `CollectionType`, on views (libraries).
    pub collection_type: Option<String>,
    pub production_year: Option<i32>,
    pub run_time_ticks: Option<i64>,
    pub user_data: Option<UserItemDataDto>,
    pub child_count: Option<i64>,

    #[serde(deserialize_with = "null_as_default")]
    pub image_tags: BTreeMap<String, String>,
    #[serde(deserialize_with = "null_as_default")]
    pub backdrop_image_tags: Vec<String>,
    /// Image type, then tag, then the BlurHash.
    #[serde(deserialize_with = "null_as_default")]
    pub image_blur_hashes: BTreeMap<String, BTreeMap<String, String>>,
    pub primary_image_aspect_ratio: Option<f64>,
    /// An episode's or season's series poster. Set whatever `fields` asks (see `docs/notes/jellyfin-browse.md`).
    pub series_primary_image_tag: Option<String>,

    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season_id: Option<String>,
    pub season_name: Option<String>,
    pub index_number: Option<i64>,
    pub index_number_end: Option<i64>,
    pub parent_index_number: Option<i64>,

    // Details only (`Items/{id}` returns every field).
    pub original_title: Option<String>,
    pub overview: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub taglines: Vec<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub genres: Vec<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub studios: Vec<NameGuidPair>,
    #[serde(deserialize_with = "null_as_default")]
    pub people: Vec<BaseItemPerson>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub premiere_date: Option<String>,
    pub end_date: Option<String>,
    pub status: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub chapters: Vec<ChapterInfo>,
    pub container: Option<String>,
    pub media_source_count: Option<i64>,
    #[serde(deserialize_with = "null_as_default")]
    pub media_sources: Vec<MediaSourceInfo>,
    #[serde(deserialize_with = "null_as_default")]
    pub media_streams: Vec<MediaStream>,
}

/// `UserItemDataDto`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct UserItemDataDto {
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<i64>,
    pub playback_position_ticks: Option<i64>,
    pub play_count: Option<i64>,
    pub is_favorite: Option<bool>,
    pub played: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct NameGuidPair {
    pub name: Option<String>,
}

/// `BaseItemPerson`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct BaseItemPerson {
    pub id: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    /// `PersonKind`.
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub primary_image_tag: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub image_blur_hashes: BTreeMap<String, BTreeMap<String, String>>,
}

/// `ChapterInfo`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct ChapterInfo {
    pub start_position_ticks: Option<i64>,
    pub name: Option<String>,
}

/// `MediaSourceInfo`, the fields the detail page's summary reads (ADR 0019 owns the rest).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaSourceInfo {
    pub container: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub media_streams: Vec<MediaStream>,
}

/// `MediaStream`, the fields the detail page's summary reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaStream {
    /// `MediaStreamType`: `Video`, `Audio`, `Subtitle`, ...
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub display_title: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
}
