//! Search (ADR 0018): the best matches for a term, one group per kind, each one request; not paged, since the server's search does
//! not page (`docs/notes/jellyfin-browse.md`). The Favourites tab shows its rows the same way (owner, 2026-10-27): one group per kind
//! of [`FAVOURITE_ROW_KINDS`], each the first items of [`super::favourites_query`] for that kind, read again when a mark changes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use current_jellyfin::{AuthError, JellyfinClient, SearchGroup};
use current_model::{ItemRef, ItemSummary, MediaKind, UserState};
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinSet};

use super::search_filter::{SearchFilter, library_groups};
use super::{Context, Live, LoadState, Stamps, below_in, find_in, lock, patch_list, sync_list};
use crate::list::ObservableList;

/// The Favourites tab's rows, in order (owner, 2026-10-27): films, series and episodes. Each row's header opens the whole list of its
/// kind, [`super::favourites_query`] with that kind.
pub const FAVOURITE_ROW_KINDS: [MediaKind; 3] =
    [MediaKind::MOVIE, MediaKind::SERIES, MediaKind::EPISODE];

/// How many items each Favourites row shows: as many as a search group ([`SEARCH_LIMIT`]), as the tab lays its rows out as the
/// search page does.
pub const FAVOURITE_ROW_LIMIT: u32 = current_jellyfin::SEARCH_LIMIT;

/// What the groups are.
enum Source {
    /// The best matches for a term, of the types and sources a filter in effect keeps ([`super::search_filter`]).
    Term(String, SearchFilter),
    /// The user's favourites, a row per kind of [`FAVOURITE_ROW_KINDS`].
    Favourites,
}

impl Source {
    async fn load(&self, client: Arc<JellyfinClient>) -> Result<Vec<SearchGroup>, AuthError> {
        match self {
            Self::Term(term, filter) => match library_groups(filter) {
                Some((kinds, limit)) => client.search(term, &kinds, limit).await,
                // The catalog only: nothing from the library (the catalog's own results come with Seerr, M4).
                None => Ok(Vec::new()),
            },
            Self::Favourites => favourite_rows(client).await,
        }
    }
}

/// The first items of each Favourites row, read together; a row without items is left out, and one failed row fails them all, as a
/// search group does.
async fn favourite_rows(client: Arc<JellyfinClient>) -> Result<Vec<SearchGroup>, AuthError> {
    let mut rows = JoinSet::new();
    for (index, kind) in FAVOURITE_ROW_KINDS.into_iter().enumerate() {
        let client = client.clone();
        rows.spawn(async move {
            let query = super::favourites_query(Some(kind.clone()));
            let page = client.items(&query, 0, FAVOURITE_ROW_LIMIT, false).await;
            (index, kind, page)
        });
    }
    let mut found = Vec::with_capacity(FAVOURITE_ROW_KINDS.len());
    while let Some(joined) = rows.join_next().await {
        // A row's task fails to join only if it panicked: its row is left out.
        let Ok((index, kind, page)) = joined else {
            continue;
        };
        let items = page?.items;
        if !items.is_empty() {
            found.push((index, SearchGroup { kind, items }));
        }
    }
    found.sort_by_key(|(index, _)| *index);
    Ok(found.into_iter().map(|(_, group)| group).collect())
}

struct Groups {
    lists: HashMap<String, Arc<ObservableList<ItemSummary>>>,
    /// When the groups' copies were read or patched: every group is read at once.
    stamps: Stamps,
}

struct Search {
    ctx: Arc<Context>,
    source: Arc<Source>,
    /// The kinds with matches, in the order shown.
    groups: Arc<ObservableList<MediaKind>>,
    /// Guards every change to `groups` and the group lists.
    lists: Mutex<Groups>,
    state: watch::Sender<LoadState>,
    task: Mutex<Option<AbortHandle>>,
    me: Weak<Search>,
}

impl Drop for Search {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

/// The results of one search, or the Favourites tab's rows ([`SearchResults::groups`] then holds the kinds of
/// [`FAVOURITE_ROW_KINDS`] that have favourites). Cheap to clone. Dropping every clone stops its requests.
#[derive(Clone)]
pub struct SearchResults {
    inner: Arc<Search>,
}

impl std::fmt::Debug for SearchResults {
    /// Leaves out the term: what the user typed stays out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchResults").finish_non_exhaustive()
    }
}

impl SearchResults {
    pub(super) fn open(ctx: Arc<Context>, term: String, filter: SearchFilter) -> Self {
        Self::start(ctx, Source::Term(term, filter))
    }

    /// The Favourites tab's rows: they read again when a played or favourite mark changes, as an item can join or leave them.
    pub(super) fn favourites(ctx: Arc<Context>) -> Self {
        Self::start(ctx, Source::Favourites)
    }

    fn start(ctx: Arc<Context>, source: Source) -> Self {
        let inner = Arc::new_cyclic(|me| Search {
            ctx,
            source: Arc::new(source),
            groups: ObservableList::new(),
            lists: Mutex::new(Groups {
                lists: HashMap::new(),
                stamps: Stamps::default(),
            }),
            state: watch::Sender::new(LoadState::default()),
            task: Mutex::new(None),
            me: me.clone(),
        });
        let live: Weak<dyn Live> = Arc::downgrade(&inner) as Weak<Search>;
        inner.ctx.register(live);
        inner.load();
        Self { inner }
    }

    /// The kinds with matches, in order.
    pub fn groups(&self) -> Arc<ObservableList<MediaKind>> {
        self.inner.groups.clone()
    }

    /// The matches of `kind`, while it has any. Its list is ready before the kind appears in [`SearchResults::groups`].
    pub fn group(&self, kind: &MediaKind) -> Option<Arc<ObservableList<ItemSummary>>> {
        lock(&self.inner.lists).lists.get(kind.as_str()).cloned()
    }

    pub fn state(&self) -> watch::Receiver<LoadState> {
        self.inner.state.subscribe()
    }

    pub fn refresh(&self) {
        self.inner.load();
    }
}

impl Search {
    fn load(&self) {
        let me = self.me.clone();
        let ctx = self.ctx.clone();
        let source = self.source.clone();
        self.state.send_modify(|state| state.loading = true);
        let at = self.ctx.tick();
        let task = self.ctx.spawn(async move {
            let result = match ctx.client() {
                Ok(client) => source.load(client).await,
                Err(error) => Err(error),
            };
            let Some(search) = me.upgrade() else {
                return;
            };
            let failed = match result {
                Ok(found) => {
                    let mut groups = lock(&search.lists);
                    let mut kinds = Vec::new();
                    let mut lists = HashMap::new();
                    let mut stamps = Stamps::default();
                    for mut group in found {
                        ctx.observe(&mut group.items);
                        stamps.read(&group.items, at);
                        let list = groups
                            .lists
                            .remove(group.kind.as_str())
                            .unwrap_or_else(ObservableList::new);
                        sync_list(&list, group.items);
                        lists.insert(group.kind.as_str().to_owned(), list);
                        kinds.push(group.kind);
                    }
                    groups.lists = lists;
                    groups.stamps = stamps;
                    sync_list(&search.groups, kinds);
                    None
                }
                Err(error) => Some(error),
            };
            search.state.send_replace(LoadState {
                loading: false,
                failed,
            });
        });
        if let Some(previous) = lock(&self.task).replace(task) {
            previous.abort();
        }
    }
}

impl Live for Search {
    fn patch(&self, item: &ItemRef, state: &UserState, at: u64) {
        let mut groups = lock(&self.lists);
        groups.stamps.patched(item, at);
        for list in groups.lists.values() {
            patch_list(list, item, state);
        }
    }

    fn dependents_changed(&self, _items: &HashSet<ItemRef>) {
        // The Favourites rows are chosen by a mark: any played or favourite change can add an item to them or take one out. A search's
        // matches do not change with marks; the marks they show are patched.
        if matches!(*self.source, Source::Favourites) {
            self.load();
        }
    }

    fn refresh(&self) {
        self.load();
    }

    fn find(&self, item: &ItemRef) -> Option<(u64, ItemSummary)> {
        let groups = lock(&self.lists);
        groups
            .lists
            .values()
            .find_map(|list| find_in(list, item))
            .map(|summary| (groups.stamps.of(item), summary))
    }
    fn find_below(&self, parent: &ItemRef) -> Vec<ItemSummary> {
        let groups = lock(&self.lists);
        groups
            .lists
            .values()
            .flat_map(|list| below_in(list, parent))
            .collect()
    }
}
