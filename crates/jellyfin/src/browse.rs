//! Browsing a Jellyfin library (ADR 0018): libraries, item lists with paging, sorting and filters, the home rows, series, search,
//! details, and the played and favourite marks. Every list query sends the summary field set; details read the whole item.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use current_model::{
    Artwork, AspectRatio, CalendarDate, Chapter, CollectionKind, EpisodeLinks, ImageRef, ImageType,
    ItemDetails, ItemFilter, ItemId, ItemQuery, ItemRef, ItemSummary, Library, MediaKind,
    MediaSummary, Person, PersonKind, SortKey, SortOrder, UserState,
};
use current_net::{Method, Request};

use crate::browse_wire::{self as wire, BaseItemDto, ItemsResult, UserItemDataDto};
use crate::client::{AuthError, JellyfinClient};
use crate::playback;

/// How many items one search group returns. Search is not paged: Jellyfin 12.2 asks its search providers for three times `limit`
/// candidates and pages within those, so later pages are not the next best matches (`docs/notes/jellyfin-browse.md`).
pub const SEARCH_LIMIT: u32 = 24;

/// The search groups, in the order they are shown: one request each.
pub const SEARCH_GROUPS: &[MediaKind] = &[
    MediaKind::MOVIE,
    MediaKind::SERIES,
    MediaKind::EPISODE,
    MediaKind::BOX_SET,
    MediaKind::MUSIC_ARTIST,
    MediaKind::MUSIC_ALBUM,
    MediaKind::AUDIO,
    MediaKind::MUSIC_VIDEO,
    MediaKind::VIDEO,
];

/// How far back "next up" looks for series in progress, until a setting exists (ADR 0018).
pub const NEXT_UP_CUTOFF: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The summary field set (ADR 0018): the aspect ratio of the primary image, at most one image per type of the three a tile shows.
/// User data stays on, the server's default.
const SUMMARY_FIELDS: &str = "PrimaryImageAspectRatio";
const SUMMARY_IMAGE_TYPES: &str = "Primary,Thumb,Backdrop";
const SUMMARY_IMAGE_LIMIT: &str = "1";

/// Episodes asked for at a time while a series' first episode is looked for past its specials: one page unless it has more specials.
const FIRST_EPISODE_PAGE: usize = 50;

/// One page of a list.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub items: Vec<ItemSummary>,
    /// The index of the first item in the whole list.
    pub start: u32,
    /// How many items the whole list holds, when the request asked for it.
    pub total: Option<u32>,
}

/// One group of search results: items of one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchGroup {
    pub kind: MediaKind,
    pub items: Vec<ItemSummary>,
}

impl JellyfinClient {
    /// The user's libraries (`GET UserViews`), in the user's order and without the views they hid.
    pub async fn libraries(&self) -> Result<Vec<Library>, AuthError> {
        let result: ItemsResult = self.get_json(Request::get("/UserViews")).await?;
        Ok(result
            .items
            .into_iter()
            .filter_map(|dto| self.library(dto))
            .collect())
    }

    /// A page of `query`'s items (`GET Items`), from `start`, at most `limit`. With `with_total`, the answer carries the total, which
    /// costs the server a count: ask for it on a first page and a refresh only. A query with a search term and no sort keys sends no
    /// sort, so the best matches come first; a search answers at most three times `limit` items, and its total counts only those
    /// (`docs/notes/jellyfin-browse.md`, "Search").
    pub async fn items(
        &self,
        query: &ItemQuery,
        start: u32,
        limit: u32,
        with_total: bool,
    ) -> Result<Page, AuthError> {
        let mut request = summary(Request::get("/Items"))
            .query("startIndex", start.to_string())
            .query("limit", limit.to_string())
            .query("recursive", query.recursive.to_string())
            .query("enableTotalRecordCount", with_total.to_string());
        if let Some(parent) = &query.parent {
            request = request.query("parentId", parent.id.as_str());
        }
        if !query.kinds.is_empty() {
            request = request.query("includeItemTypes", join(query.kinds.iter().map(kind_wire)));
        }
        // A search without sort keys keeps the server's order, the best matches first ("ordered by score then `SortName`",
        // `docs/notes/jellyfin-browse.md`, "Search"): no `sortBy`, and no `sortOrder` with it.
        if query.search_term.is_none() || !query.sort.is_empty() {
            request = request
                .query("sortBy", sort_by(&query.sort))
                .query("sortOrder", sort_order(query.order));
        }
        if !query.filters.is_empty() {
            request = request.query("filters", join(query.filters.iter().map(filter_wire)));
        }
        if !query.genres.is_empty() {
            // Genres can hold commas: the server splits this one on `|`.
            request = request.query("genres", query.genres.join("|"));
        }
        if !query.years.is_empty() {
            request = request.query("years", join(query.years.iter().map(i32::to_string)));
        }
        if let Some(prefix) = &query.name_starts_with {
            request = request.query("nameStartsWith", prefix.as_str());
        }
        if let Some(term) = &query.search_term {
            request = request.query("searchTerm", term.as_str());
        }
        let result: ItemsResult = self.get_json(request).await?;
        let total = with_total
            .then(|| result.total_record_count.and_then(to_u32))
            .flatten();
        let start = result.start_index.and_then(to_u32).unwrap_or(start);
        Ok(Page {
            items: self.summaries(result.items),
            start,
            total,
        })
    }

    /// The newest items in `library` (`GET Items/Latest`), with new episodes of one series grouped into the series
    /// ([`ItemSummary::child_count`] says how many).
    pub async fn latest(
        &self,
        library: &ItemRef,
        limit: u32,
    ) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get("/Items/Latest"))
            .query("parentId", library.id.as_str())
            .query("limit", limit.to_string())
            .query("groupItems", "true");
        // A bare array, not a query result.
        let items: Vec<BaseItemDto> = self.get_json(request).await?;
        Ok(self.summaries(items))
    }

    /// Videos the user started and did not finish (`GET UserItems/Resume`), most recent first: "continue watching". Named apart
    /// from [`JellyfinClient::resume`], which ends an answer check.
    pub async fn resume_items(&self, limit: u32) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get("/UserItems/Resume"))
            .query("limit", limit.to_string())
            .query("mediaTypes", "Video")
            .query("enableTotalRecordCount", "false");
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// The next episode of each series in progress (`GET Shows/NextUp`), for series watched since `cutoff` (now minus
    /// [`NEXT_UP_CUTOFF`] until a setting exists). Episodes already started are left to [`JellyfinClient::resume_items`].
    pub async fn next_up(
        &self,
        limit: u32,
        cutoff: SystemTime,
    ) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get("/Shows/NextUp"))
            .query("limit", limit.to_string())
            .query("enableResumable", "false")
            .query("enableTotalRecordCount", "false")
            .query("nextUpDateCutoff", utc_timestamp(cutoff));
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// The episode a series' Play button starts (`GET Shows/NextUp?seriesId=`): its next unplayed episode, or the one in progress,
    /// since `enableResumable` stays at its default, true ("when [resumable episodes are not allowed and] the next episode has saved
    /// playback progress, no next-up item is returned", `TVSeriesManager.DetermineNextEpisode`, v12.2 at ca0f16eb, read 2026-10-25).
    /// No date cutoff: a series watched long ago still has its next episode. `None` when the server names none (a series never
    /// started or watched to its end: not verified which the server answers for a series never started).
    pub async fn series_next_up(&self, series: &ItemRef) -> Result<Option<ItemSummary>, AuthError> {
        let request = summary(Request::get("/Shows/NextUp"))
            .query("seriesId", series.id.as_str())
            .query("limit", "1")
            .query("enableTotalRecordCount", "false");
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items).into_iter().next())
    }

    /// A series' first episode that is not a special (season 0), for its Play button when it has no next episode; its first special
    /// when it has only specials. `GET Shows/{id}/Episodes?isMissing=false`, a page at a time. Without a season the server lists every
    /// season's episodes in season order, seasons sort by their number padded to four digits, so Specials ("0000") come first, and a
    /// user who shows missing episodes gets the virtual ones too; `isMissing` filters before the paging (`TvShowsController.GetEpisodes`,
    /// `Series.GetEpisodes`, `Season.CreateSortName`, jellyfin tag v12.3, read 2026-10-25).
    pub async fn first_episode(&self, series: &ItemRef) -> Result<Option<ItemSummary>, AuthError> {
        let mut special = None;
        let mut start = 0;
        loop {
            let request = summary(Request::get(format!(
                "/Shows/{}/Episodes",
                series.id.as_str()
            )))
            .query("isMissing", "false")
            .query("startIndex", start.to_string())
            .query("limit", FIRST_EPISODE_PAGE.to_string());
            let result: ItemsResult = self.get_json(request).await?;
            let count = result.items.len();
            for episode in self.summaries(result.items) {
                let season = episode
                    .episode
                    .as_ref()
                    .and_then(|links| links.season_number);
                if season == Some(0) {
                    special.get_or_insert(episode);
                } else {
                    return Ok(Some(episode));
                }
            }
            if count < FIRST_EPISODE_PAGE {
                return Ok(special);
            }
            start += count;
        }
    }

    /// A series' seasons (`GET Shows/{id}/Seasons`), in order.
    pub async fn seasons(&self, series: &ItemRef) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get(format!(
            "/Shows/{}/Seasons",
            series.id.as_str()
        )));
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// A season's episodes (`GET Shows/{id}/Episodes`), in order.
    pub async fn episodes(
        &self,
        series: &ItemRef,
        season: &ItemRef,
    ) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get(format!(
            "/Shows/{}/Episodes",
            series.id.as_str()
        )))
        .query("seasonId", season.id.as_str());
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// The episode after `episode` in its series, across seasons (`GET Shows/{series}/Episodes?startItemId=`, which skips "through
    /// the list until a given item is found", `TvShowsController.cs` at v12.2; docs/notes/playback.md section 5). `None` for the last
    /// episode, and for an item that is not an episode of a series.
    pub async fn next_episode(&self, episode: &ItemRef) -> Result<Option<ItemSummary>, AuthError> {
        let details = self.details(episode).await?;
        let Some(series) = details
            .summary
            .episode
            .as_ref()
            .and_then(|links| links.series.clone())
        else {
            return Ok(None);
        };
        let request = summary(Request::get(format!(
            "/Shows/{}/Episodes",
            series.id.as_str()
        )))
        .query("startItemId", episode.id.as_str())
        .query("limit", "2");
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self
            .summaries(result.items)
            .into_iter()
            .skip_while(|summary| summary.item.id != episode.id)
            .nth(1))
    }

    /// Items like `item` (`GET Items/{id}/Similar`). The endpoint takes `fields` and `limit` only.
    pub async fn similar(&self, item: &ItemRef, limit: u32) -> Result<Vec<ItemSummary>, AuthError> {
        let request = Request::get(format!("/Items/{}/Similar", item.id.as_str()))
            .query("fields", SUMMARY_FIELDS)
            .query("limit", limit.to_string());
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// Searches for `term` (`GET Items?searchTerm=`): one request per kind in `kinds` (usually [`SEARCH_GROUPS`]), sent together,
    /// each answering its best `limit` matches. Groups come back in the order of `kinds`; a group without matches is left out. Not
    /// paged: see [`SEARCH_LIMIT`]. The term is redacted in the request log, as every query value outside paging and sorting is.
    pub async fn search(
        &self,
        term: &str,
        kinds: &[MediaKind],
        limit: u32,
    ) -> Result<Vec<SearchGroup>, AuthError> {
        let requests = kinds
            .iter()
            .map(|kind| self.search_group(term, kind, limit))
            .collect();
        let mut groups = Vec::with_capacity(kinds.len());
        for (kind, result) in kinds.iter().zip(join_all(requests).await) {
            let items = result?;
            if !items.is_empty() {
                groups.push(SearchGroup {
                    kind: kind.clone(),
                    items,
                });
            }
        }
        Ok(groups)
    }

    async fn search_group(
        &self,
        term: &str,
        kind: &MediaKind,
        limit: u32,
    ) -> Result<Vec<ItemSummary>, AuthError> {
        let request = summary(Request::get("/Items"))
            .query("searchTerm", term)
            .query("includeItemTypes", kind_wire(kind))
            .query("recursive", "true")
            .query("limit", limit.to_string())
            .query("enableTotalRecordCount", "false");
        let result: ItemsResult = self.get_json(request).await?;
        Ok(self.summaries(result.items))
    }

    /// Everything a detail page shows of `item` (`GET Items/{id}`, which returns every field).
    pub async fn details(&self, item: &ItemRef) -> Result<ItemDetails, AuthError> {
        let dto: BaseItemDto = self
            .get_json(Request::get(format!("/Items/{}", item.id.as_str())))
            .await?;
        Ok(self.item_details(dto, item))
    }

    /// Marks `item` played or unplayed (`POST`/`DELETE UserPlayedItems/{id}`). Returns the state the server now holds.
    pub async fn set_played(&self, item: &ItemRef, played: bool) -> Result<UserState, AuthError> {
        let path = format!("/UserPlayedItems/{}", item.id.as_str());
        self.set_mark(path, played).await
    }

    /// Marks `item` a favourite or not (`POST`/`DELETE UserFavoriteItems/{id}`). Returns the state the server now holds.
    pub async fn set_favourite(
        &self,
        item: &ItemRef,
        favourite: bool,
    ) -> Result<UserState, AuthError> {
        let path = format!("/UserFavoriteItems/{}", item.id.as_str());
        self.set_mark(path, favourite).await
    }

    /// `item`'s marks as the server holds them now (`GET UserItems/{id}/UserData`, which answers what the mark requests do: for a
    /// series or season, its played state, unplayed count and played percentage derived from its episodes; `ItemsController
    /// .GetItemUserData` and `Folder.FillUserDataDtoValues`, v12.2 at ca0f16eb, read 2026-10-27).
    pub async fn user_data(&self, item: &ItemRef) -> Result<UserState, AuthError> {
        let path = format!("/UserItems/{}/UserData", item.id.as_str());
        let data: UserItemDataDto = self.get_json(Request::get(path)).await?;
        Ok(user_state(Some(data)))
    }

    async fn set_mark(&self, path: String, on: bool) -> Result<UserState, AuthError> {
        let method = if on { Method::POST } else { Method::DELETE };
        let data: UserItemDataDto = self.get_json(Request::new(method, path)).await?;
        Ok(user_state(Some(data)))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        request: Request,
    ) -> Result<T, AuthError> {
        Ok(self.send_authenticated(request).await?.json()?)
    }

    fn item_ref(&self, id: &str) -> ItemRef {
        ItemRef::new(self.server().clone(), ItemId::new(id))
    }

    fn summaries(&self, items: Vec<BaseItemDto>) -> Vec<ItemSummary> {
        items
            .into_iter()
            .filter_map(|dto| {
                let item = self.item_ref(dto.id.as_deref().filter(|id| !id.is_empty())?);
                Some(self.summary(&dto, item))
            })
            .collect()
    }

    fn library(&self, dto: BaseItemDto) -> Option<Library> {
        let item = self.item_ref(dto.id.as_deref().filter(|id| !id.is_empty())?);
        Some(Library {
            artwork: summary_artwork(&dto, &item, |id| self.item_ref(id)),
            name: dto.name.unwrap_or_default(),
            collection: dto.collection_type.as_deref().map(collection_kind),
            item,
        })
    }

    fn summary(&self, dto: &BaseItemDto, item: ItemRef) -> ItemSummary {
        let kind = dto.kind.as_deref().map_or(MediaKind::new(""), media_kind);
        let episode = (kind == MediaKind::EPISODE).then(|| EpisodeLinks {
            series: dto.series_id.as_deref().map(|id| self.item_ref(id)),
            series_name: dto.series_name.clone(),
            season: dto.season_id.as_deref().map(|id| self.item_ref(id)),
            season_name: dto.season_name.clone(),
            number: dto.index_number.and_then(to_u32),
            number_end: dto.index_number_end.and_then(to_u32),
            season_number: dto.parent_index_number.and_then(to_u32),
        });
        ItemSummary {
            artwork: summary_artwork(dto, &item, |id| self.item_ref(id)),
            item,
            kind,
            name: dto.name.clone().unwrap_or_default(),
            year: dto.production_year,
            runtime: dto.run_time_ticks.and_then(ticks),
            user: user_state(dto.user_data.clone()),
            episode,
            child_count: dto.child_count.and_then(to_u32),
        }
    }

    fn item_details(&self, dto: BaseItemDto, asked: &ItemRef) -> ItemDetails {
        let item = dto
            .id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map_or_else(|| asked.clone(), |id| self.item_ref(id));
        let summary = self.summary(&dto, item.clone());
        let backdrops = dto
            .backdrop_image_tags
            .iter()
            .enumerate()
            .map(|(index, tag)| {
                artwork_of(
                    &dto,
                    &item,
                    "Backdrop",
                    u32::try_from(index).unwrap_or(u32::MAX),
                    tag,
                )
            })
            .collect();
        let logo = dto
            .image_tags
            .get("Logo")
            .map(|tag| artwork_of(&dto, &item, "Logo", 0, tag));
        let people = dto
            .people
            .iter()
            .filter_map(|person| self.person(person))
            .collect();
        let chapters = dto
            .chapters
            .iter()
            .filter_map(|chapter| {
                Some(Chapter {
                    start: chapter.start_position_ticks.and_then(ticks)?,
                    name: chapter.name.clone(),
                })
            })
            .collect();
        ItemDetails {
            summary,
            original_title: dto.original_title.clone(),
            overview: dto.overview.clone(),
            taglines: dto.taglines.clone(),
            genres: dto.genres.clone(),
            studios: dto.studios.iter().filter_map(|s| s.name.clone()).collect(),
            people,
            official_rating: dto.official_rating.clone(),
            community_rating: dto.community_rating,
            critic_rating: dto.critic_rating,
            premiere: dto.premiere_date.as_deref().and_then(calendar_date),
            end: dto.end_date.as_deref().and_then(calendar_date),
            status: dto.status.clone(),
            backdrops,
            logo,
            chapters,
            media: media_summary(&dto),
        }
    }

    fn person(&self, person: &wire::BaseItemPerson) -> Option<Person> {
        let id = person.id.as_deref().filter(|id| !id.is_empty())?;
        let item = self.item_ref(id);
        let image = person.primary_image_tag.as_ref().map(|tag| Artwork {
            kind: ImageType::PRIMARY,
            image: ImageRef {
                item: item.clone(),
                kind: ImageType::PRIMARY,
                index: 0,
                tag: tag.clone(),
            },
            aspect_ratio: None,
            blurhash: blurhash(&person.image_blur_hashes, "Primary", tag),
        });
        Some(Person {
            person: item,
            name: person.name.clone().unwrap_or_default(),
            role: person.role.clone().filter(|role| !role.is_empty()),
            kind: person
                .kind
                .as_deref()
                .map_or(PersonKind::new(""), person_kind),
            image,
        })
    }
}

/// Adds the summary field set to a list query.
fn summary(request: Request) -> Request {
    request
        .query("fields", SUMMARY_FIELDS)
        .query("enableImageTypes", SUMMARY_IMAGE_TYPES)
        .query("imageTypeLimit", SUMMARY_IMAGE_LIMIT)
}

/// The image types a summary carries, in the order tiles pick them.
const SUMMARY_ORDER: &[&str] = &["Primary", "Thumb", "Backdrop", "Logo", "Banner"];

/// At most one image per type: the item's own, then (for an episode or season without a primary image) the series' poster.
fn summary_artwork(
    dto: &BaseItemDto,
    item: &ItemRef,
    item_ref: impl Fn(&str) -> ItemRef,
) -> Vec<Artwork> {
    let mut artwork = Vec::new();
    for kind in SUMMARY_ORDER {
        let tag = if *kind == "Backdrop" {
            dto.backdrop_image_tags.first()
        } else {
            dto.image_tags.get(*kind)
        };
        if let Some(tag) = tag {
            artwork.push(artwork_of(dto, item, kind, 0, tag));
        }
    }
    for (kind, tag) in &dto.image_tags {
        if !SUMMARY_ORDER.contains(&kind.as_str()) {
            artwork.push(artwork_of(dto, item, kind, 0, tag));
        }
    }
    if !dto.image_tags.contains_key("Primary")
        && let (Some(tag), Some(series)) = (&dto.series_primary_image_tag, &dto.series_id)
    {
        // The server then sets `PrimaryImageAspectRatio` from the series' poster too (`DtoService`, Jellyfin 12.2).
        artwork.insert(0, artwork_of(dto, &item_ref(series), "Primary", 0, tag));
    }
    artwork
}

fn artwork_of(dto: &BaseItemDto, item: &ItemRef, kind: &str, index: u32, tag: &str) -> Artwork {
    let image_type = image_type(kind);
    let aspect_ratio = if kind == "Primary" {
        dto.primary_image_aspect_ratio.and_then(aspect_ratio)
    } else {
        None
    };
    Artwork {
        kind: image_type.clone(),
        image: ImageRef {
            item: item.clone(),
            kind: image_type,
            index,
            tag: tag.to_owned(),
        },
        aspect_ratio,
        blurhash: blurhash(&dto.image_blur_hashes, kind, tag),
    }
}

fn blurhash(
    hashes: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    kind: &str,
    tag: &str,
) -> Option<String> {
    hashes.get(kind)?.get(tag).cloned()
}

/// A ratio the server sends as a double (width / height) as the nearest small whole-number ratio within 0.05 %.
fn aspect_ratio(ratio: f64) -> Option<AspectRatio> {
    if !ratio.is_finite() || ratio <= 0.0 || ratio > 100.0 {
        return None;
    }
    (1..=1000u32).find_map(|height| {
        let width = (ratio * f64::from(height)).round();
        if width < 1.0 {
            return None;
        }
        ((width / f64::from(height) - ratio).abs() <= ratio * 0.0005)
            // `width` is at most 100 × 1000, so the cast is exact.
            .then(|| AspectRatio::new(width as u32, height))
            .flatten()
    })
}

fn user_state(data: Option<UserItemDataDto>) -> UserState {
    let Some(data) = data else {
        return UserState::default();
    };
    UserState {
        played: data.played.unwrap_or(false),
        position: data
            .playback_position_ticks
            .filter(|ticks| *ticks > 0)
            .and_then(ticks),
        // A percentage, clamped to 0..=1 first: the cast to f32 loses only precision.
        progress: data
            .played_percentage
            .filter(|p| p.is_finite())
            .map(|p| (p / 100.0).clamp(0.0, 1.0) as f32),
        favourite: data.is_favorite.unwrap_or(false),
        unplayed_count: data.unplayed_item_count.and_then(to_u32),
        play_count: data.play_count.and_then(to_u32).unwrap_or(0),
    }
}

/// The summary of the item's first media source (ADR 0019: derived from the playback types), or of the item's own streams when it
/// lists no source.
fn media_summary(dto: &BaseItemDto) -> Option<MediaSummary> {
    let source = dto.media_sources.first().map(playback::map_source);
    if source.is_none() && dto.media_streams.is_empty() {
        return None;
    }
    let source_count = dto
        .media_source_count
        .and_then(to_u32)
        .unwrap_or_else(|| u32::try_from(dto.media_sources.len()).unwrap_or(u32::MAX));
    Some(match source {
        Some(source) if !source.streams.is_empty() => {
            MediaSummary::from_source(&source, source_count)
        }
        source => MediaSummary::from_streams(
            source_count,
            source
                .and_then(|source| source.container)
                .or_else(|| dto.container.clone()),
            &playback::map_streams(&dto.media_streams),
        ),
    })
}

/// Jellyfin's ticks (100 ns) as a duration; `None` for a negative count.
fn ticks(ticks: i64) -> Option<Duration> {
    let ticks = u64::try_from(ticks).ok()?;
    // Below 10^7, so the nanoseconds fit in u32.
    let nanos = u32::try_from((ticks % 10_000_000) * 100).ok()?;
    Some(Duration::new(ticks / 10_000_000, nanos))
}

fn to_u32(value: i64) -> Option<u32> {
    u32::try_from(value).ok()
}

/// The date part of an ISO 8601 timestamp (`2010-07-16T00:00:00.0000000Z`), as the server wrote it, without converting zones.
fn calendar_date(value: &str) -> Option<CalendarDate> {
    let date = value.get(..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() || value.len() > 10 && !value[10..].starts_with('T') {
        return None;
    }
    CalendarDate::new(year, month, day)
}

/// `time` as an ISO 8601 UTC timestamp to the second (`2026-10-25T07:05:03Z`). Times before 1970 are written as 1970.
fn utc_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2);
    let of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`, public domain).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Day and month are 1..=31 and 1..=12 by construction.
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

fn join(values: impl Iterator<Item = impl AsRef<str>>) -> String {
    values.fold(String::new(), |mut joined, value| {
        if !joined.is_empty() {
            joined.push(',');
        }
        joined.push_str(value.as_ref());
        joined
    })
}

/// Polls every future until all are done, and returns their outputs in order. A dozen lines instead of a dependency.
async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut futures: Vec<Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (future, output) in futures.iter_mut().zip(outputs.iter_mut()) {
            if output.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(value) => *output = Some(value),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().flatten().collect()
}

// Wire names (Jellyfin 12.2: `BaseItemKind`, `CollectionType`, `ImageType`, `PersonKind`, `ItemSortBy`, `ItemFilter`) and the model's
// constants, where the meaning matches. Anything else passes through unchanged, in both directions.

const KINDS: &[(&str, MediaKind)] = &[
    ("Movie", MediaKind::MOVIE),
    ("Series", MediaKind::SERIES),
    ("Season", MediaKind::SEASON),
    ("Episode", MediaKind::EPISODE),
    ("Person", MediaKind::PERSON),
    ("MusicAlbum", MediaKind::MUSIC_ALBUM),
    ("Audio", MediaKind::AUDIO),
    ("MusicArtist", MediaKind::MUSIC_ARTIST),
    ("MusicVideo", MediaKind::MUSIC_VIDEO),
    ("Video", MediaKind::VIDEO),
    ("Folder", MediaKind::FOLDER),
    ("BoxSet", MediaKind::BOX_SET),
];

const COLLECTIONS: &[(&str, CollectionKind)] = &[
    ("movies", CollectionKind::MOVIES),
    ("tvshows", CollectionKind::TV_SHOWS),
    ("music", CollectionKind::MUSIC),
    ("musicvideos", CollectionKind::MUSIC_VIDEOS),
    ("homevideos", CollectionKind::HOME_VIDEOS),
    ("boxsets", CollectionKind::BOX_SETS),
    ("books", CollectionKind::BOOKS),
    ("photos", CollectionKind::PHOTOS),
    ("playlists", CollectionKind::PLAYLISTS),
];

const IMAGE_TYPES: &[(&str, ImageType)] = &[
    ("Primary", ImageType::PRIMARY),
    ("Backdrop", ImageType::BACKDROP),
    ("Logo", ImageType::LOGO),
    ("Thumb", ImageType::THUMB),
    ("Banner", ImageType::BANNER),
];

const PERSON_KINDS: &[(&str, PersonKind)] = &[
    ("Actor", PersonKind::ACTOR),
    ("Director", PersonKind::DIRECTOR),
    ("Writer", PersonKind::WRITER),
    ("Producer", PersonKind::PRODUCER),
    ("GuestStar", PersonKind::GUEST_STAR),
    ("Composer", PersonKind::COMPOSER),
];

const SORT_KEYS: &[(&str, SortKey)] = &[
    ("SortName", SortKey::SORT_NAME),
    ("DateCreated", SortKey::DATE_CREATED),
    ("PremiereDate", SortKey::PREMIERE_DATE),
    ("ProductionYear", SortKey::PRODUCTION_YEAR),
    ("CommunityRating", SortKey::COMMUNITY_RATING),
    ("Runtime", SortKey::RUNTIME),
    ("DatePlayed", SortKey::DATE_PLAYED),
    ("PlayCount", SortKey::PLAY_COUNT),
    ("Random", SortKey::RANDOM),
];

const FILTERS: &[(&str, ItemFilter)] = &[
    ("IsPlayed", ItemFilter::PLAYED),
    ("IsUnplayed", ItemFilter::UNPLAYED),
    ("IsFavorite", ItemFilter::FAVOURITE),
    ("IsResumable", ItemFilter::RESUMABLE),
];

fn from_wire<T: Clone + From<String>>(table: &[(&str, T)], wire: &str) -> T {
    table
        .iter()
        .find(|(name, _)| *name == wire)
        .map_or_else(|| T::from(wire.to_owned()), |(_, value)| value.clone())
}

fn to_wire<'a, T: PartialEq + AsRef<str>>(table: &[(&'static str, T)], value: &'a T) -> &'a str {
    table
        .iter()
        .find(|(_, known)| known == value)
        .map_or(value.as_ref(), |(name, _)| name)
}

fn media_kind(wire: &str) -> MediaKind {
    from_wire(KINDS, wire)
}

fn kind_wire(kind: &MediaKind) -> &str {
    to_wire(KINDS, kind)
}

fn collection_kind(wire: &str) -> CollectionKind {
    from_wire(COLLECTIONS, wire)
}

fn image_type(wire: &str) -> ImageType {
    from_wire(IMAGE_TYPES, wire)
}

fn person_kind(wire: &str) -> PersonKind {
    from_wire(PERSON_KINDS, wire)
}

fn filter_wire(filter: &ItemFilter) -> &str {
    to_wire(FILTERS, filter)
}

/// The keys, then `SortName` unless already there, so items with equal keys keep one order between pages.
fn sort_by(keys: &[SortKey]) -> String {
    let mut names: Vec<&str> = keys.iter().map(|key| to_wire(SORT_KEYS, key)).collect();
    if !names.contains(&"SortName") {
        names.push("SortName");
    }
    join(names.into_iter())
}

/// One order: the server applies the first order to every key without its own (`RequestHelpers.GetOrderBy`, Jellyfin 12.2).
fn sort_order(order: SortOrder) -> &'static str {
    match order {
        SortOrder::Ascending => "Ascending",
        SortOrder::Descending => "Descending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_100_nanoseconds() {
        assert_eq!(ticks(0), Some(Duration::ZERO));
        assert_eq!(ticks(1), Some(Duration::from_nanos(100)));
        assert_eq!(ticks(72_000_000_000), Some(Duration::from_secs(7200)));
        assert_eq!(
            ticks(12_345_678_901),
            Some(Duration::new(1234, 567_890_100))
        );
        assert_eq!(ticks(-1), None);
        assert!(ticks(i64::MAX).is_some());
    }

    #[test]
    fn ratios_become_small_whole_numbers() {
        assert_eq!(aspect_ratio(2.0 / 3.0), AspectRatio::new(2, 3));
        assert_eq!(aspect_ratio(16.0 / 9.0), AspectRatio::new(16, 9));
        assert_eq!(aspect_ratio(1.0), AspectRatio::new(1, 1));
        assert_eq!(aspect_ratio(0.0), None);
        assert_eq!(aspect_ratio(-1.0), None);
        assert_eq!(aspect_ratio(f64::NAN), None);
        assert!(aspect_ratio(1.78).is_some());
    }

    #[test]
    fn dates_are_read_from_timestamps() {
        assert_eq!(
            calendar_date("2010-07-16T00:00:00.0000000Z"),
            CalendarDate::new(2010, 7, 16)
        );
        assert_eq!(calendar_date("2010-07-16"), CalendarDate::new(2010, 7, 16));
        assert_eq!(calendar_date("2010-02-30T00:00:00Z"), None);
        assert_eq!(calendar_date("not a date"), None);
        assert_eq!(calendar_date("2010-07"), None);
    }

    #[test]
    fn timestamps_are_utc() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let time = UNIX_EPOCH + Duration::from_secs(1_793_000_000);
        assert_eq!(utc_timestamp(time), "2026-10-26T07:33:20Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_timestamp(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn sort_always_ends_with_the_name() {
        assert_eq!(sort_by(&[]), "SortName");
        assert_eq!(sort_by(&[SortKey::DATE_CREATED]), "DateCreated,SortName");
        assert_eq!(
            sort_by(&[SortKey::SORT_NAME, SortKey::PRODUCTION_YEAR]),
            "SortName,ProductionYear"
        );
        assert_eq!(
            sort_by(&[SortKey::new("SeriesDatePlayed")]),
            "SeriesDatePlayed,SortName"
        );
    }

    #[test]
    fn unknown_wire_values_pass_through() {
        assert_eq!(media_kind("Movie"), MediaKind::MOVIE);
        assert_eq!(media_kind("Playlist").as_str(), "Playlist");
        assert_eq!(kind_wire(&MediaKind::BOX_SET), "BoxSet");
        assert_eq!(kind_wire(&MediaKind::new("Playlist")), "Playlist");
        assert_eq!(collection_kind("tvshows"), CollectionKind::TV_SHOWS);
        assert_eq!(collection_kind("livetv").as_str(), "livetv");
        assert_eq!(image_type("Art").as_str(), "Art");
        assert_eq!(person_kind("Lyricist").as_str(), "Lyricist");
        assert_eq!(filter_wire(&ItemFilter::FAVOURITE), "IsFavorite");
        assert_eq!(filter_wire(&ItemFilter::new("Likes")), "Likes");
    }
}
