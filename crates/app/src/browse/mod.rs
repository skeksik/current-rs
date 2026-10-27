//! Browsing a signed-in Jellyfin server (ADR 0018, sections 3, 5 and 6): library grids as paged lists ([`PagedItems`]), Home
//! ([`HomeFeed`]), detail pages ([`DetailsView`]), search and the Favourites tab's rows ([`SearchResults`]), and played and
//! favourite marks shown at once in every live list and sent to the server one at a time per item.
//!
//! A [`Browse`] belongs to one session of one server: once that session ends (sign-out, removal, a new sign-in), its lists stop
//! loading and its requests fail with [`AuthError::NotSignedIn`] without reaching the network. A new session needs a new
//! `Browse` from [`crate::App::browse`].
//!
//! Each list registers with its session's [`Context`] as a [`Live`] list, so a played or favourite change, a change that other
//! lists depend on, and a refresh reach every list that is still alive. Lists are held weakly: dropping a list (every clone of its
//! handle) stops its requests.

mod details;
mod home;
mod labels;
mod libraries;
mod paged;
mod present;
mod search;
mod search_filter;
mod states;
mod store;
mod texts;
mod views;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use current_jellyfin::{AuthError, JellyfinClient};
use current_model::{
    CollectionKind, ItemFilter, ItemQuery, ItemRef, ItemSummary, Library, MediaKind, Reachability,
    ServerRef, ServerStatus, ServiceType, SortKey, SortOrder, UserState,
};
use current_net::RuntimeHandle;
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::list::{ObservableList, VectorDiff};
use crate::session::Shared;

pub use details::{DetailsView, LoadState, SIMILAR_LIMIT};
pub use home::{
    HomeFeed, HomeRow, HomeRowKind, HomeState, LATEST_LIMIT, NEXT_UP_LIMIT, RESUME_LIMIT,
};
pub use labels::{
    FAVOURITES_EMPTY, FAVOURITES_FAILED, favourites_status, kind_choice_name, kind_group,
    library_filter_name, library_sort_name, runtime_label, time_left_label, time_left_short,
};
pub use paged::{BACKLOG_PAGES, MAX_PAGE_REQUESTS, PAGE_SIZE, PagedItems, PagedState, Slot};
pub(crate) use present::ShownAgain;
pub use present::{
    FILTERS_OFF_WHILE_SEARCHING, GridTools, LIBRARY_SEARCH_WINDOW, REFRESH_WHEN_SHOWN_AFTER,
    SORT_OFF_WHILE_SEARCHING, SeriesRun, SortChoice, TileShape, grid_tools, library_filters,
    library_search_note, library_sorts, library_toggle_filters, series_run, tile_artwork,
    with_filter_choice, with_filter_toggle,
};
pub use search::{FAVOURITE_ROW_KINDS, FAVOURITE_ROW_LIMIT, SearchResults};
pub use search_filter::{
    CATALOG_NEEDS_SEERR, CATALOG_NEEDS_SEERR_LABEL, NARROWED_SEARCH_LIMIT, SearchChoice,
    SearchFilter, SearchSource, SearchSourceChoice, SearchTools, SearchTypeChoice,
    catalog_available, person_query, search_filter_in_effect, search_note, search_result_display,
    search_tools, search_type_label, search_types,
};
pub use states::{BrowseNotice, ItemAction, NoticeKind};
pub use texts::{
    DetailsFacts, FAILED_TILE_NAME, LIBRARY_GONE_MESSAGE, LIBRARY_GONE_TITLE, LOADING_TILE_NAME,
    action_failed_title, details_facts, facts_line, item_count_label, person_kind_name,
    search_status, size_on_disk, tile_name, tile_subtitle, tile_title,
};
pub use views::{
    BrowseView, FilterMenu, GridOpening, ItemOpens, RowDisplay, RowLink, RowNote, RowTarget,
    ViewLayout, filter_menu, grid_opening, home_row_display, search_group_display, view_layout,
    view_link_tooltip, view_opening, view_query, view_title,
};

use libraries::Libraries;
use states::ItemStates;
use store::ItemStore;

/// How long after the last played or favourite change the lists that depend on it reload (ADR 0018, section 5).
pub const DEPENDENT_RELOAD_DELAY: Duration = Duration::from_secs(1);

/// What every live list does when its session's items change.
pub(crate) trait Live: Send + Sync {
    /// `item`'s user state is now `state`: patch every slot that shows it.
    fn patch(&self, item: &ItemRef, state: &UserState);
    /// Something under `items` changed (a played mark below a series, playback stopped): reload what depends on them.
    fn dependents_changed(&self, items: &HashSet<ItemRef>);
    /// Read the list again, keeping what it shows until the answers arrive.
    fn refresh(&self);
    /// Adds every item the list shows to `held`: the session's store keeps their state ([`store::ItemStore`]).
    fn holds(&self, held: &mut HashSet<ItemRef>);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every value behind these locks is changed in single steps that leave it consistent, so a poisoned lock is still usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The latest browse notice, shared by every session. Only the latest is kept: a UI that shows a message bar reads it as it
/// changes.
pub(crate) struct Notices {
    tx: watch::Sender<Option<BrowseNotice>>,
    seq: AtomicU64,
}

impl Notices {
    fn raise(&self, kind: NoticeKind) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.tx.send_replace(Some(BrowseNotice { seq, kind }));
    }
}

/// The app's browse state: the notices, and one [`Context`] per live session.
pub(crate) struct Hub {
    notices: Arc<Notices>,
    contexts: Mutex<HashMap<ServerRef, Weak<Context>>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            notices: Arc::new(Notices {
                tx: watch::Sender::new(None),
                seq: AtomicU64::new(0),
            }),
            contexts: Mutex::new(HashMap::new()),
        }
    }
}

impl Hub {
    pub(crate) fn notices(&self) -> watch::Receiver<Option<BrowseNotice>> {
        self.notices.tx.subscribe()
    }

    /// A `Browse` over `server`'s live session, sharing that session's item states with every other `Browse` of it; `None` unless
    /// it is a Jellyfin server whose status is available (signed in) and that has a session, or outside the tokio runtime.
    pub(crate) fn browse(&self, shared: &Arc<Shared>, server: &ServerRef) -> Option<Browse> {
        if server.service != ServiceType::JELLYFIN {
            return None;
        }
        let available = shared
            .capabilities
            .borrow()
            .server(server)
            .is_some_and(ServerStatus::is_available);
        if !available {
            return None;
        }
        let (session, _) = shared.current_session(server)?;
        let runtime = RuntimeHandle::try_current().ok()?;
        let mut contexts = lock(&self.contexts);
        contexts.retain(|_, context| context.strong_count() > 0);
        if let Some(context) = contexts.get(server).and_then(Weak::upgrade)
            && context.session == session
        {
            return Some(Browse { ctx: context });
        }
        let context = Arc::new(Context::new(
            server.clone(),
            session,
            Arc::downgrade(shared),
            runtime,
            self.notices.clone(),
        ));
        contexts.insert(server.clone(), Arc::downgrade(&context));
        context.start_libraries(shared.capabilities.subscribe());
        Some(Browse { ctx: context })
    }

    /// Playback of `item` stopped (ADR 0019): its session's lists that depend on it reload ([`Browse::playback_stopped`]). Nothing
    /// when no `Browse` of the item's current session lives.
    pub(crate) fn playback_stopped(&self, item: &ItemRef) {
        let context = lock(&self.contexts)
            .get(&item.server)
            .and_then(Weak::upgrade);
        if let Some(context) = context.filter(|context| context.is_current()) {
            Browse { ctx: context }.playback_stopped(item);
        }
    }

    /// Playback of `item` is about to be reported started: the workaround of [`Context::mend`] runs first.
    pub(crate) async fn before_play(&self, item: &ItemRef) {
        let context = lock(&self.contexts)
            .get(&item.server)
            .and_then(Weak::upgrade);
        if let Some(context) = context.filter(|context| context.is_current()) {
            context.mend(item).await;
        }
    }

    /// Refreshes every live list of every session still current: the app came back to the foreground.
    pub(crate) fn refresh_all(&self) {
        let contexts: Vec<Arc<Context>> = lock(&self.contexts)
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        for context in contexts.iter().filter(|context| context.is_current()) {
            context.refresh();
        }
    }
}

#[derive(Default)]
struct Dependents {
    changed: HashSet<ItemRef>,
    /// Items whose marks are read again when the timer fires ([`Context::read_back`]).
    read_back: HashSet<ItemRef>,
    /// Whether the lists reload when it fires: a read again alone does not reload them.
    reload: bool,
    timer: Option<AbortHandle>,
    /// Counts the schedules: a timer that fires after a newer schedule leaves the reload to the newer timer.
    generation: u64,
}

/// Whether a resync ([`Context::resync`]) runs, and whether another was asked for meanwhile.
#[derive(Default)]
struct Resync {
    running: bool,
    again: bool,
}

/// Workaround for an upstream Jellyfin bug: the series and seasons this session marked, and the episodes whose own instance on the
/// server was put right since ([`Context::mend`]).
#[derive(Default)]
struct Sweeps {
    /// When each series or season was marked last, on the session's clock.
    swept: HashMap<ItemRef, u64>,
    /// When each episode's played mark was sent on its own id last.
    mended: HashMap<ItemRef, u64>,
}

/// How many items one resync request asks for ([`Context::resync`]): their ids go in the URL.
const RESYNC_CHUNK: usize = 50;

/// One session's browse state: its store of the items' states, the changes wanted, and its live lists.
pub(crate) struct Context {
    server: ServerRef,
    /// The session this belongs to ([`Shared::current_session`]).
    session: u64,
    shared: Weak<Shared>,
    runtime: RuntimeHandle,
    notices: Arc<Notices>,
    store: ItemStore,
    states: ItemStates,
    lives: Mutex<Vec<Weak<dyn Live>>>,
    /// Serializes publishing an item's state, so the last publish shows the newest state.
    publishing: Mutex<()>,
    dependents: Mutex<Dependents>,
    resync: Mutex<Resync>,
    sweeps: Mutex<Sweeps>,
    libraries: Libraries,
    /// Orders the lists' reads and the server's answers ([`store::ItemStore`]).
    clock: AtomicU64,
}

impl Drop for Context {
    fn drop(&mut self) {
        if let Some(timer) = lock(&self.dependents).timer.take() {
            timer.abort();
        }
    }
}

impl Context {
    fn new(
        server: ServerRef,
        session: u64,
        shared: Weak<Shared>,
        runtime: RuntimeHandle,
        notices: Arc<Notices>,
    ) -> Self {
        Self {
            server,
            session,
            shared,
            runtime,
            notices,
            store: ItemStore::default(),
            states: ItemStates::default(),
            lives: Mutex::new(Vec::new()),
            publishing: Mutex::new(()),
            dependents: Mutex::new(Dependents::default()),
            resync: Mutex::new(Resync::default()),
            sweeps: Mutex::new(Sweeps::default()),
            libraries: Libraries::default(),
            clock: AtomicU64::new(0),
        }
    }

    /// The session's client, while the session is the server's current one.
    fn client(&self) -> Result<Arc<JellyfinClient>, AuthError> {
        let shared = self.shared.upgrade().ok_or(AuthError::NotSignedIn)?;
        match shared.current_session(&self.server) {
            Some((session, client)) if session == self.session => Ok(client),
            _ => Err(AuthError::NotSignedIn),
        }
    }

    fn is_current(&self) -> bool {
        self.shared
            .upgrade()
            .and_then(|shared| shared.session_id(&self.server))
            == Some(self.session)
    }

    /// Whether the server's status says it cannot be reached now: lists keep what they last loaded, marked offline.
    fn is_unreachable(&self) -> bool {
        self.shared.upgrade().is_some_and(|shared| {
            shared
                .capabilities
                .borrow()
                .server(&self.server)
                .is_some_and(|status| status.reachability == Reachability::Unreachable)
        })
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> AbortHandle {
        self.runtime.spawn(future).abort_handle()
    }

    fn register(&self, live: Weak<dyn Live>) {
        let mut lives = lock(&self.lives);
        lives.retain(|live| live.strong_count() > 0);
        lives.push(live);
    }

    fn lives(&self) -> Vec<Arc<dyn Live>> {
        lock(&self.lives).iter().filter_map(Weak::upgrade).collect()
    }

    /// Reads the libraries and every live list again.
    fn refresh(self: &Arc<Self>) {
        self.reload_libraries();
        for live in self.lives() {
            live.refresh();
        }
    }

    /// The next time on the session's clock: a list takes it when its request goes out, an answer when it arrives.
    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A list read `items` at `at` (below `series`, for a series' seasons): each takes the store's state if the store's is newer, or
    /// becomes it if older, then shows the changes pending for it or above it. Returns the items whose state the read changed: the
    /// list hands them to [`Context::spread`] once it shows them, so every other list shows them too.
    #[must_use]
    fn take(&self, items: &mut [ItemSummary], at: u64, series: Option<&ItemRef>) -> Vec<ItemRef> {
        let mut changed = Vec::new();
        for summary in items {
            if self.store.read(summary, at, series) {
                changed.push(summary.item.clone());
            }
            self.states.observe_under(summary, series);
        }
        changed
    }

    /// Shows the items a read changed ([`Context::take`]) in every live list, and drops from the store what no list holds now.
    fn spread(&self, changed: Vec<ItemRef>) {
        for item in changed {
            self.publish(&item);
        }
        let mut held = HashSet::new();
        for live in self.lives() {
            live.holds(&mut held);
        }
        self.store.retain(&held);
    }

    /// `item` as the lists show it: the store's state with the changes pending for it or above it.
    fn shown(&self, item: &ItemRef) -> Option<ItemSummary> {
        let mut summary = self.store.get(item)?;
        let series = self.store.series_of(item);
        summary.user = self.states.view(&summary, series.as_ref());
        Some(summary)
    }

    /// The series and season of `item`: its own links, else the series of a season as the page listing it said.
    fn parents_of(&self, item: &ItemRef, found: Option<&ItemSummary>) -> Vec<ItemRef> {
        let links = found.map(states::parents).unwrap_or_default();
        if !links.is_empty() {
            return links;
        }
        self.store.series_of(item).into_iter().collect()
    }

    /// Reads the marks of `items` (a series and season whose counts changed) from the server, one request each, and shows them in
    /// every live list. They correct the estimate shown at once ([`states::estimate_parent`]), which is off where the server counts
    /// what the lists do not show (episodes not loaded, or marked elsewhere). An answer is dropped when a played change was wanted
    /// after its request went out (its count may predate it), and that item is read again after the next delay; a failed read
    /// keeps what the lists show, with no notice.
    async fn read_marks_again(self: Arc<Self>, items: HashSet<ItemRef>) {
        let Ok(client) = self.client() else {
            return;
        };
        for item in items {
            let wanted = self.states.wanted(&item);
            let played = self.states.played_wants();
            let at = self.tick();
            let Ok(state) = client.user_data(&item).await else {
                // The lists keep the estimate; the next load shows the server's state.
                self.states.settle(&item, played);
                continue;
            };
            if self.states.played_wants() != played {
                self.read_back(Some(item));
            } else if self.states.wanted(&item) == wanted {
                self.states.settle(&item, played);
                self.store.answer(&item, state, at);
                self.publish(&item);
            }
        }
    }

    /// The marks of `items` are read again with the dependent reload ([`Context::dependents_changed`]): once per item however many
    /// changes came within the delay.
    fn read_back(self: &Arc<Self>, items: impl IntoIterator<Item = ItemRef>) {
        let items: Vec<ItemRef> = items.into_iter().collect();
        if items.is_empty() {
            return;
        }
        lock(&self.dependents).read_back.extend(items);
        self.schedule(false);
    }

    /// Sends `item`'s state to every live list: the pending state if a change is pending, else the store's with the changes pending
    /// above it. Read under one lock, so publishes for one item cannot land out of order: the last one shows the newest state.
    fn publish(&self, item: &ItemRef) {
        let _order = lock(&self.publishing);
        let Some(state) = self
            .states
            .pending(item)
            .or_else(|| self.shown(item).map(|summary| summary.user))
        else {
            return;
        };
        for live in self.lives() {
            live.patch(item, &state);
        }
    }

    /// Reads the state of every item the live lists hold again, [`RESYNC_CHUNK`] a request (`GET Items?ids=`, whose user data
    /// Jellyfin reads fresh), and shows what changed: a page shown again within [`REFRESH_WHEN_SHOWN_AFTER`], and a sweep whose
    /// items the core cannot name (a collection's, a folder's), take what another device or the server changed meanwhile (owner,
    /// 2026-10-27). One runs at a time; one asked for meanwhile runs after it. A failure leaves the lists as they are, with no notice.
    fn resync(self: &Arc<Self>) {
        {
            let mut resync = lock(&self.resync);
            if resync.running {
                resync.again = true;
                return;
            }
            resync.running = true;
        }
        let context = self.clone();
        self.spawn(async move {
            loop {
                context.resync_once().await;
                let mut resync = lock(&context.resync);
                if !std::mem::take(&mut resync.again) {
                    resync.running = false;
                    return;
                }
            }
        });
    }

    async fn resync_once(&self) {
        let Ok(client) = self.client() else {
            return;
        };
        let items = self.store.items();
        for chunk in items.chunks(RESYNC_CHUNK) {
            let at = self.tick();
            let Ok(states) = client.user_states(chunk).await else {
                return;
            };
            for (item, state) in states {
                if self.store.answer(&item, state, at) {
                    self.publish(&item);
                }
            }
        }
    }

    /// The lists depending on `items` reload [`DEPENDENT_RELOAD_DELAY`] after the last such call.
    fn dependents_changed(self: &Arc<Self>, items: impl IntoIterator<Item = ItemRef>) {
        lock(&self.dependents).changed.extend(items);
        self.schedule(true);
    }

    /// (Re)starts the timer of [`Context::dependents_changed`]; `reload` when the lists are to reload, not only marks to be read.
    fn schedule(self: &Arc<Self>, reload: bool) {
        let mut dependents = lock(&self.dependents);
        dependents.reload |= reload;
        if let Some(timer) = dependents.timer.take() {
            timer.abort();
        }
        dependents.generation += 1;
        let generation = dependents.generation;
        let context = Arc::downgrade(self);
        dependents.timer = Some(self.spawn(async move {
            tokio::time::sleep(DEPENDENT_RELOAD_DELAY).await;
            let Some(context) = context.upgrade() else {
                return;
            };
            let (reload, changed, read_back) = {
                let mut dependents = lock(&context.dependents);
                // A newer schedule came while this one woke: its timer reloads, a second after it.
                if dependents.generation != generation {
                    return;
                }
                dependents.timer = None;
                // A played change still waits for the server, whose counts may not include it yet: its answer schedules again, and
                // everything gathered so far waits for that.
                if context.states.played_pending() {
                    return;
                }
                (
                    std::mem::take(&mut dependents.reload),
                    std::mem::take(&mut dependents.changed),
                    std::mem::take(&mut dependents.read_back),
                )
            };
            if !read_back.is_empty() {
                context.spawn(context.clone().read_marks_again(read_back));
            }
            if reload {
                for live in context.lives() {
                    live.dependents_changed(&changed);
                }
            }
        }));
    }

    fn want(self: &Arc<Self>, item: &ItemRef, action: ItemAction) {
        // The item as the store holds it (the server's state), and as the lists show it before this change.
        let held = self.store.get(item);
        let before = self.shown(item);
        let parents = self.parents_of(item, held.as_ref());
        // What the lists show below the item before the change: a failure rolls them back to it.
        let below: Vec<ItemSummary> = if matches!(action, ItemAction::SetPlayed(_)) {
            self.store
                .below(item)
                .into_iter()
                .filter_map(|summary| self.shown(&summary.item))
                .collect()
        } else {
            Vec::new()
        };
        // What the lists show is the base the change is measured from: an episode unmarked while its series' mark is pending shows
        // played, and its unmark must go out, since the series' mark reaches it on the server too.
        let (_, start) = self
            .states
            .want(item, action, before.as_ref(), parents.clone());
        self.publish(item);
        if let ItemAction::SetPlayed(played) = action {
            // A series or season: the episodes and seasons below it take the mark at once, as the server gives it to them.
            for below in below {
                self.states.keep_below(item, &below);
                self.publish(&below.item);
            }
            // An episode or season: its series and season show the count it moves at once; the server's follows its answer.
            if let Some(before) = &before {
                for parent in &parents {
                    if let Some(mut shown) = self.shown(parent) {
                        states::estimate_parent(&mut shown.user, &before.user, played);
                        self.states.estimate(parent, &shown.user);
                        self.publish(parent);
                    }
                }
            }
        }
        if start {
            let context = self.clone();
            let item = item.clone();
            self.spawn(async move { context.send_changes(item).await });
        }
    }

    /// Workaround for an upstream Jellyfin bug: Jellyfin saves a favourite, a playback start or a progress or stop report on an
    /// episode with the user data its cached instance of the episode holds, which a series' or season's sweep does not update, so
    /// the write puts the episode's old played mark back in the database (`docs/notes/jellyfin-browse.md`, "Upstream bugs and our
    /// workarounds"). Jellyfin v12.3 (tag 06de749, read 2026-10-27; v12.2 at ca0f16eb has the same lines):
    /// `UserLibraryController.MarkFavorite` ("`var data = _userDataRepository.GetUserData(user, item); data.IsFavorite =
    /// isFavorite;`"), `SessionManager.OnPlaybackStart` ("`var data = _userDataManager.GetUserData(user, item); data.PlayCount++;`"),
    /// `UserDataManager.GetUserData` (which reads the instance: "`ResolveUserDataRow(item, item.UserData?...)`") and `SaveUserData`
    /// (which saves the whole row). A played mark on the item's own id saves through that instance and reloads it ("`// rehydrate the
    /// cached userdata`"). So before such a write on an episode whose series or season this session marked since its last mend, the
    /// episode's played mark as a list reads it (`GET Items?ids=`) is sent on its own id. It covers this session's sweeps only, not
    /// another device's or an earlier session's. Marking played again moves the episode's last-played date to now. A failure sends
    /// the write anyway. It can go once a write on an item reads its user data fresh: re-check on each new server baseline.
    async fn mend(&self, item: &ItemRef) {
        let parents = self.parents_of(item, self.store.get(item).as_ref());
        let swept = {
            let sweeps = lock(&self.sweeps);
            let since = sweeps.mended.get(item).copied().unwrap_or(0);
            parents
                .iter()
                .filter_map(|parent| sweeps.swept.get(parent))
                .any(|at| *at > since)
        };
        if !swept {
            return;
        }
        let Ok(client) = self.client() else {
            return;
        };
        let at = self.tick();
        let Ok(states) = client.user_states(std::slice::from_ref(item)).await else {
            return;
        };
        let Some((_, state)) = states.into_iter().find(|(found, _)| found == item) else {
            return;
        };
        if client.set_played(item, state.played).await.is_ok() {
            lock(&self.sweeps).mended.insert(item.clone(), at);
        }
    }

    /// Records the answered played change on `item`: a series' or season's sweep (`answered`, the folder's own state) for
    /// [`Context::mend`], and the items below it in the store, as the server marked them. Returns whether the item is a folder whose
    /// items below the core cannot name (a collection, a plain folder): those are read again ([`Context::resync`]).
    fn swept(&self, item: &ItemRef, played: bool, answered: &UserState) -> bool {
        let kind = self.store.get(item).map(|summary| summary.kind);
        let series_or_season =
            matches!(&kind, Some(kind) if *kind == MediaKind::SERIES || *kind == MediaKind::SEASON);
        let below = self.store.below(item);
        if series_or_season || !below.is_empty() {
            let at = self.tick();
            lock(&self.sweeps).swept.insert(item.clone(), at);
            for summary in below {
                let mut state = summary.user;
                states::set_played(&mut state, played);
                self.store.answer(&summary.item, state, at);
            }
            return false;
        }
        // A folder carries a count of the items inside it; a film or an episode does not.
        answered.unplayed_count.is_some()
    }

    /// Sends `item`'s wanted changes one at a time until none is left (ADR 0018, section 5).
    async fn send_changes(self: Arc<Self>, item: ItemRef) {
        while let Some(action) = self.states.next(&item) {
            let result = match self.client() {
                Ok(client) => match action {
                    ItemAction::SetPlayed(played) => client.set_played(&item, played).await,
                    ItemAction::SetFavourite(favourite) => {
                        self.mend(&item).await;
                        client.set_favourite(&item, favourite).await
                    }
                },
                Err(error) => Err(error),
            };
            let mut unnamed_below = false;
            if let Ok(state) = &result {
                let at = self.tick();
                self.store.answer(&item, state.clone(), at);
                if let ItemAction::SetPlayed(played) = action {
                    unnamed_below = self.swept(&item, played, state);
                }
            }
            let answered = self.states.answered(&item, action, result);
            self.publish(&item);
            // A failure: the items below show the store's state again.
            for below in answered.rolled_back {
                self.publish(&below.item);
            }
            // A success: they show what the server marked them ([`Context::swept`]).
            if matches!(action, ItemAction::SetPlayed(_)) && answered.failure.is_none() {
                for below in self.store.below(&item) {
                    self.publish(&below.item);
                }
            }
            if unnamed_below {
                self.resync();
            }
            match answered.failure {
                Some((name, problem)) => {
                    // The parents showed an estimate: the server's state replaces it. The item's page and those below it read the
                    // server again too, in case a list showed the change in a way the rollback does not reach.
                    if matches!(action, ItemAction::SetPlayed(_)) {
                        self.read_back(answered.parents.clone());
                        let mut dependents = answered.parents.clone();
                        dependents.push(item.clone());
                        self.dependents_changed(dependents);
                    }
                    self.notices.raise(NoticeKind::ActionFailed {
                        action,
                        item_name: name,
                        problem,
                    });
                }
                // A mark changes the lists chosen by marks (Home's rows, the Favourites tab, a filtered grid): they read again a moment
                // later. A played mark changes its series' and season's own marks and counts: they are read again then, and their pages
                // and the item's own (a series' episodes, a collection's films, which the server marked too) reload with the rest.
                None if matches!(action, ItemAction::SetPlayed(_)) => {
                    let mut changed = answered.parents.clone();
                    // A series' seasons have counts of their own.
                    changed.extend(
                        self.store
                            .below(&item)
                            .into_iter()
                            .filter(|below| below.user.unplayed_count.is_some())
                            .map(|below| below.item),
                    );
                    self.read_back(changed);
                    let mut dependents = answered.parents;
                    dependents.push(item.clone());
                    self.dependents_changed(dependents);
                }
                None => self.dependents_changed(answered.parents),
            }
        }
    }
}

/// Browsing one session of a signed-in Jellyfin server. Cheap to clone: clones share the session's state.
#[derive(Clone)]
pub struct Browse {
    ctx: Arc<Context>,
}

impl std::fmt::Debug for Browse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browse")
            .field("server", &self.ctx.server)
            .finish_non_exhaustive()
    }
}

impl Browse {
    pub fn server(&self) -> &ServerRef {
        &self.ctx.server
    }

    /// The user's libraries, in the user's order, as a load that starts now reads them; the list [`Browse::library_list`] holds
    /// takes them too.
    ///
    /// # Errors
    /// The session ended ([`AuthError::NotSignedIn`]), or the server's or the network's failure.
    pub async fn libraries(&self) -> Result<Vec<Library>, AuthError> {
        self.ctx.libraries_again().await
    }

    /// The user's libraries for the app's navigation, in the user's order: `None` until first loaded, then the last list loaded.
    /// It follows the server: read again on a refresh, on the foreground refresh, when Home reloads, after a library request the
    /// server refused, and when the server is reached again after a failed load. A library the user can no longer open leaves it.
    pub fn library_list(&self) -> watch::Receiver<Option<Vec<Library>>> {
        self.ctx.libraries.subscribe()
    }

    /// A grid of `library`'s items: `query` with the library as its parent. Starts loading at once. [`library_query`] is what a
    /// grid shows before the user sorts or filters it. When a request fails as it does for a library that is gone, the libraries
    /// are read again; if the library left them, the grid's state says [`AuthError::NotAvailable`].
    pub fn open_library(&self, library: &Library, query: ItemQuery) -> PagedItems {
        let query = ItemQuery {
            parent: Some(library.item.clone()),
            ..query
        };
        PagedItems::open(self.ctx.clone(), query, Some(library.item.clone()))
    }

    /// A grid of items from every library: `query` with no parent (a Favourites row's whole list and the Recently added tab,
    /// [`favourites_query`] and [`recently_added_everywhere_query`]). Starts loading at once.
    pub fn open_items(&self, query: ItemQuery) -> PagedItems {
        PagedItems::open(
            self.ctx.clone(),
            ItemQuery {
                parent: None,
                ..query
            },
            None,
        )
    }

    /// Home's rows. Start loading at once.
    pub fn home(&self) -> HomeFeed {
        HomeFeed::open(self.ctx.clone())
    }

    /// `item`'s detail page. Starts loading at once.
    pub fn details(&self, item: &ItemRef) -> DetailsView {
        DetailsView::open(self.ctx.clone(), item.clone())
    }

    /// The best matches for `term`, one group per kind, of the types and sources `filter` keeps as it can apply now
    /// ([`search_filter_in_effect`]): every kind, [`current_jellyfin::SEARCH_LIMIT`] of each, or one kind,
    /// [`NARROWED_SEARCH_LIMIT`] of it. Starts loading at once.
    pub fn search(&self, term: &str, filter: &SearchFilter) -> SearchResults {
        let filter = match self.ctx.shared.upgrade() {
            Some(shared) => search_filter_in_effect(&shared.capabilities.borrow(), filter),
            // The app is gone: the search fails as signed out, whatever it asks for.
            None => filter.clone(),
        };
        SearchResults::open(self.ctx.clone(), term.to_owned(), filter)
    }

    /// The Favourites tab (owner, 2026-10-27): the user's favourite films, series and episodes, a row each ([`FAVOURITE_ROW_KINDS`]),
    /// as many as [`FAVOURITE_ROW_LIMIT`] in each, by name; a row only while it has items. They read again a moment after a played or
    /// favourite mark changes. A row's header opens the whole list: [`Browse::open_items`] with [`favourites_query`] for its kind.
    /// Starts loading at once.
    pub fn favourites(&self) -> SearchResults {
        SearchResults::favourites(self.ctx.clone())
    }

    /// Marks `item` played or unplayed: every live list shows it at once, the server is told, and on failure the change rolls
    /// back with a notice ([`crate::App::browse_notices`]).
    pub fn set_played(&self, item: &ItemRef, played: bool) {
        self.ctx.want(item, ItemAction::SetPlayed(played));
    }

    /// Marks `item` a favourite or not, as [`Browse::set_played`] does.
    pub fn set_favourite(&self, item: &ItemRef, favourite: bool) {
        self.ctx.want(item, ItemAction::SetFavourite(favourite));
    }

    /// `item`'s state as every list shows it, if a list holds it: a change still pending for it, else the session's store's state
    /// with the changes pending above it.
    pub fn user_state(&self, item: &ItemRef) -> Option<UserState> {
        self.ctx
            .states
            .pending(item)
            .or_else(|| self.ctx.shown(item).map(|summary| summary.user))
    }

    /// Refreshes every live list of this session, as the user asked.
    pub fn refresh(&self) {
        self.ctx.refresh();
    }

    /// Playback of `item` stopped (ADR 0019 calls this, after the server was told): the lists that depend on it ("continue
    /// watching", "next up", its series and season) reload, as after a played change; and every live list showing the item itself
    /// takes its new marks (its resume point, played) from the server, so a row that does not reload (a library's latest, a grid)
    /// does not keep the old ones, and Play on any tile of it resumes (found on the owner's run, 2026-10-25).
    pub fn playback_stopped(&self, item: &ItemRef) {
        let parents = self
            .ctx
            .store
            .get(item)
            .map(|summary| states::parents(&summary))
            .unwrap_or_default();
        // An episode played to its end changes its series' and season's counts.
        self.ctx.read_back(parents.clone());
        let mut items = parents;
        items.push(item.clone());
        self.ctx.dependents_changed(items);
        let context = self.ctx.clone();
        let item = item.clone();
        self.ctx.spawn(async move {
            let Ok(client) = context.client() else {
                return;
            };
            let wanted = context.states.wanted(&item);
            let at = context.tick();
            // A failure leaves the lists as they were; the dependent reload still runs. A mark the user made meanwhile is newer than
            // this answer, even once the server confirmed it: the answer is dropped.
            if let Ok(details) = client.details(&item).await
                && context.states.wanted(&item) == wanted
                && context.store.answer(&item, details.summary.user, at)
            {
                context.publish(&item);
            }
        });
    }
}

/// Makes `list` hold `values` with `Set`, `Append` and `Remove` diffs only, so a UI keeps its scroll position.
fn sync_list<T: Clone + PartialEq + Send + 'static>(list: &ObservableList<T>, values: Vec<T>) {
    let old = list.snapshot();
    let mut diffs = Vec::new();
    for (index, (old, new)) in old.iter().zip(&values).enumerate() {
        if old != new {
            diffs.push(VectorDiff::Set {
                index,
                value: new.clone(),
            });
        }
    }
    if values.len() > old.len() {
        diffs.push(VectorDiff::Append {
            values: values[old.len()..].to_vec(),
        });
    }
    for index in (values.len()..old.len()).rev() {
        diffs.push(VectorDiff::Remove { index });
    }
    // Cannot fail: every index is below the length it is applied to.
    let _ = list.change_many(diffs);
}

/// Gives `item`'s new state to every summary of it in `list`.
fn patch_list(list: &ObservableList<ItemSummary>, item: &ItemRef, state: &UserState) {
    let diffs: Vec<_> = list
        .snapshot()
        .into_iter()
        .enumerate()
        .filter(|(_, summary)| &summary.item == item && &summary.user != state)
        .map(|(index, mut summary)| {
            summary.user = state.clone();
            VectorDiff::Set {
                index,
                value: summary,
            }
        })
        .collect();
    // Cannot fail: the indices come from the list, under the owner's lock.
    let _ = list.change_many(diffs);
}

/// What `library`'s grid shows before the user sorts or filters it, by name: a film library its films and a TV library its series,
/// wherever they sit below it, as the server's own web client lists them; any other library its top level, so folders, albums and
/// collections show as themselves.
pub fn library_query(library: &Library) -> ItemQuery {
    let kinds = match &library.collection {
        Some(kind) if *kind == CollectionKind::MOVIES => vec![MediaKind::MOVIE],
        Some(kind) if *kind == CollectionKind::TV_SHOWS => vec![MediaKind::SERIES],
        _ => Vec::new(),
    };
    ItemQuery {
        parent: Some(library.item.clone()),
        recursive: !kinds.is_empty(),
        kinds,
        sort: vec![SortKey::SORT_NAME],
        ..ItemQuery::default()
    }
}

/// What a Home row of `library`'s latest items opens when its header is chosen (owner, 2026-10-26): the library's grid, the newest
/// added first.
pub fn recently_added_query(library: &Library) -> ItemQuery {
    ItemQuery {
        sort: vec![SortKey::DATE_CREATED],
        order: SortOrder::Descending,
        ..library_query(library)
    }
}

/// The kinds the views across every library list (owner, 2026-10-26): films and series as a library lists them, with other videos
/// and music videos; never an episode on its own.
const VIEW_KINDS: [MediaKind; 4] = [
    MediaKind::MOVIE,
    MediaKind::SERIES,
    MediaKind::VIDEO,
    MediaKind::MUSIC_VIDEO,
];

/// The media types a view across every library can be narrowed to, in the order the UI lists them after "every type" (owner,
/// 2026-10-26).
pub fn view_kinds() -> Vec<MediaKind> {
    vec![MediaKind::MOVIE, MediaKind::SERIES]
}

fn view_kinds_for(kind: Option<MediaKind>) -> Vec<MediaKind> {
    match kind {
        Some(kind) => vec![kind],
        None => VIEW_KINDS.to_vec(),
    }
}

/// Every item of `kind` in every library, by name (owner, 2026-10-27): the Movies, Series or Episodes page a Favourites row's header
/// opens, with the favourites tick on ([`favourites_query`]).
pub fn kind_query(kind: MediaKind) -> ItemQuery {
    ItemQuery {
        recursive: true,
        kinds: vec![kind],
        sort: vec![SortKey::SORT_NAME],
        ..ItemQuery::default()
    }
}

/// The user's favourites in every library, by name (owner, 2026-10-26 and 2026-10-27): with `kind`, [`kind_query`] with the
/// favourites tick on, as a Favourites row's header opens it (episodes too, [`FAVOURITE_ROW_KINDS`]); without, Home's Favourites
/// row, of the kinds the views across every library list, with no episode on its own.
pub fn favourites_query(kind: Option<MediaKind>) -> ItemQuery {
    let query = match kind {
        Some(kind) => kind_query(kind),
        None => ItemQuery {
            recursive: true,
            kinds: view_kinds_for(None),
            sort: vec![SortKey::SORT_NAME],
            ..ItemQuery::default()
        },
    };
    with_filter_toggle(&query, &ItemFilter::FAVOURITE, true)
}

/// The Recently added tab and Home's Recently added row (owner, 2026-10-26): what was added to every library, the newest first; only
/// `kind` when given. A series counts from when it was added: one is not moved up when an episode is added to it (Jellyfin keeps that
/// date for folders only, so sorting films and series together by it would put the films last; not verified on a server).
pub fn recently_added_everywhere_query(kind: Option<MediaKind>) -> ItemQuery {
    ItemQuery {
        recursive: true,
        kinds: view_kinds_for(kind),
        sort: vec![SortKey::DATE_CREATED],
        order: SortOrder::Descending,
        ..ItemQuery::default()
    }
}

/// What plays by itself, for Seen recently: films, episodes and other videos, each as it was watched.
const PLAYABLE: [MediaKind; 4] = [
    MediaKind::MOVIE,
    MediaKind::EPISODE,
    MediaKind::VIDEO,
    MediaKind::MUSIC_VIDEO,
];

/// Home's Seen recently row (owner, 2026-10-26): what the user finished last, in every library, the latest first.
pub fn seen_recently_query() -> ItemQuery {
    ItemQuery {
        recursive: true,
        kinds: PLAYABLE.to_vec(),
        filters: vec![ItemFilter::PLAYED],
        sort: vec![SortKey::DATE_PLAYED],
        order: SortOrder::Descending,
        ..ItemQuery::default()
    }
}

/// What a library grid loads for the text in its own search box (owner, 2026-10-26 and 2026-10-27), from `base`, the query it showed
/// before searching, which the UI keeps: the text without the spaces round it, searched for in `base`'s items (its library, kinds and
/// depth) with no sort and no filter, so the best matches come first; when no text is left, `base` itself, with its sort and filters
/// back.
pub fn library_search(base: &ItemQuery, term: &str) -> ItemQuery {
    let term = term.trim();
    if term.is_empty() {
        return ItemQuery {
            search_term: None,
            ..base.clone()
        };
    }
    ItemQuery {
        parent: base.parent.clone(),
        kinds: base.kinds.clone(),
        recursive: base.recursive,
        person: base.person.clone(),
        search_term: Some(term.to_owned()),
        ..ItemQuery::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_views_across_libraries_list_every_library_s_playable_items() {
        let favourites = favourites_query(None);
        assert!(favourites.parent.is_none());
        assert!(favourites.recursive);
        assert_eq!(favourites.filters, [ItemFilter::FAVOURITE]);
        assert!(favourites.kinds.contains(&MediaKind::SERIES));
        assert!(favourites.kinds.contains(&MediaKind::MOVIE));
        // No episode on its own: its series shows.
        assert!(!favourites.kinds.contains(&MediaKind::EPISODE));
        assert_eq!(
            favourites_query(Some(MediaKind::MOVIE)).kinds,
            [MediaKind::MOVIE]
        );
        // Each Favourites row's header opens its kind, episodes included.
        for kind in FAVOURITE_ROW_KINDS {
            let row = favourites_query(Some(kind.clone()));
            assert_eq!(row.kinds, [kind]);
            assert_eq!(row.filters, [ItemFilter::FAVOURITE]);
            assert!(row.recursive && row.depends_on_marks());
        }
        assert_eq!(
            FAVOURITE_ROW_KINDS,
            [MediaKind::MOVIE, MediaKind::SERIES, MediaKind::EPISODE]
        );

        let recent = recently_added_everywhere_query(None);
        assert!(recent.recursive);
        assert_eq!(recent.sort, [SortKey::DATE_CREATED]);
        assert_eq!(recent.order, SortOrder::Descending);
        assert!(recent.kinds.contains(&MediaKind::SERIES));
        assert!(!recent.kinds.contains(&MediaKind::EPISODE));
        assert_eq!(
            recently_added_everywhere_query(Some(MediaKind::SERIES)).kinds,
            [MediaKind::SERIES]
        );
        assert_eq!(view_kinds(), [MediaKind::MOVIE, MediaKind::SERIES]);

        let seen = seen_recently_query();
        assert_eq!(seen.filters, [ItemFilter::PLAYED]);
        assert_eq!(seen.sort, [SortKey::DATE_PLAYED]);
        assert_eq!(seen.order, SortOrder::Descending);
    }

    #[test]
    fn a_library_search_takes_the_typed_text_trimmed_and_nothing_when_blank() {
        use current_model::{ItemId, ServerKey};
        let library = ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new("57a2d1e0000000000000000000001001"),
        );
        let base = ItemQuery {
            parent: Some(library),
            kinds: vec![MediaKind::MOVIE],
            recursive: true,
            sort: vec![SortKey::DATE_CREATED],
            order: SortOrder::Descending,
            filters: vec![ItemFilter::UNPLAYED],
            genres: vec!["Drama".into()],
            years: vec![2010],
            name_starts_with: Some("E".into()),
            search_term: None,
            person: None,
        };
        let searching = library_search(&base, "  Example  ");
        assert_eq!(searching.search_term.as_deref(), Some("Example"));
        // The library's items, by the server's relevance: no sort, no filter.
        assert_eq!(searching.parent, base.parent);
        assert_eq!(searching.kinds, base.kinds);
        assert!(searching.recursive);
        assert!(searching.sort.is_empty());
        assert_eq!(searching.order, SortOrder::Ascending);
        assert!(searching.filters.is_empty());
        assert!(searching.genres.is_empty() && searching.years.is_empty());
        assert_eq!(searching.name_starts_with, None);
        assert!(!searching.depends_on_marks());
        // Cleared: the sort and filters come back.
        assert_eq!(library_search(&base, "   "), base);
        assert_eq!(library_search(&base, ""), base);
        // A base that still searches (a UI that kept the searching query) loses its search.
        assert_eq!(
            library_search(&searching, ""),
            ItemQuery {
                search_term: None,
                ..searching.clone()
            }
        );
    }

    #[test]
    fn a_library_s_first_query_follows_what_it_holds() {
        use current_model::{ItemId, ServerKey};
        let library = |collection: Option<CollectionKind>| Library {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new("57a2d1e0000000000000000000001001"),
            ),
            name: "Example Library".into(),
            collection,
            artwork: Vec::new(),
        };
        let films = library_query(&library(Some(CollectionKind::MOVIES)));
        assert_eq!(films.kinds, [MediaKind::MOVIE]);
        assert!(films.recursive);
        assert_eq!(films.sort, [SortKey::SORT_NAME]);
        assert_eq!(films.order, SortOrder::Ascending);
        assert_eq!(films.parent, Some(library(None).item));
        let shows = library_query(&library(Some(CollectionKind::TV_SHOWS)));
        assert_eq!(shows.kinds, [MediaKind::SERIES]);
        assert!(shows.recursive);
        for other in [
            None,
            Some(CollectionKind::MUSIC),
            Some(CollectionKind::new("unknown_kind")),
        ] {
            let query = library_query(&library(other));
            assert!(query.kinds.is_empty());
            assert!(!query.recursive);
            assert_eq!(query.sort, [SortKey::SORT_NAME]);
        }

        // A Latest row's header: the same items, the newest added first.
        let recent = recently_added_query(&library(Some(CollectionKind::MOVIES)));
        assert_eq!(recent.sort, [SortKey::DATE_CREATED]);
        assert_eq!(recent.order, SortOrder::Descending);
        assert_eq!(
            ItemQuery {
                sort: films.sort.clone(),
                order: films.order,
                ..recent
            },
            films
        );
    }

    #[tokio::test]
    async fn sync_list_uses_set_append_and_remove_only() {
        let list = ObservableList::new();
        list.append(vec![1, 2, 3]);
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        sync_list(&list, vec![1, 5, 3, 4]);
        assert_eq!(
            sub.next_batch().await,
            Some(vec![
                VectorDiff::Set { index: 1, value: 5 },
                VectorDiff::Append { values: vec![4] }
            ])
        );
        sync_list(&list, vec![1]);
        assert_eq!(
            sub.next_batch().await,
            Some(vec![
                VectorDiff::Remove { index: 3 },
                VectorDiff::Remove { index: 2 },
                VectorDiff::Remove { index: 1 }
            ])
        );
        assert_eq!(list.snapshot(), vec![1]);
    }
}
