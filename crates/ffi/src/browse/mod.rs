//! Browsing across the bridge (ADR 0018, section 7): [`Core::browse`] gives a [`Browse`] for a signed-in Jellyfin server, with library
//! grids ([`PagedItems`]), Home ([`HomeFeed`]), detail pages ([`DetailsView`]) and search ([`SearchResults`]); played and favourite
//! changes go through it, and their notices through [`Core::subscribe_browse_notices`]. [`Core::load_image`] hands the UI an
//! image's encoded bytes; [`Core::image_cache_size`] and [`Core::clear_image_cache`] serve Settings → Storage. Lists and values use the shapes of `subscribe` (ADR 0004).
//!
//! Each object holds its `current-app` counterpart: dropping it (every foreign reference, and every subscription it started)
//! stops its requests.

mod problems;
mod records;

use std::sync::Arc;

use tokio::runtime::Handle;

use crate::subscribe::{self, LiveListeners};
use crate::{CancelToken, Core, CoreError, ServerRef, TaskHandle, cancel};

pub use problems::{BrowseProblem, ImageProblem};
pub use records::{
    Artwork, AspectRatio, BrowseNotice, CalendarDate, Chapter, EpisodeLinks, HomeRow, HomeRowDiff,
    HomeRowKind, HomeState, ImageRef, ImageRequest, ImageSource, ItemAction, ItemDetails,
    ItemQuery, ItemRef, ItemSummary, ItemSummaryDiff, Library, LoadState, MediaSummary, NoticeKind,
    PagedState, Person, PlayTarget, SearchGroup, SearchGroupDiff, SeriesRun, Slot, SlotDiff,
    SortChoice, SortOrder, StreamSummary, UserState,
};

/// Receives a grid's slots: `[Reset]` first, then batches of diffs, on a core thread. Apply each batch in order. It must only hand
/// the batch to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait SlotsListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<SlotDiff>);
}

/// Receives a short list of items, as [`SlotsListener`] does.
#[uniffi::export(callback_interface)]
pub trait ItemsListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<ItemSummaryDiff>);
}

/// Receives Home's rows, as [`SlotsListener`] does.
#[uniffi::export(callback_interface)]
pub trait HomeRowsListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<HomeRowDiff>);
}

/// Receives a search's groups, as [`SlotsListener`] does.
#[uniffi::export(callback_interface)]
pub trait SearchGroupsListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<SearchGroupDiff>);
}

/// Receives a grid's state: the current value first, then every change, on a core thread. It must only hand the value to the UI
/// thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait PagedStateListener: Send + Sync {
    fn on_value(&self, value: PagedState);
}

/// Receives Home's state, as [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait HomeStateListener: Send + Sync {
    fn on_value(&self, value: HomeState);
}

/// Receives a detail page's or a search's state, as [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait LoadStateListener: Send + Sync {
    fn on_value(&self, value: LoadState);
}

/// Receives a detail page's details (`None` until loaded), as [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait ItemDetailsListener: Send + Sync {
    fn on_value(&self, value: Option<ItemDetails>);
}

/// Receives what a detail page's Play button starts (`None` until loaded, and for an item that does not play), as
/// [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait PlayTargetListener: Send + Sync {
    fn on_value(&self, value: Option<PlayTarget>);
}

/// Receives the user's libraries (`None` until first loaded), as [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait LibrariesListener: Send + Sync {
    fn on_value(&self, value: Option<Vec<Library>>);
}

/// Receives the latest browse notice (`None` until the first), as [`PagedStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait BrowseNoticeListener: Send + Sync {
    fn on_value(&self, value: Option<BrowseNotice>);
}

/// What every browse object needs to start subscriptions.
#[derive(Clone)]
struct Host {
    runtime: Handle,
    listeners: LiveListeners,
}

impl Host {
    fn items(
        &self,
        list: &Arc<current_app::ObservableList<current_model::ItemSummary>>,
        listener: Box<dyn ItemsListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_list(&self.runtime, list.subscribe(), move |batch| {
            listener
                .listener
                .on_diffs(batch.into_iter().map(ItemSummaryDiff::from_core).collect());
        })
    }

    fn load_state(
        &self,
        rx: tokio::sync::watch::Receiver<current_app::LoadState>,
        listener: Box<dyn LoadStateListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, rx, move |state| {
            listener.listener.on_value((&state).into());
        })
    }
}

#[uniffi::export]
impl Core {
    /// Browsing `server` (ADR 0018). The `Browse` belongs to the server's current session: after a sign-out, a removal or a new
    /// sign-in its lists stop loading and its requests fail with [`BrowseProblem::NotSignedIn`]; ask for a new one then.
    ///
    /// # Errors
    /// [`CoreError::UnknownServer`] if no such server is saved; [`CoreError::NotSignedIn`] unless it is a Jellyfin server that is
    /// signed in, with a live session.
    pub fn browse(&self, server: ServerRef) -> Result<Arc<Browse>, CoreError> {
        let server: current_model::ServerRef = (&server).into();
        if !self
            .app
            .servers()
            .iter()
            .any(|saved| saved.server == server)
        {
            return Err(CoreError::UnknownServer);
        }
        // `App::browse` reads the runtime it starts the session's work on from the calling context.
        let _runtime = self.runtime.enter();
        let browse = self.app.browse(&server).ok_or(CoreError::NotSignedIn)?;
        Ok(Arc::new(Browse {
            browse,
            host: Host {
                runtime: self.runtime.clone(),
                listeners: self.listeners.clone(),
            },
        }))
    }

    /// Subscribes to the latest browse notice: a played or favourite change a server did not take, which was rolled back. Show
    /// each new `seq` once, as a message. Keep the handle while listening.
    pub fn subscribe_browse_notices(
        &self,
        listener: Box<dyn BrowseNoticeListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.app.browse_notices(), move |notice| {
            listener.listener.on_value(notice.as_ref().map(Into::into));
        })
    }

    /// An image's encoded bytes (JPEG or PNG from a server, BMP for a BlurHash), from memory, the disk cache or the server, through
    /// the server's session and its gateway credentials. Identical requests share one fetch; cancelling `token` stops waiting, and
    /// the fetch stops once no request waits for it.
    ///
    /// # Errors
    /// [`CoreError::Image`]: not signed in, the server's or the network's failure, an answer that is not an image, an invalid
    /// BlurHash or an empty box; [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn load_image(
        &self,
        request: ImageRequest,
        token: Arc<CancelToken>,
    ) -> Result<Vec<u8>, CoreError> {
        let app = self.app.clone();
        let load: current_app::ImageLoad = (&request).into();
        cancel::run_cancellable(&self.runtime, token, async move {
            app.load_image(&load)
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| CoreError::Image {
                    problem: (&error).into(),
                })
        })
        .await
    }

    /// The image cache's size on disk, in bytes, every server's images together: what Settings → Storage shows. 0 for a core that
    /// stores nothing.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn image_cache_size(&self, token: Arc<CancelToken>) -> Result<u64, CoreError> {
        let app = self.app.clone();
        cancel::run_cancellable(&self.runtime, token, async move {
            Ok(app.image_cache_size().await)
        })
        .await
    }

    /// Deletes every cached image of every server, in memory and on disk (Settings → Storage, "Clear"); they load again from the
    /// servers when next shown. Safe while images load. Not cancellable: it runs to its end once started.
    ///
    /// # Errors
    /// [`CoreError::Runtime`] if the core's runtime could not run it.
    pub async fn clear_image_cache(&self) -> Result<(), CoreError> {
        let app = self.app.clone();
        self.runtime
            .spawn(async move { app.clear_image_cache().await })
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })
    }
}

/// The sorts a library grid offers, in the order to list them, each with its order: names A to Z first (the grid's first sort),
/// then the newest added, the newest released and the best rated first. The UI names each key.
#[uniffi::export]
pub fn library_sort_choices() -> Vec<SortChoice> {
    current_app::library_sorts()
        .into_iter()
        .map(|choice| SortChoice {
            key: choice.key.as_str().to_owned(),
            order: match choice.order {
                current_model::SortOrder::Ascending => SortOrder::Ascending,
                current_model::SortOrder::Descending => SortOrder::Descending,
            },
        })
        .collect()
}

/// The filters a library grid offers, in the order to list them: everything first (`None`), then `unplayed`, `played` and
/// `favourite`. The UI names each.
#[uniffi::export]
pub fn library_filter_choices() -> Vec<Option<String>> {
    current_app::library_filters()
        .into_iter()
        .map(|filter| filter.map(|filter| filter.as_str().to_owned()))
        .collect()
}

/// What `library`'s grid shows before the user sorts or filters it, by name: a film library its films and a TV library its series,
/// wherever they sit below it; any other library its top level.
#[uniffi::export]
pub fn library_query(library: Library) -> ItemQuery {
    (&current_app::library_query(&(&library).into())).into()
}

/// What a Home row of `library`'s latest items opens when its header is chosen: the library's grid, the newest added first.
#[uniffi::export]
pub fn recently_added_query(library: Library) -> ItemQuery {
    (&current_app::recently_added_query(&(&library).into())).into()
}

/// What Play on a tile's item starts: the item from its resume point, unless played; `None` for a kind that does not play by itself
/// (a series plays its next episode from its page, which asks the server). Pass the item with the user's marks as every list shows them.
#[uniffi::export]
pub fn play_target(item: ItemSummary) -> Option<PlayTarget> {
    current_model::PlayTarget::for_item(&(&item).into())
        .as_ref()
        .map(Into::into)
}

/// Browsing one session of a signed-in Jellyfin server.
#[derive(uniffi::Object)]
pub struct Browse {
    browse: current_app::Browse,
    host: Host,
}

#[uniffi::export]
impl Browse {
    pub fn server(&self) -> ServerRef {
        self.browse.server().into()
    }

    /// The user's libraries, in the user's order, as a load that starts now reads them ([`Browse::subscribe_libraries`] hears
    /// them too).
    ///
    /// # Errors
    /// [`CoreError::Browse`]: the session ended, or the server's or the network's failure; [`CoreError::Cancelled`] if `token` is
    /// cancelled first.
    pub async fn libraries(&self, token: Arc<CancelToken>) -> Result<Vec<Library>, CoreError> {
        let browse = self.browse.clone();
        cancel::run_cancellable(&self.host.runtime, token, async move {
            browse
                .libraries()
                .await
                .map(|libraries| libraries.iter().map(Into::into).collect())
                .map_err(|error| CoreError::Browse {
                    problem: (&error).into(),
                })
        })
        .await
    }

    /// Subscribes to the user's libraries for the app's navigation, in the user's order: `None` until first loaded, then the last
    /// list loaded. The core reads them again on a refresh, on return to the foreground, when Home reloads, after a library request
    /// the server refused, and when the server is reached again after a failed load; a library the user can no longer open leaves
    /// the list. Keep the handle while listening.
    pub fn subscribe_libraries(&self, listener: Box<dyn LibrariesListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(
            &self.host.runtime,
            self.browse.library_list(),
            move |libraries| {
                listener.listener.on_value(
                    libraries.map(|libraries| libraries.iter().map(Into::into).collect()),
                );
            },
        )
    }

    /// A grid of `library`'s items, sorted and filtered by `query`. Starts loading at once. When a request fails as it does for a
    /// library that is gone, the core reads the libraries again; if the library left them, the grid's state says
    /// [`BrowseProblem::NotAvailable`].
    pub fn open_library(&self, library: Library, query: ItemQuery) -> Arc<PagedItems> {
        let items = self
            .browse
            .open_library(&(&library).into(), (&query).into());
        Arc::new(PagedItems {
            items,
            host: self.host.clone(),
        })
    }

    /// Home's rows. Starts loading at once.
    pub fn home(&self) -> Arc<HomeFeed> {
        Arc::new(HomeFeed {
            feed: self.browse.home(),
            host: self.host.clone(),
        })
    }

    /// `item`'s detail page. Starts loading at once.
    pub fn details(&self, item: ItemRef) -> Arc<DetailsView> {
        Arc::new(DetailsView {
            view: self.browse.details(&(&item).into()),
            host: self.host.clone(),
        })
    }

    /// The best matches for `term`, one group per kind. Starts loading at once.
    pub fn search(&self, term: String) -> Arc<SearchResults> {
        Arc::new(SearchResults {
            results: self.browse.search(&term),
            host: self.host.clone(),
        })
    }

    /// Marks `item` played or unplayed: every live list shows it at once, the server is told, and on failure the change rolls
    /// back with a notice ([`Core::subscribe_browse_notices`]).
    pub fn set_played(&self, item: ItemRef, played: bool) {
        self.browse.set_played(&(&item).into(), played);
    }

    /// Marks `item` a favourite or not, as [`Browse::set_played`] does.
    pub fn set_favourite(&self, item: ItemRef, favourite: bool) {
        self.browse.set_favourite(&(&item).into(), favourite);
    }

    /// `item`'s marks as the lists show them, if a list loaded it.
    pub fn user_state(&self, item: ItemRef) -> Option<UserState> {
        self.browse
            .user_state(&(&item).into())
            .as_ref()
            .map(Into::into)
    }

    /// Reads every live list of this session again, without resetting them, as the user asked.
    pub fn refresh(&self) {
        self.browse.refresh();
    }
}

/// A library grid: a list of slots sized to the query's total, filled a page at a time around what the UI shows (ADR 0018,
/// section 3).
#[derive(uniffi::Object)]
pub struct PagedItems {
    items: current_app::PagedItems,
    host: Host,
}

#[uniffi::export]
impl PagedItems {
    /// Subscribes to the slots. A refresh sends only `Set`, `Append` and `Remove`, so the UI keeps its scroll position; only a new
    /// query resets.
    pub fn subscribe_slots(&self, listener: Box<dyn SlotsListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_list(
            &self.host.runtime,
            self.items.slots().subscribe(),
            move |batch| {
                listener
                    .listener
                    .on_diffs(batch.into_iter().map(SlotDiff::from_core).collect());
            },
        )
    }

    pub fn state(&self) -> PagedState {
        (&*self.items.state().borrow()).into()
    }

    pub fn subscribe_state(&self, listener: Box<dyn PagedStateListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(&self.host.runtime, self.items.state(), move |state| {
            listener.listener.on_value((&state).into());
        })
    }

    /// The UI shows slots `first` to `last` (inclusive, either order): the pages around them load, the others' requests stop. A
    /// failed page that comes into view is retried once by itself.
    pub fn set_visible_range(&self, first: u32, last: u32) {
        let index = |value: u32| usize::try_from(value).unwrap_or(usize::MAX);
        self.items.set_visible_range(index(first), index(last));
    }

    /// Reads the list again without resetting it.
    pub fn refresh(&self) {
        self.items.refresh();
    }

    /// The page showing the grid was left.
    pub fn hidden(&self) {
        self.items.hidden();
    }

    /// The page showing the grid is shown again: it refreshes, without a reset, if it was left five minutes or more ago. Returns
    /// whether it refreshed.
    pub fn shown(&self) -> bool {
        self.items.shown()
    }

    /// Reloads every failed page (the Retry button).
    pub fn retry(&self) {
        self.items.retry();
    }

    /// A new sort or filter: a new query, which resets the list and starts at the top.
    pub fn set_query(&self, query: ItemQuery) {
        self.items.set_query((&query).into());
    }
}

/// Home: rows, each with its own list (ADR 0018, section 6). Its layout is provisional.
#[derive(uniffi::Object)]
pub struct HomeFeed {
    feed: current_app::HomeFeed,
    host: Host,
}

#[uniffi::export]
impl HomeFeed {
    /// Subscribes to the rows, in order. A row exists only while it has items.
    pub fn subscribe_rows(&self, listener: Box<dyn HomeRowsListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_list(
            &self.host.runtime,
            self.feed.rows().subscribe(),
            move |batch| {
                listener
                    .listener
                    .on_diffs(batch.into_iter().map(HomeRowDiff::from_core).collect());
            },
        )
    }

    /// Subscribes to the items of the row with `key`. `None` if no such row exists now; a row's list exists before the row
    /// appears in [`HomeFeed::subscribe_rows`].
    pub fn subscribe_row(
        &self,
        key: String,
        listener: Box<dyn ItemsListener>,
    ) -> Option<Arc<TaskHandle>> {
        let list = self.feed.row(&key)?;
        Some(self.host.items(&list, listener))
    }

    pub fn state(&self) -> HomeState {
        (&*self.feed.state().borrow()).into()
    }

    pub fn subscribe_state(&self, listener: Box<dyn HomeStateListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(&self.host.runtime, self.feed.state(), move |state| {
            listener.listener.on_value((&state).into());
        })
    }

    /// Loads every row again, keeping what they show until the answers arrive.
    pub fn refresh(&self) {
        self.feed.refresh();
    }

    /// Home was left.
    pub fn hidden(&self) {
        self.feed.hidden();
    }

    /// Home is shown again: its rows load again, keeping what they show, if it was left five minutes or more ago. Returns whether
    /// it refreshed.
    pub fn shown(&self) -> bool {
        self.feed.shown()
    }

    /// The Retry button.
    pub fn retry(&self) {
        self.feed.retry();
    }
}

/// An item's detail page: its details, and a series' seasons, a season's episodes and similar items as short lists.
#[derive(uniffi::Object)]
pub struct DetailsView {
    view: current_app::DetailsView,
    host: Host,
}

#[uniffi::export]
impl DetailsView {
    pub fn item(&self) -> ItemRef {
        self.view.item().into()
    }

    pub fn subscribe_details(&self, listener: Box<dyn ItemDetailsListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(&self.host.runtime, self.view.details(), move |details| {
            listener.listener.on_value(details.as_ref().map(Into::into));
        })
    }

    pub fn subscribe_state(&self, listener: Box<dyn LoadStateListener>) -> Arc<TaskHandle> {
        self.host.load_state(self.view.state(), listener)
    }

    /// What the Play button starts: the item from its resume point, or a series' next episode (else its first).
    pub fn subscribe_play(&self, listener: Box<dyn PlayTargetListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(&self.host.runtime, self.view.play(), move |play| {
            listener.listener.on_value(play.as_ref().map(Into::into));
        })
    }

    /// A series' seasons; empty for other kinds.
    pub fn subscribe_seasons(&self, listener: Box<dyn ItemsListener>) -> Arc<TaskHandle> {
        self.host.items(&self.view.seasons(), listener)
    }

    /// The picked season's episodes ([`DetailsView::select_season`]), the first season's until then; empty unless a series.
    pub fn subscribe_episodes(&self, listener: Box<dyn ItemsListener>) -> Arc<TaskHandle> {
        self.host.items(&self.view.episodes(), listener)
    }

    pub fn subscribe_similar(&self, listener: Box<dyn ItemsListener>) -> Arc<TaskHandle> {
        self.host.items(&self.view.similar(), listener)
    }

    /// Shows `season`'s episodes.
    pub fn select_season(&self, season: ItemRef) {
        self.view.select_season(&(&season).into());
    }

    /// Loads the page again, keeping what it shows until the answers arrive.
    pub fn refresh(&self) {
        self.view.refresh();
    }
}

/// The results of one search: one group per kind.
#[derive(uniffi::Object)]
pub struct SearchResults {
    results: current_app::SearchResults,
    host: Host,
}

#[uniffi::export]
impl SearchResults {
    /// Subscribes to the kinds with matches, in order.
    pub fn subscribe_groups(&self, listener: Box<dyn SearchGroupsListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_list(
            &self.host.runtime,
            self.results.groups().subscribe(),
            move |batch| {
                listener
                    .listener
                    .on_diffs(batch.into_iter().map(SearchGroupDiff::from_core).collect());
            },
        )
    }

    /// Subscribes to the matches of `kind`. `None` if it has none now; a group's list exists before the kind appears in
    /// [`SearchResults::subscribe_groups`].
    pub fn subscribe_group(
        &self,
        kind: String,
        listener: Box<dyn ItemsListener>,
    ) -> Option<Arc<TaskHandle>> {
        let list = self.results.group(&current_model::MediaKind::new(kind))?;
        Some(self.host.items(&list, listener))
    }

    pub fn subscribe_state(&self, listener: Box<dyn LoadStateListener>) -> Arc<TaskHandle> {
        self.host.load_state(self.results.state(), listener)
    }

    pub fn refresh(&self) {
        self.results.refresh();
    }
}

#[cfg(test)]
mod tests;
