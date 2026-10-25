//! Search (ADR 0018): the best matches for a term, one group per kind, each one request; not paged, since the server's search does
//! not page (`docs/notes/jellyfin-browse.md`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use current_jellyfin::{SEARCH_GROUPS, SEARCH_LIMIT};
use current_model::{ItemRef, ItemSummary, MediaKind, UserState};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::{Context, Live, LoadState, lock, patch_list, sync_list};
use crate::list::ObservableList;

struct Groups {
    lists: HashMap<String, Arc<ObservableList<ItemSummary>>>,
}

struct Search {
    ctx: Arc<Context>,
    term: String,
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

/// The results of one search. Cheap to clone. Dropping every clone stops its requests.
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
    pub(super) fn open(ctx: Arc<Context>, term: String) -> Self {
        let inner = Arc::new_cyclic(|me| Search {
            ctx,
            term,
            groups: ObservableList::new(),
            lists: Mutex::new(Groups {
                lists: HashMap::new(),
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
        let term = self.term.clone();
        self.state.send_modify(|state| state.loading = true);
        let task = self.ctx.spawn(async move {
            let result = match ctx.client() {
                Ok(client) => client.search(&term, SEARCH_GROUPS, SEARCH_LIMIT).await,
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
                    for mut group in found {
                        ctx.observe(&mut group.items);
                        let list = groups
                            .lists
                            .remove(group.kind.as_str())
                            .unwrap_or_else(ObservableList::new);
                        sync_list(&list, group.items);
                        lists.insert(group.kind.as_str().to_owned(), list);
                        kinds.push(group.kind);
                    }
                    groups.lists = lists;
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
    fn patch(&self, item: &ItemRef, state: &UserState) {
        let groups = lock(&self.lists);
        for list in groups.lists.values() {
            patch_list(list, item, state);
        }
    }

    fn dependents_changed(&self, _items: &HashSet<ItemRef>) {}

    fn refresh(&self) {
        self.load();
    }
}
