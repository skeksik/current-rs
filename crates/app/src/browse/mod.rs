//! Browsing a signed-in Jellyfin server (ADR 0018, sections 3, 5 and 6): library grids as paged lists ([`PagedItems`]), Home
//! ([`HomeFeed`]), detail pages ([`DetailsView`]), search ([`SearchResults`]), and played and favourite marks shown at once in
//! every live list and sent to the server one at a time per item.
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
mod paged;
mod search;
mod states;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use current_jellyfin::{AuthError, JellyfinClient};
use current_model::{
    ItemQuery, ItemRef, ItemSummary, Library, Reachability, ServerRef, ServerStatus, ServiceType,
    UserState,
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
pub use paged::{BACKLOG_PAGES, MAX_PAGE_REQUESTS, PAGE_SIZE, PagedItems, PagedState, Slot};
pub use search::SearchResults;
pub use states::{BrowseNotice, ItemAction, NoticeKind};

use states::ItemStates;

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
    /// `item` as the list shows it, if it does.
    fn find(&self, item: &ItemRef) -> Option<ItemSummary>;
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
        let context = Arc::new(Context {
            server: server.clone(),
            session,
            shared: Arc::downgrade(shared),
            runtime,
            notices: self.notices.clone(),
            states: ItemStates::default(),
            lives: Mutex::new(Vec::new()),
            publishing: Mutex::new(()),
            dependents: Mutex::new(Dependents::default()),
        });
        contexts.insert(server.clone(), Arc::downgrade(&context));
        Some(Browse { ctx: context })
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
    timer: Option<AbortHandle>,
    /// Counts the schedules: a timer that fires after a newer schedule leaves the reload to the newer timer.
    generation: u64,
}

/// One session's browse state: its item states and its live lists.
pub(crate) struct Context {
    server: ServerRef,
    /// The session this belongs to ([`Shared::current_session`]).
    session: u64,
    shared: Weak<Shared>,
    runtime: RuntimeHandle,
    notices: Arc<Notices>,
    states: ItemStates,
    lives: Mutex<Vec<Weak<dyn Live>>>,
    /// Serializes publishing an item's state, so the last publish shows the newest state.
    publishing: Mutex<()>,
    dependents: Mutex<Dependents>,
}

impl Drop for Context {
    fn drop(&mut self) {
        if let Some(timer) = lock(&self.dependents).timer.take() {
            timer.abort();
        }
    }
}

impl Context {
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

    fn refresh(&self) {
        for live in self.lives() {
            live.refresh();
        }
    }

    /// Gives each of `items` the state of a change still pending for it.
    fn observe(&self, items: &mut [ItemSummary]) {
        for item in items {
            self.states.observe(item);
        }
    }

    /// `item` as the first live list that shows it does.
    fn find(&self, item: &ItemRef) -> Option<ItemSummary> {
        self.lives().iter().find_map(|live| live.find(item))
    }

    /// Sends `item`'s state to every live list: the pending state if a change is pending, else `settled` (the server's answer).
    /// Read under one lock, so publishes for one item cannot land out of order: the last one shows the newest state.
    fn publish(&self, item: &ItemRef, settled: UserState) {
        let _order = lock(&self.publishing);
        let state = self.states.pending(item).unwrap_or(settled);
        for live in self.lives() {
            live.patch(item, &state);
        }
    }

    /// The lists depending on `items` reload [`DEPENDENT_RELOAD_DELAY`] after the last such call.
    fn dependents_changed(self: &Arc<Self>, items: impl IntoIterator<Item = ItemRef>) {
        let mut dependents = lock(&self.dependents);
        dependents.changed.extend(items);
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
            let changed = {
                let mut dependents = lock(&context.dependents);
                // A newer schedule came while this one woke: its timer reloads, a second after it.
                if dependents.generation != generation {
                    return;
                }
                dependents.timer = None;
                std::mem::take(&mut dependents.changed)
            };
            for live in context.lives() {
                live.dependents_changed(&changed);
            }
        }));
    }

    fn want(self: &Arc<Self>, item: &ItemRef, action: ItemAction) {
        let found = self.find(item);
        let (shown, start) = self.states.want(item, action, found.as_ref());
        self.publish(item, shown);
        if start {
            let context = self.clone();
            let item = item.clone();
            self.spawn(async move { context.send_changes(item).await });
        }
    }

    /// Sends `item`'s wanted changes one at a time until none is left (ADR 0018, section 5).
    async fn send_changes(self: Arc<Self>, item: ItemRef) {
        while let Some(action) = self.states.next(&item) {
            let result = match self.client() {
                Ok(client) => match action {
                    ItemAction::SetPlayed(played) => client.set_played(&item, played).await,
                    ItemAction::SetFavourite(favourite) => {
                        client.set_favourite(&item, favourite).await
                    }
                },
                Err(error) => Err(error),
            };
            let answered = self.states.answered(&item, action, result);
            self.publish(&item, answered.shown);
            match answered.failure {
                Some((name, problem)) => self.notices.raise(NoticeKind::ActionFailed {
                    action,
                    item_name: name,
                    problem,
                }),
                None if matches!(action, ItemAction::SetPlayed(_)) => {
                    self.dependents_changed(answered.parents);
                }
                // A favourite changes no other list's content, only how the item shows.
                None => {}
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

    /// The user's libraries, in the user's order, for the app's navigation.
    ///
    /// # Errors
    /// The session ended ([`AuthError::NotSignedIn`]), or the server's or the network's failure.
    pub async fn libraries(&self) -> Result<Vec<Library>, AuthError> {
        self.ctx.client()?.libraries().await
    }

    /// A grid of `library`'s items: `query` with the library as its parent. Starts loading at once.
    pub fn open_library(&self, library: &Library, query: ItemQuery) -> PagedItems {
        let query = ItemQuery {
            parent: Some(library.item.clone()),
            ..query
        };
        PagedItems::open(self.ctx.clone(), query)
    }

    /// Home's rows. Start loading at once.
    pub fn home(&self) -> HomeFeed {
        HomeFeed::open(self.ctx.clone())
    }

    /// `item`'s detail page. Starts loading at once.
    pub fn details(&self, item: &ItemRef) -> DetailsView {
        DetailsView::open(self.ctx.clone(), item.clone())
    }

    /// The best matches for `term`, one group per kind. Starts loading at once.
    pub fn search(&self, term: &str) -> SearchResults {
        SearchResults::open(self.ctx.clone(), term.to_owned())
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

    /// `item`'s state as the lists show it, if a list loaded it.
    pub fn user_state(&self, item: &ItemRef) -> Option<UserState> {
        self.ctx
            .states
            .pending(item)
            .or_else(|| self.ctx.find(item).map(|summary| summary.user))
    }

    /// Refreshes every live list of this session, as the user asked.
    pub fn refresh(&self) {
        self.ctx.refresh();
    }

    /// Playback of `item` stopped (ADR 0019 calls this): the lists that depend on it ("continue watching", "next up", its series
    /// and season) reload, as after a played change.
    pub fn playback_stopped(&self, item: &ItemRef) {
        let mut items = self
            .ctx
            .find(item)
            .map(|summary| states::parents(&summary))
            .unwrap_or_default();
        items.push(item.clone());
        self.ctx.dependents_changed(items);
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

/// `item`'s summary in `list`, if it holds one.
fn find_in(list: &ObservableList<ItemSummary>, item: &ItemRef) -> Option<ItemSummary> {
    list.snapshot()
        .into_iter()
        .find(|summary| &summary.item == item)
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

#[cfg(test)]
mod tests {
    use super::*;

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
