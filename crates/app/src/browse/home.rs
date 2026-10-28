//! Home (ADR 0018, section 6): rows, each with its own list. Home's layout is provisional and will be redesigned; the shape (a list
//! of rows, a list of items per row) is meant to stay.
//!
//! Rows, in order: Continue watching, Next up, Favourites and Recently added across every library, then Latest in each library in
//! the user's order, skipping the libraries the user keeps out of "latest" and the kinds without a meaningful "latest", then Seen
//! recently (owner, 2026-10-26), as the default the user arranges: rows reordered and hidden in Current (ADR 0024), a hidden row not
//! asked for. A row exists only while it has items, so no row assumes a
//! library type exists. Rows load in parallel and each appears in its place as soon as it has items. A row that fails keeps what
//! it showed from the last load (none the first time) and the state carries one failure for a Retry message.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use current_jellyfin::{AuthError, JellyfinClient, NEXT_UP_CUTOFF};
use current_model::{CollectionKind, ItemQuery, ItemRef, ItemSummary, Library, UserState};
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinSet};

use super::store::ReadTicket;
use super::{Context, Live, lock, patch_list, sync_list};
use crate::list::ObservableList;

/// How many items each row asks for (what jellyfin-web asks; `docs/notes/jellyfin-browse.md`).
pub const RESUME_LIMIT: u32 = 12;
pub const NEXT_UP_LIMIT: u32 = 24;
pub const LATEST_LIMIT: u32 = 16;
/// The rows across every library ask for as many as a library's Latest row.
pub const VIEW_ROW_LIMIT: u32 = 16;

/// Collection kinds without a meaningful "latest": the open values Current names, and the Jellyfin collection types jellyfin-web
/// also skips (`docs/notes/jellyfin-browse.md`, "Latest per library").
const NO_LATEST: &[&str] = &["livetv", "channels", "folders"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeRowKind {
    ContinueWatching,
    NextUp,
    Latest,
    /// What was added to every library, the newest first ([`super::recently_added_everywhere_query`]).
    RecentlyAdded,
    /// The user's favourites in every library ([`super::favourites_query`]).
    Favourites,
    /// What the user finished last ([`super::seen_recently_query`]).
    SeenRecently,
}

/// One row of Home. Debug leaves out the title, a library's name.
#[derive(Clone, PartialEq, Eq)]
pub struct HomeRow {
    /// Stable across loads: the key for [`HomeFeed::row`].
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeState {
    pub loading: bool,
    /// Why a row failed in the last load: one message with Retry.
    pub failed: Option<AuthError>,
    /// The server could not be reached: the rows are the last ones loaded.
    pub offline: bool,
}

#[derive(Clone)]
pub(super) enum Spec {
    Resume,
    NextUp,
    RecentlyAdded,
    Favourites,
    Latest(Library),
    SeenRecently,
}

impl Spec {
    pub(super) fn key(&self) -> String {
        match self {
            Self::Resume => "resume".to_owned(),
            Self::NextUp => "next_up".to_owned(),
            Self::RecentlyAdded => "recently_added".to_owned(),
            Self::Favourites => "favourites".to_owned(),
            Self::SeenRecently => "seen_recently".to_owned(),
            Self::Latest(library) => format!("latest:{}", library.item.id.as_str()),
        }
    }

    pub(super) fn row(&self) -> HomeRow {
        let (kind, title, library) = match self {
            Self::Resume => (HomeRowKind::ContinueWatching, String::new(), None),
            Self::NextUp => (HomeRowKind::NextUp, String::new(), None),
            Self::RecentlyAdded => (HomeRowKind::RecentlyAdded, String::new(), None),
            Self::Favourites => (HomeRowKind::Favourites, String::new(), None),
            Self::SeenRecently => (HomeRowKind::SeenRecently, String::new(), None),
            Self::Latest(library) => (
                HomeRowKind::Latest,
                library.name.clone(),
                Some(library.item.clone()),
            ),
        };
        HomeRow {
            key: self.key(),
            kind,
            title,
            library,
        }
    }

    async fn load(&self, client: &JellyfinClient) -> Result<Vec<ItemSummary>, AuthError> {
        match self {
            Self::Resume => client.resume_items(RESUME_LIMIT).await,
            Self::NextUp => {
                let cutoff = SystemTime::now()
                    .checked_sub(NEXT_UP_CUTOFF)
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                client.next_up(NEXT_UP_LIMIT, cutoff).await
            }
            Self::Latest(library) => client.latest(&library.item, LATEST_LIMIT).await,
            Self::RecentlyAdded => {
                view(client, &super::recently_added_everywhere_query(None)).await
            }
            Self::Favourites => view(client, &super::favourites_query(None)).await,
            Self::SeenRecently => view(client, &super::seen_recently_query()).await,
        }
    }
}

/// The first items of a view across every library, for its row.
async fn view(client: &JellyfinClient, query: &ItemQuery) -> Result<Vec<ItemSummary>, AuthError> {
    Ok(client.items(query, 0, VIEW_ROW_LIMIT, false).await?.items)
}

/// The rows that follow the user's played and favourite marks, loaded again when they change ([`Live::dependents_changed`]).
const DEPENDENT: [Spec; 4] = [
    Spec::Resume,
    Spec::NextUp,
    Spec::Favourites,
    Spec::SeenRecently,
];

/// Home's rows in their default order (owner, 2026-10-26): Continue watching, Next up, Favourites, Recently added, a Latest row for
/// each of `libraries` (in that order) that has one, Seen recently.
pub(super) fn specs(libraries: &[Library], excludes: &[ItemRef]) -> Vec<Spec> {
    let mut specs = vec![
        Spec::Resume,
        Spec::NextUp,
        Spec::Favourites,
        Spec::RecentlyAdded,
    ];
    specs.extend(
        libraries
            .iter()
            .filter(|library| has_latest(library, excludes))
            .cloned()
            .map(Spec::Latest),
    );
    specs.push(Spec::SeenRecently);
    specs
}

/// Whether `library` gets a Latest row.
fn has_latest(library: &Library, excludes: &[ItemRef]) -> bool {
    if excludes.contains(&library.item) {
        return false;
    }
    library.collection.as_ref().is_none_or(|kind| {
        *kind != CollectionKind::PLAYLISTS
            && *kind != CollectionKind::BOX_SETS
            && !NO_LATEST.contains(&kind.as_str())
    })
}

struct Rows {
    /// Every row's key in Home's order, from the last load of the libraries.
    order: Vec<String>,
    lists: HashMap<String, Arc<ObservableList<ItemSummary>>>,
}

/// One kind of load: every row, or only the rows that depend on played marks.
#[derive(Default)]
struct Load {
    task: Option<AbortHandle>,
    /// Tells the current load of this kind apart from one it replaced.
    generation: u64,
    running: bool,
    /// Why its last load failed, if it did.
    failed: Option<AuthError>,
}

/// Each kind keeps its own outcome, so one finishing does not hide the other's failure or that it still runs.
#[derive(Default)]
struct Loads {
    full: Load,
    dependent: Load,
}

impl Loads {
    fn kind(&mut self, full: bool) -> &mut Load {
        if full {
            &mut self.full
        } else {
            &mut self.dependent
        }
    }
}

struct Home {
    ctx: Arc<Context>,
    rows: Arc<ObservableList<HomeRow>>,
    /// Guards every change to `rows` and the row lists.
    lists: Mutex<Rows>,
    state: watch::Sender<HomeState>,
    loads: Mutex<Loads>,
    /// The task that follows the user's arrangement of the rows.
    follower: Mutex<Option<AbortHandle>>,
    me: Weak<Home>,
}

impl Drop for Home {
    fn drop(&mut self) {
        if let Some(follower) = lock(&self.follower).take() {
            follower.abort();
        }
        let loads = lock(&self.loads);
        for task in loads.full.task.iter().chain(&loads.dependent.task) {
            task.abort();
        }
    }
}

/// Home's rows. Cheap to clone: clones share the rows. Dropping every clone stops its requests.
#[derive(Clone)]
pub struct HomeFeed {
    inner: Arc<Home>,
    shown: Arc<super::ShownAgain>,
}

impl std::fmt::Debug for HomeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeFeed").finish_non_exhaustive()
    }
}

impl HomeFeed {
    pub(super) fn open(ctx: Arc<Context>) -> Self {
        let order = shown_keys(&ctx.arranged_home(&ctx.known_libraries()));
        let arrangement = ctx
            .shared
            .upgrade()
            .map(|shared| shared.navigation.subscribe());
        let inner = Arc::new_cyclic(|me| Home {
            ctx,
            rows: ObservableList::new(),
            lists: Mutex::new(Rows {
                order,
                lists: HashMap::new(),
            }),
            state: watch::Sender::new(HomeState::default()),
            loads: Mutex::new(Loads::default()),
            follower: Mutex::new(None),
            me: me.clone(),
        });
        let live: Weak<dyn Live> = Arc::downgrade(&inner) as Weak<Home>;
        inner.ctx.register(live);
        inner.load(true);
        if let Some(arrangement) = arrangement {
            let follower = inner
                .ctx
                .spawn(follow_arrangement(Arc::downgrade(&inner), arrangement));
            *lock(&inner.follower) = Some(follower);
        }
        Self {
            inner,
            shown: Arc::default(),
        }
    }

    /// Home was left (a UI calls it when it navigates away).
    pub fn hidden(&self) {
        self.shown.hidden();
    }

    /// Home is shown again: loads every row again, keeping what they show, if it was left five minutes or more ago (ADR 0018,
    /// section 3: [`super::REFRESH_WHEN_SHOWN_AFTER`]); else the marks of every item the session's lists hold are read again
    /// (section 5, amended 2026-10-27 by the owner). Returns whether it refreshed.
    pub fn shown(&self) -> bool {
        let stale = self.shown.shown();
        if stale {
            self.refresh();
        } else {
            // Within the five minutes: what Home shows reads its marks again, which another device may have changed meanwhile.
            let mut held = HashSet::new();
            self.inner.holds(&mut held);
            self.inner
                .ctx
                .resync(held.into_iter().take(super::RESYNC_LIMIT));
        }
        stale
    }

    /// The rows, in order.
    pub fn rows(&self) -> Arc<ObservableList<HomeRow>> {
        self.inner.rows.clone()
    }

    /// The items of the row with `key`, while it exists. Its list is ready before the row appears in [`HomeFeed::rows`].
    pub fn row(&self, key: &str) -> Option<Arc<ObservableList<ItemSummary>>> {
        lock(&self.inner.lists).lists.get(key).cloned()
    }

    pub fn state(&self) -> watch::Receiver<HomeState> {
        self.inner.state.subscribe()
    }

    /// Loads every row again, keeping what they show until the answers arrive.
    pub fn refresh(&self) {
        self.inner.load(true);
    }

    /// The Retry button: the same as [`HomeFeed::refresh`].
    pub fn retry(&self) {
        self.inner.load(true);
    }
}

impl Home {
    /// Loads every row (`full`) or only the rows that depend on played marks.
    fn load(&self, full: bool) {
        let me = self.me.clone();
        let ctx = self.ctx.clone();
        let generation = {
            let mut loads = lock(&self.loads);
            let load = loads.kind(full);
            if let Some(previous) = load.task.take() {
                previous.abort();
            }
            load.generation += 1;
            load.running = true;
            self.publish_state(&loads);
            load_generation(&loads, full)
        };
        let task = self.ctx.spawn(async move {
            let mut failures = Vec::new();
            let mut order = None;
            match ctx.client() {
                Ok(client) => {
                    let mut rows = JoinSet::new();
                    // The rows the user hid are not asked for (ADR 0024).
                    let shown_now: Vec<String> =
                        shown_keys(&ctx.arranged_home(&ctx.known_libraries()));
                    for spec in DEPENDENT {
                        if shown_now.contains(&spec.key()) {
                            spawn_row(&mut rows, &ctx, &client, spec);
                        }
                    }
                    if full {
                        if shown_now.contains(&Spec::RecentlyAdded.key()) {
                            spawn_row(&mut rows, &ctx, &client, Spec::RecentlyAdded);
                        }
                        // The navigation's list takes them too, so the navigation follows what Home shows.
                        match ctx.libraries_joined().await {
                            Ok(libraries) => {
                                let arranged = ctx.arranged_home(&libraries);
                                for (spec, hidden) in &arranged {
                                    if !hidden && matches!(spec, Spec::Latest(_)) {
                                        spawn_row(&mut rows, &ctx, &client, spec.clone());
                                    }
                                }
                                let keys = shown_keys(&arranged);
                                // Rows that answered before, by a load that read the order without the libraries, move into it.
                                if let Some(home) = me.upgrade() {
                                    home.take_order(keys.clone());
                                }
                                order = Some(keys);
                            }
                            Err(error) => failures.push(error),
                        }
                    }
                    while let Some(joined) = rows.join_next().await {
                        let Ok((spec, read, result)) = joined else {
                            continue;
                        };
                        let Some(home) = me.upgrade() else {
                            return;
                        };
                        match result {
                            Ok(items) => home.apply(&spec, items, read.at),
                            Err(error) => failures.push(error),
                        }
                    }
                }
                Err(error) => failures.push(error),
            }
            if let Some(home) = me.upgrade() {
                // The order as it is now: the user may have hidden a row while this load ran.
                if order.is_some() {
                    let now = lock(&home.lists).order.clone();
                    home.keep_only(&now);
                }
                home.finished(full, generation, failures.into_iter().next());
            }
        });
        let mut loads = lock(&self.loads);
        if load_generation(&loads, full) == generation {
            loads.kind(full).task = Some(task);
        }
    }

    /// A load of one kind ended; a load it was replaced by keeps the state.
    fn finished(&self, full: bool, generation: u64, failed: Option<AuthError>) {
        let mut loads = lock(&self.loads);
        if load_generation(&loads, full) != generation {
            return;
        }
        let succeeded = failed.is_none();
        let load = loads.kind(full);
        load.running = false;
        load.task = None;
        load.failed = failed;
        // A full load reads the dependent rows too.
        if full && succeeded && !loads.dependent.running {
            loads.dependent.failed = None;
        }
        self.publish_state(&loads);
    }

    fn publish_state(&self, loads: &Loads) {
        let state = HomeState {
            loading: loads.full.running || loads.dependent.running,
            failed: loads
                .full
                .failed
                .clone()
                .or_else(|| loads.dependent.failed.clone()),
            offline: self.ctx.is_unreachable(),
        };
        self.state.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
    }

    /// A row answered `items`, read at `read`: shown in its place, or gone without items.
    fn apply(&self, spec: &Spec, mut items: Vec<ItemSummary>, read: u64) {
        let key = spec.key();
        let mut rows = lock(&self.lists);
        // Asked for before the user hid it, or its library left: not shown.
        if !rows.order.contains(&key) {
            rows.lists.remove(&key);
            return;
        }
        let at = self.rows.snapshot().iter().position(|row| row.key == key);
        if items.is_empty() {
            rows.lists.remove(&key);
            if let Some(at) = at {
                let _ = self.rows.remove(at);
            }
            return;
        }
        let changed = self.ctx.take(&mut items, read, None);
        let list = rows
            .lists
            .entry(key.clone())
            .or_insert_with(ObservableList::new)
            .clone();
        sync_list(&list, items);
        let row = spec.row();
        match at {
            Some(at) => {
                if self.rows.get(at).as_ref() != Some(&row) {
                    let _ = self.rows.set(at, row);
                }
            }
            None => {
                let rank = |key: &str| {
                    rows.order
                        .iter()
                        .position(|known| known == key)
                        .unwrap_or(usize::MAX)
                };
                let mine = rank(&key);
                let at = self
                    .rows
                    .snapshot()
                    .iter()
                    .filter(|row| rank(&row.key) < mine)
                    .count();
                let _ = self.rows.insert(at, row);
            }
        }
        // Outside the lock: every list, this one too, is patched.
        drop(rows);
        self.ctx.spread(changed);
    }

    /// The user's arrangement changed: the rows take its order, hidden rows go, and a row shown again loads.
    fn rearrange(&self) {
        let order = shown_keys(&self.ctx.arranged_home(&self.ctx.known_libraries()));
        // A row shown again: read every row, keeping what they show.
        if self.take_order(order) {
            self.load(true);
        }
    }

    /// Takes `order` as Home's: the rows not in it go, the rest move into it. Returns whether it shows a row the last order did not.
    fn take_order(&self, order: Vec<String>) -> bool {
        let shown_again = {
            let mut rows = lock(&self.lists);
            let again = order.iter().any(|key| !rows.order.contains(key));
            rows.order.clone_from(&order);
            again
        };
        self.keep_only(&order);
        {
            let _rows = lock(&self.lists);
            let mut shown = self.rows.snapshot();
            let mut want = shown.clone();
            want.sort_by_key(|row| order.iter().position(|key| *key == row.key));
            for (at, row) in want.into_iter().enumerate() {
                if shown[at].key == row.key {
                    continue;
                }
                if let Some(from) = shown.iter().position(|known| known.key == row.key) {
                    let _ = self.rows.remove(from);
                    shown.remove(from);
                    let _ = self.rows.insert(at, row.clone());
                    shown.insert(at, row);
                }
            }
        }
        shown_again
    }

    /// Removes the rows of libraries that are gone or now excluded, and the rows the user hid.
    fn keep_only(&self, order: &[String]) {
        let mut rows = lock(&self.lists);
        let snapshot = self.rows.snapshot();
        for (at, row) in snapshot.iter().enumerate().rev() {
            if !order.contains(&row.key) {
                rows.lists.remove(&row.key);
                let _ = self.rows.remove(at);
            }
        }
    }
}

/// The keys of the rows that show, in order.
fn shown_keys(arranged: &[(Spec, bool)]) -> Vec<String> {
    arranged
        .iter()
        .filter(|(_, hidden)| !hidden)
        .map(|(spec, _)| spec.key())
        .collect()
}

/// Rearranges Home each time the user saves an arrangement, until Home is gone.
async fn follow_arrangement(home: Weak<Home>, mut arrangement: watch::Receiver<u64>) {
    arrangement.borrow_and_update();
    while arrangement.changed().await.is_ok() {
        let Some(home) = home.upgrade() else {
            return;
        };
        home.rearrange();
    }
}

fn load_generation(loads: &Loads, full: bool) -> u64 {
    if full {
        loads.full.generation
    } else {
        loads.dependent.generation
    }
}

/// A row's answer, and its read ([`Context::begin_read`]): when its request went out.
type RowAnswer = (Spec, ReadTicket, Result<Vec<ItemSummary>, AuthError>);

fn spawn_row(
    rows: &mut JoinSet<RowAnswer>,
    ctx: &Context,
    client: &Arc<JellyfinClient>,
    spec: Spec,
) {
    let client = client.clone();
    let read = ctx.begin_read();
    rows.spawn(async move {
        let result = spec.load(&client).await;
        (spec, read, result)
    });
}

impl Live for Home {
    fn patch(&self, item: &ItemRef, state: &UserState) {
        let rows = lock(&self.lists);
        for list in rows.lists.values() {
            patch_list(list, item, state);
        }
    }

    fn dependents_changed(&self, _items: &HashSet<ItemRef>) {
        // "Continue watching", "next up", Favourites and Seen recently depend on every played or favourite mark.
        self.load(false);
    }

    fn refresh(&self) {
        self.load(true);
    }

    fn holds(&self, held: &mut HashSet<ItemRef>) {
        let rows = lock(&self.lists);
        for list in rows.lists.values() {
            held.extend(list.snapshot().into_iter().map(|summary| summary.item));
        }
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ItemId, ServerKey, ServerRef, ServiceType};

    use super::*;

    fn library(id: &str, collection: Option<CollectionKind>) -> Library {
        Library {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new(id),
            ),
            name: "Example Library".into(),
            collection,
            artwork: Vec::new(),
        }
    }

    #[test]
    fn latest_skips_excluded_libraries_and_kinds_without_latest() {
        let films = library("a", Some(CollectionKind::MOVIES));
        assert!(has_latest(&films, &[]));
        assert!(!has_latest(&films, std::slice::from_ref(&films.item)));
        assert!(!has_latest(
            &library("b", Some(CollectionKind::PLAYLISTS)),
            &[]
        ));
        assert!(!has_latest(
            &library("c", Some(CollectionKind::BOX_SETS)),
            &[]
        ));
        assert!(!has_latest(
            &library("d", Some(CollectionKind::new("livetv"))),
            &[]
        ));
        // Mixed content and kinds Current does not know keep their row.
        assert!(has_latest(&library("e", None), &[]));
        assert!(has_latest(
            &library("f", Some(CollectionKind::new("Future"))),
            &[]
        ));
    }
}
