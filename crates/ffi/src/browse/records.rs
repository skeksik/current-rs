//! The browse records the bindings carry (ADR 0018, section 7): flat copies of the `current-model` browse types and of
//! `current-app`'s list states. Item ids and image tags cross; no address, URL or path does. Open values (kinds, image types,
//! collection kinds, sort keys, filters) cross as their strings, unchanged (ADR 0005). Durations cross as whole milliseconds.

use std::time::Duration;

use current_model as model;

use crate::ServerRef;
use crate::diff::list_diff;

use super::problems::BrowseProblem;

/// An item on one server: its server and the server's id for it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ItemRef {
    pub server: ServerRef,
    pub id: String,
}

/// One image of an item, as the item's artwork names it: what [`ImageRequest`] asks for.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ImageRef {
    pub item: ItemRef,
    /// The image type (`primary`, `backdrop`, `logo`, `thumb`, `banner`, or one the server named).
    pub kind: String,
    /// Which image of that type: 0 for the first.
    pub index: u32,
    /// The server's tag for this version of the image.
    pub tag: String,
}

/// An image's width to height, reduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AspectRatio {
    pub width: u32,
    pub height: u32,
}

/// One piece of an item's artwork.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Artwork {
    pub kind: String,
    pub image: ImageRef,
    pub aspect_ratio: Option<AspectRatio>,
    /// The image's BlurHash, for a placeholder through [`ImageSource::BlurHash`].
    pub blurhash: Option<String>,
}

/// The user's marks on an item.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct UserState {
    pub played: bool,
    /// The resume point, in milliseconds.
    pub position_ms: Option<u64>,
    /// How far the resume point is, from 0 to 1.
    pub progress: Option<f32>,
    pub favourite: bool,
    /// For a series or season: how many episodes are unplayed.
    pub unplayed_count: Option<u32>,
    pub play_count: u32,
}

/// Where an episode belongs.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct EpisodeLinks {
    pub series: Option<ItemRef>,
    pub series_name: Option<String>,
    pub season: Option<ItemRef>,
    pub season_name: Option<String>,
    pub number: Option<u32>,
    /// The last episode of a double episode.
    pub number_end: Option<u32>,
    pub season_number: Option<u32>,
    /// The core's label for the numbers: `S1:E2`, `S1:E2-E3`, `E5` or empty (`EpisodeLinks::number_label`). Ignored on the way in.
    pub label: String,
}

/// An item as a tile or a row shows it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ItemSummary {
    pub item: ItemRef,
    /// The media kind (`movie`, `series`, `episode`..., or one the server named).
    pub kind: String,
    pub name: String,
    pub year: Option<i32>,
    /// The running time, in milliseconds.
    pub runtime_ms: Option<u64>,
    pub artwork: Vec<Artwork>,
    pub user: UserState,
    pub episode: Option<EpisodeLinks>,
    /// How many items a folder, series or season holds.
    pub child_count: Option<u32>,
    /// The artwork a poster tile shows (2:3), as the core picks it from `artwork`.
    #[uniffi(default = None)]
    pub poster: Option<Artwork>,
    /// The artwork a wide tile shows (16:9), as the core picks it from `artwork`.
    #[uniffi(default = None)]
    pub wide: Option<Artwork>,
    /// What is left to watch from the resume point, in milliseconds (`ItemSummary::time_left`). Ignored on the way in.
    #[uniffi(default = None)]
    pub time_left_ms: Option<u64>,
}

/// One of the user's libraries, for the app's navigation.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Library {
    pub item: ItemRef,
    pub name: String,
    /// The collection kind (`movies`, `tv_shows`..., or one the server named); none for a mixed library.
    pub collection: Option<String>,
    pub artwork: Vec<Artwork>,
}

/// A person in an item's cast or crew.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Person {
    /// The person as an item (C# forbids a member named as its record, so not `person`).
    pub item: ItemRef,
    pub name: String,
    /// The part played, for an actor.
    pub role: Option<String>,
    /// `actor`, `director`, `writer`..., or one the server named.
    pub kind: String,
    pub image: Option<Artwork>,
}

/// A date without a time or zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Chapter {
    /// Where it starts, in milliseconds.
    pub start_ms: u64,
    pub name: Option<String>,
}

/// One stream of the default version, as a detail page lists it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StreamSummary {
    pub label: String,
    pub language: Option<String>,
    pub codec: Option<String>,
}

/// The default version of an item's media.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MediaSummary {
    /// How many versions the item has.
    pub source_count: u32,
    pub container: Option<String>,
    pub video: Option<StreamSummary>,
    pub audio: Vec<StreamSummary>,
    pub subtitles: Vec<StreamSummary>,
}

/// What a detail page shows.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ItemDetails {
    pub summary: ItemSummary,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub taglines: Vec<String>,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub people: Vec<Person>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub premiere: Option<CalendarDate>,
    pub end: Option<CalendarDate>,
    pub status: Option<String>,
    pub backdrops: Vec<Artwork>,
    pub logo: Option<Artwork>,
    pub chapters: Vec<Chapter>,
    pub media: Option<MediaSummary>,
    /// A series' run, from its dates and status; `None` for anything else.
    #[uniffi(default = None)]
    pub run: Option<SeriesRun>,
}

/// A series' run: the year it started, the year it ended if that is another year, and whether it is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SeriesRun {
    pub start_year: i32,
    pub end_year: Option<i32>,
    pub continuing: bool,
}

/// One way to sort a library grid: its key (`sort_name`, `date_created`...) and the order it goes in.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SortChoice {
    pub key: String,
    pub order: SortOrder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Enum)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// What a library grid shows: its library is the parent. Empty lists mean no restriction; no sort key means the server's order.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ItemQuery {
    /// Media kinds to include (`movie`, `series`...).
    pub kinds: Vec<String>,
    /// Include items in folders below the library, not only its top level.
    pub recursive: bool,
    /// Sort keys (`sort_name`, `date_created`..., or one the server names), most significant first.
    pub sort: Vec<String>,
    pub order: SortOrder,
    /// `played`, `unplayed`, `favourite`, `resumable`, or one the server names.
    pub filters: Vec<String>,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    pub name_starts_with: Option<String>,
    /// Only items the server finds for this, as its search does.
    #[uniffi(default = None)]
    pub search_term: Option<String>,
}

/// One place in a grid. Never null across the bridge: WinUI 3 does not take null placeholders.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
// UniFFI cannot carry a boxed record, and a slot lives here only while its batch is lowered for the bindings.
#[allow(clippy::large_enum_variant)]
pub enum Slot {
    Loading,
    Item {
        summary: ItemSummary,
    },
    /// Its page could not be loaded: the tile shows a retry glyph.
    Failed,
}

/// What the message above a grid shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct PagedState {
    /// How many items the query holds; 0 until the first page answered.
    pub total: u32,
    /// Whether requests are in flight.
    pub loading: bool,
    /// Why the last page or refresh failed: one message with Retry.
    pub failed: Option<BrowseProblem>,
}

/// Whether a detail page or a search is loading, and why it last failed.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct LoadState {
    pub loading: bool,
    pub failed: Option<BrowseProblem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HomeRowKind {
    ContinueWatching,
    NextUp,
    Latest,
}

/// One row of Home. Debug leaves out the title, a library's name.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct HomeRow {
    /// Stable across loads: the key for [`HomeFeed::subscribe_row`](super::HomeFeed::subscribe_row).
    pub key: String,
    pub kind: HomeRowKind,
    /// The library's name for a Latest row; empty for the others, which the UI names.
    pub title: String,
    /// The library of a Latest row.
    pub library: Option<ItemRef>,
}

impl std::fmt::Debug for HomeRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeRow")
            .field("key", &self.key)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// What Home's message shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct HomeState {
    pub loading: bool,
    /// Why a row failed in the last load: one message with Retry.
    pub failed: Option<BrowseProblem>,
    /// The server could not be reached: the rows are the last ones loaded.
    pub offline: bool,
}

/// One group of search results: the kind its items share.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SearchGroup {
    pub kind: String,
}

/// A change to an item's marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ItemAction {
    SetPlayed { played: bool },
    SetFavourite { favourite: bool },
}

/// Something the UI tells the user once, as a message. Debug leaves out the item's name.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BrowseNotice {
    /// Counts up with each notice, so two identical failures can be told apart.
    pub seq: u64,
    pub kind: NoticeKind,
}

/// Debug leaves out the item's name.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NoticeKind {
    /// The server did not take a played or favourite change: it was rolled back. `item_name` is for the message only; it never
    /// goes to a log.
    ActionFailed {
        action: ItemAction,
        item_name: Option<String>,
        problem: BrowseProblem,
    },
}

impl std::fmt::Debug for BrowseNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowseNotice")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::fmt::Debug for NoticeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActionFailed {
                action, problem, ..
            } => f
                .debug_struct("ActionFailed")
                .field("action", action)
                .field("problem", problem)
                .finish_non_exhaustive(),
        }
    }
}

/// An image the UI asks for, at the size of the box it fills.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ImageRequest {
    pub source: ImageSource,
    /// The box's width in physical pixels: its width in view pixels times the display's scale (`XamlRoot.RasterizationScale` on
    /// Windows). The core rounds it up to a size bucket. 0 is refused ([`ImageProblem::EmptyBox`](super::ImageProblem::EmptyBox)).
    pub width: u32,
    /// The box's height in physical pixels, as `width`. 0 is refused.
    pub height: u32,
}

/// Where an image comes from.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ImageSource {
    /// A server's image of an item, as the item's artwork names it: JPEG, or PNG when it needs transparency.
    Server { image: ImageRef },
    /// A BlurHash placeholder: a BMP at most 32 pixels on its long side, at the box's ratio.
    BlurHash { hash: String },
}

list_diff!(
    /// A change to a grid's slots.
    SlotDiff,
    Slot
);

list_diff!(
    /// A change to a short list of items: a Home row, seasons, episodes, similar items, a search group.
    ItemSummaryDiff,
    ItemSummary
);

list_diff!(
    /// A change to Home's rows.
    HomeRowDiff,
    HomeRow
);

list_diff!(
    /// A change to a search's groups.
    SearchGroupDiff,
    SearchGroup
);

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl From<&model::ItemRef> for ItemRef {
    fn from(item: &model::ItemRef) -> Self {
        Self {
            server: (&item.server).into(),
            id: item.id.as_str().to_owned(),
        }
    }
}

impl From<&ItemRef> for model::ItemRef {
    fn from(item: &ItemRef) -> Self {
        Self::new((&item.server).into(), model::ItemId::new(item.id.clone()))
    }
}

impl From<&model::ImageRef> for ImageRef {
    fn from(image: &model::ImageRef) -> Self {
        Self {
            item: (&image.item).into(),
            kind: image.kind.as_str().to_owned(),
            index: image.index,
            tag: image.tag.clone(),
        }
    }
}

impl From<&ImageRef> for model::ImageRef {
    fn from(image: &ImageRef) -> Self {
        Self {
            item: (&image.item).into(),
            kind: model::ImageType::new(image.kind.clone()),
            index: image.index,
            tag: image.tag.clone(),
        }
    }
}

impl From<&model::Artwork> for Artwork {
    fn from(artwork: &model::Artwork) -> Self {
        Self {
            kind: artwork.kind.as_str().to_owned(),
            image: (&artwork.image).into(),
            aspect_ratio: artwork.aspect_ratio.map(|ratio| AspectRatio {
                width: ratio.width(),
                height: ratio.height(),
            }),
            blurhash: artwork.blurhash.clone(),
        }
    }
}

impl From<&Artwork> for model::Artwork {
    fn from(artwork: &Artwork) -> Self {
        Self {
            kind: model::ImageType::new(artwork.kind.clone()),
            image: (&artwork.image).into(),
            aspect_ratio: artwork
                .aspect_ratio
                .and_then(|ratio| model::AspectRatio::new(ratio.width, ratio.height)),
            blurhash: artwork.blurhash.clone(),
        }
    }
}

impl From<&model::UserState> for UserState {
    fn from(state: &model::UserState) -> Self {
        Self {
            played: state.played,
            position_ms: state.position.map(millis),
            progress: state.progress,
            favourite: state.favourite,
            unplayed_count: state.unplayed_count,
            play_count: state.play_count,
        }
    }
}

impl From<&model::EpisodeLinks> for EpisodeLinks {
    fn from(links: &model::EpisodeLinks) -> Self {
        Self {
            series: links.series.as_ref().map(Into::into),
            series_name: links.series_name.clone(),
            season: links.season.as_ref().map(Into::into),
            season_name: links.season_name.clone(),
            number: links.number,
            number_end: links.number_end,
            season_number: links.season_number,
            label: links.number_label(),
        }
    }
}

impl From<&model::ItemSummary> for ItemSummary {
    fn from(summary: &model::ItemSummary) -> Self {
        Self {
            item: (&summary.item).into(),
            kind: summary.kind.as_str().to_owned(),
            name: summary.name.clone(),
            year: summary.year,
            runtime_ms: summary.runtime.map(millis),
            artwork: summary.artwork.iter().map(Into::into).collect(),
            user: (&summary.user).into(),
            episode: summary.episode.as_ref().map(Into::into),
            child_count: summary.child_count,
            poster: current_app::tile_artwork(summary, current_app::TileShape::Poster)
                .map(Into::into),
            wide: current_app::tile_artwork(summary, current_app::TileShape::Wide).map(Into::into),
            time_left_ms: summary.time_left().map(millis),
        }
    }
}

impl From<model::ItemSummary> for ItemSummary {
    fn from(summary: model::ItemSummary) -> Self {
        (&summary).into()
    }
}

impl From<&UserState> for model::UserState {
    fn from(state: &UserState) -> Self {
        Self {
            played: state.played,
            position: state.position_ms.map(Duration::from_millis),
            progress: state.progress,
            favourite: state.favourite,
            unplayed_count: state.unplayed_count,
            play_count: state.play_count,
        }
    }
}

impl From<&EpisodeLinks> for model::EpisodeLinks {
    fn from(links: &EpisodeLinks) -> Self {
        Self {
            series: links.series.as_ref().map(Into::into),
            series_name: links.series_name.clone(),
            season: links.season.as_ref().map(Into::into),
            season_name: links.season_name.clone(),
            number: links.number,
            number_end: links.number_end,
            season_number: links.season_number,
        }
    }
}

/// Back to the model, for the rules the UI asks about a tile's item ([`crate::browse::play_target`]). `poster` and `wide` are picked
/// from `artwork` again when needed.
impl From<&ItemSummary> for model::ItemSummary {
    fn from(summary: &ItemSummary) -> Self {
        Self {
            item: (&summary.item).into(),
            kind: model::MediaKind::new(summary.kind.as_str()),
            name: summary.name.clone(),
            year: summary.year,
            runtime: summary.runtime_ms.map(Duration::from_millis),
            artwork: summary.artwork.iter().map(Into::into).collect(),
            user: (&summary.user).into(),
            episode: summary.episode.as_ref().map(Into::into),
            child_count: summary.child_count,
        }
    }
}

/// What a Play button starts (ADR 0019, task 5): `item` (a series' next episode on its page) from `start_ms`. When `resumes`, the
/// button says "Resume" and "Play from the beginning" goes with it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PlayTarget {
    pub item: ItemSummary,
    pub start_ms: u64,
    pub resumes: bool,
}

impl From<&model::PlayTarget> for PlayTarget {
    fn from(target: &model::PlayTarget) -> Self {
        Self {
            item: (&target.item).into(),
            start_ms: millis(target.start),
            resumes: target.resumes(),
        }
    }
}

impl From<&model::Library> for Library {
    fn from(library: &model::Library) -> Self {
        Self {
            item: (&library.item).into(),
            name: library.name.clone(),
            collection: library
                .collection
                .as_ref()
                .map(|kind| kind.as_str().to_owned()),
            artwork: library.artwork.iter().map(Into::into).collect(),
        }
    }
}

impl From<&Library> for model::Library {
    fn from(library: &Library) -> Self {
        Self {
            item: (&library.item).into(),
            name: library.name.clone(),
            collection: library
                .collection
                .as_ref()
                .map(|kind| model::CollectionKind::new(kind.clone())),
            artwork: library.artwork.iter().map(Into::into).collect(),
        }
    }
}

impl From<&model::Person> for Person {
    fn from(person: &model::Person) -> Self {
        Self {
            item: (&person.person).into(),
            name: person.name.clone(),
            role: person.role.clone(),
            kind: person.kind.as_str().to_owned(),
            image: person.image.as_ref().map(Into::into),
        }
    }
}

impl From<model::CalendarDate> for CalendarDate {
    fn from(date: model::CalendarDate) -> Self {
        Self {
            year: date.year,
            month: date.month,
            day: date.day,
        }
    }
}

impl From<&model::StreamSummary> for StreamSummary {
    fn from(stream: &model::StreamSummary) -> Self {
        Self {
            label: stream.label.clone(),
            language: stream.language.clone(),
            codec: stream.codec.clone(),
        }
    }
}

impl From<&model::MediaSummary> for MediaSummary {
    fn from(media: &model::MediaSummary) -> Self {
        Self {
            source_count: media.source_count,
            container: media.container.clone(),
            video: media.video.as_ref().map(Into::into),
            audio: media.audio.iter().map(Into::into).collect(),
            subtitles: media.subtitles.iter().map(Into::into).collect(),
        }
    }
}

impl From<&model::ItemDetails> for ItemDetails {
    fn from(details: &model::ItemDetails) -> Self {
        Self {
            summary: (&details.summary).into(),
            original_title: details.original_title.clone(),
            overview: details.overview.clone(),
            taglines: details.taglines.clone(),
            genres: details.genres.clone(),
            studios: details.studios.clone(),
            people: details.people.iter().map(Into::into).collect(),
            official_rating: details.official_rating.clone(),
            community_rating: details.community_rating,
            critic_rating: details.critic_rating,
            premiere: details.premiere.map(Into::into),
            end: details.end.map(Into::into),
            status: details.status.clone(),
            backdrops: details.backdrops.iter().map(Into::into).collect(),
            logo: details.logo.as_ref().map(Into::into),
            chapters: details
                .chapters
                .iter()
                .map(|chapter| Chapter {
                    start_ms: millis(chapter.start),
                    name: chapter.name.clone(),
                })
                .collect(),
            media: details.media.as_ref().map(Into::into),
            run: current_app::series_run(details).map(|run| SeriesRun {
                start_year: run.start_year,
                end_year: run.end_year,
                continuing: run.continuing,
            }),
        }
    }
}

impl From<&model::ItemQuery> for ItemQuery {
    /// The parent stays behind: across the bridge a query's library is passed beside it.
    fn from(query: &model::ItemQuery) -> Self {
        Self {
            kinds: query
                .kinds
                .iter()
                .map(|kind| kind.as_str().to_owned())
                .collect(),
            recursive: query.recursive,
            sort: query
                .sort
                .iter()
                .map(|key| key.as_str().to_owned())
                .collect(),
            order: match query.order {
                model::SortOrder::Ascending => SortOrder::Ascending,
                model::SortOrder::Descending => SortOrder::Descending,
            },
            filters: query
                .filters
                .iter()
                .map(|filter| filter.as_str().to_owned())
                .collect(),
            genres: query.genres.clone(),
            years: query.years.clone(),
            name_starts_with: query.name_starts_with.clone(),
            search_term: query.search_term.clone(),
        }
    }
}

impl From<&ItemQuery> for model::ItemQuery {
    fn from(query: &ItemQuery) -> Self {
        Self {
            parent: None,
            kinds: query
                .kinds
                .iter()
                .map(|kind| model::MediaKind::new(kind.clone()))
                .collect(),
            recursive: query.recursive,
            sort: query
                .sort
                .iter()
                .map(|key| model::SortKey::new(key.clone()))
                .collect(),
            order: match query.order {
                SortOrder::Ascending => model::SortOrder::Ascending,
                SortOrder::Descending => model::SortOrder::Descending,
            },
            filters: query
                .filters
                .iter()
                .map(|filter| model::ItemFilter::new(filter.clone()))
                .collect(),
            genres: query.genres.clone(),
            years: query.years.clone(),
            name_starts_with: query.name_starts_with.clone(),
            search_term: query.search_term.clone(),
        }
    }
}

impl From<current_app::Slot> for Slot {
    fn from(slot: current_app::Slot) -> Self {
        match slot {
            current_app::Slot::Loading => Self::Loading,
            current_app::Slot::Item(summary) => Self::Item {
                summary: (&*summary).into(),
            },
            current_app::Slot::Failed => Self::Failed,
        }
    }
}

impl From<&current_app::PagedState> for PagedState {
    fn from(state: &current_app::PagedState) -> Self {
        Self {
            total: state.total,
            loading: state.loading,
            failed: state.failed.as_ref().map(Into::into),
        }
    }
}

impl From<&current_app::LoadState> for LoadState {
    fn from(state: &current_app::LoadState) -> Self {
        Self {
            loading: state.loading,
            failed: state.failed.as_ref().map(Into::into),
        }
    }
}

impl From<&current_app::HomeState> for HomeState {
    fn from(state: &current_app::HomeState) -> Self {
        Self {
            loading: state.loading,
            failed: state.failed.as_ref().map(Into::into),
            offline: state.offline,
        }
    }
}

impl From<current_app::HomeRow> for HomeRow {
    fn from(row: current_app::HomeRow) -> Self {
        Self {
            key: row.key,
            kind: match row.kind {
                current_app::HomeRowKind::ContinueWatching => HomeRowKind::ContinueWatching,
                current_app::HomeRowKind::NextUp => HomeRowKind::NextUp,
                current_app::HomeRowKind::Latest => HomeRowKind::Latest,
            },
            title: row.title,
            library: row.library.as_ref().map(Into::into),
        }
    }
}

impl From<model::MediaKind> for SearchGroup {
    fn from(kind: model::MediaKind) -> Self {
        Self {
            kind: kind.as_str().to_owned(),
        }
    }
}

impl From<current_app::ItemAction> for ItemAction {
    fn from(action: current_app::ItemAction) -> Self {
        match action {
            current_app::ItemAction::SetPlayed(played) => Self::SetPlayed { played },
            current_app::ItemAction::SetFavourite(favourite) => Self::SetFavourite { favourite },
        }
    }
}

impl From<&current_app::BrowseNotice> for BrowseNotice {
    fn from(notice: &current_app::BrowseNotice) -> Self {
        Self {
            seq: notice.seq,
            kind: match &notice.kind {
                current_app::NoticeKind::ActionFailed {
                    action,
                    item_name,
                    problem,
                } => NoticeKind::ActionFailed {
                    action: (*action).into(),
                    item_name: item_name.clone(),
                    problem: problem.into(),
                },
            },
        }
    }
}

impl From<&ImageRequest> for current_app::ImageLoad {
    fn from(request: &ImageRequest) -> Self {
        Self {
            source: match &request.source {
                ImageSource::Server { image } => current_app::ImageSource::Server(image.into()),
                ImageSource::BlurHash { hash } => current_app::ImageSource::BlurHash(hash.clone()),
            },
            width: request.width,
            height: request.height,
        }
    }
}
