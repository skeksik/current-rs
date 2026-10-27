//! A detail page (ADR 0018, section 7): the item's details, and its seasons, a season's episodes and similar items as short lists,
//! each loaded in one request.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

use current_jellyfin::AuthError;
use current_model::{ItemDetails, ItemRef, ItemSummary, MediaKind, PlayTarget, UserState};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::{Context, Live, Stamps, below_in, find_in, lock, patch_list, states, sync_list};
use crate::list::ObservableList;

/// How many similar items a detail page shows.
pub const SIMILAR_LIMIT: u32 = 12;

/// Whether a short list or a page is loading, and why it last failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadState {
    pub loading: bool,
    pub failed: Option<AuthError>,
}

struct Details {
    ctx: Arc<Context>,
    item: ItemRef,
    details: watch::Sender<Option<ItemDetails>>,
    /// What the Play button starts: the item, or a series' next episode.
    play: watch::Sender<Option<PlayTarget>>,
    state: watch::Sender<LoadState>,
    seasons: Arc<ObservableList<ItemSummary>>,
    episodes: Arc<ObservableList<ItemSummary>>,
    similar: Arc<ObservableList<ItemSummary>>,
    /// The season whose episodes show; the first season until the user picks one.
    season: Mutex<Option<ItemRef>>,
    /// Guards changes to the lists, and holds when the page's copies were read or patched.
    lists: Mutex<Stamps>,
    task: Mutex<Option<AbortHandle>>,
    me: Weak<Details>,
}

impl Drop for Details {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

/// An item's detail page. Cheap to clone. Dropping every clone stops its requests.
#[derive(Clone)]
pub struct DetailsView {
    inner: Arc<Details>,
}

impl std::fmt::Debug for DetailsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetailsView").finish_non_exhaustive()
    }
}

impl DetailsView {
    pub(super) fn open(ctx: Arc<Context>, item: ItemRef) -> Self {
        let inner = Arc::new_cyclic(|me| Details {
            ctx,
            item,
            details: watch::Sender::new(None),
            play: watch::Sender::new(None),
            state: watch::Sender::new(LoadState::default()),
            seasons: ObservableList::new(),
            episodes: ObservableList::new(),
            similar: ObservableList::new(),
            season: Mutex::new(None),
            lists: Mutex::new(Stamps::default()),
            task: Mutex::new(None),
            me: me.clone(),
        });
        let live: Weak<dyn Live> = Arc::downgrade(&inner) as Weak<Details>;
        inner.ctx.register(live);
        inner.load();
        Self { inner }
    }

    pub fn item(&self) -> &ItemRef {
        &self.inner.item
    }

    /// The details, once loaded.
    pub fn details(&self) -> watch::Receiver<Option<ItemDetails>> {
        self.inner.details.subscribe()
    }

    pub fn state(&self) -> watch::Receiver<LoadState> {
        self.inner.state.subscribe()
    }

    /// What the page's Play button starts, once loaded: the item itself from its resume point ([`PlayTarget::for_item`]), or for a
    /// series its next episode (the server's next up, else its first episode). `None` for an item that does not play.
    pub fn play(&self) -> watch::Receiver<Option<PlayTarget>> {
        self.inner.play.subscribe()
    }

    /// A series' seasons; empty for other kinds.
    pub fn seasons(&self) -> Arc<ObservableList<ItemSummary>> {
        self.inner.seasons.clone()
    }

    /// The episodes of the season picked ([`DetailsView::select_season`]), the first season until then; empty unless a series.
    pub fn episodes(&self) -> Arc<ObservableList<ItemSummary>> {
        self.inner.episodes.clone()
    }

    pub fn similar(&self) -> Arc<ObservableList<ItemSummary>> {
        self.inner.similar.clone()
    }

    /// Shows `season`'s episodes.
    pub fn select_season(&self, season: &ItemRef) {
        *lock(&self.inner.season) = Some(season.clone());
        self.inner.load();
    }

    /// Loads the page again, keeping what it shows until the answers arrive.
    pub fn refresh(&self) {
        self.inner.load();
    }
}

impl Details {
    fn load(&self) {
        let me = self.me.clone();
        let ctx = self.ctx.clone();
        let item = self.item.clone();
        let season = lock(&self.season).clone();
        self.state.send_modify(|state| state.loading = true);
        let at = self.ctx.tick();
        let task = self.ctx.spawn(async move {
            let mut failures = Vec::new();
            match ctx.client() {
                Ok(client) => {
                    let (details, similar) =
                        tokio::join!(client.details(&item), client.similar(&item, SIMILAR_LIMIT));
                    let mut lists = None;
                    // A series' episode to play; `None` when it could not be asked for.
                    let mut next = None;
                    if let Ok(details) = &details
                        && details.summary.kind == MediaKind::SERIES
                    {
                        let episode = async {
                            match client.series_next_up(&item).await {
                                Ok(None) => client.first_episode(&item).await,
                                found => found,
                            }
                        };
                        let seasons = async {
                            let seasons = client.seasons(&item).await?;
                            let picked = season
                                .filter(|season| seasons.iter().any(|s| &s.item == season))
                                .or_else(|| seasons.first().map(|first| first.item.clone()));
                            let episodes = match &picked {
                                Some(picked) => client.episodes(&item, picked).await?,
                                None => Vec::new(),
                            };
                            Ok((seasons, episodes))
                        };
                        let (episode, seasons) = tokio::join!(episode, seasons);
                        // A failed answer leaves the Play button as it is, and does not fail the page: its details loaded, and a
                        // refresh keeps what is shown until answers arrive.
                        next = episode.ok();
                        match seasons {
                            Ok(found) => lists = Some(found),
                            Err(error) => failures.push(error),
                        }
                    }
                    let Some(page) = me.upgrade() else {
                        return;
                    };
                    // Under the guard that patches take, so a patch is never overwritten by an older state.
                    let mut stamps = lock(&page.lists);
                    match details {
                        Ok(mut details) => {
                            page.ctx.states.observe(&mut details.summary);
                            let play = if details.summary.kind == MediaKind::SERIES {
                                next.map(|episode| {
                                    episode
                                        .map(|mut episode| {
                                            page.ctx.states.observe(&mut episode);
                                            episode
                                        })
                                        .as_ref()
                                        .and_then(PlayTarget::for_item)
                                })
                            } else {
                                Some(PlayTarget::for_item(&details.summary))
                            };
                            if let Some(play) = play {
                                page.play.send_replace(play);
                            }
                            stamps.read([&details.summary], at);
                            page.details.send_replace(Some(details));
                        }
                        Err(error) => failures.insert(0, error),
                    }
                    match similar {
                        Ok(mut similar) => {
                            page.ctx.observe(&mut similar);
                            stamps.read(&similar, at);
                            sync_list(&page.similar, similar);
                        }
                        Err(error) => failures.push(error),
                    }
                    if let Some((mut seasons, mut episodes)) = lists {
                        // A season names no series: a played change pending for this one shows on it too.
                        for season in &mut seasons {
                            page.ctx.states.observe_under(season, Some(&page.item));
                        }
                        page.ctx.observe(&mut episodes);
                        stamps.read(seasons.iter().chain(&episodes), at);
                        sync_list(&page.seasons, seasons);
                        sync_list(&page.episodes, episodes);
                    }
                    let shown = page.shown();
                    stamps.retain(|item| shown.contains(item));
                }
                Err(error) => failures.push(error),
            }
            if let Some(page) = me.upgrade() {
                page.state.send_replace(LoadState {
                    loading: false,
                    failed: failures.into_iter().next(),
                });
            }
        });
        if let Some(previous) = lock(&self.task).replace(task) {
            previous.abort();
        }
    }
}

impl Details {
    /// Every item the page shows.
    fn shown(&self) -> HashSet<ItemRef> {
        let details = self
            .details
            .borrow()
            .as_ref()
            .map(|details| details.summary.item.clone());
        [&self.seasons, &self.episodes, &self.similar]
            .into_iter()
            .flat_map(|list| list.snapshot())
            .map(|summary| summary.item)
            .chain(details)
            .collect()
    }
}

impl Live for Details {
    fn patch(&self, item: &ItemRef, state: &UserState, at: u64) {
        let mut stamps = lock(&self.lists);
        stamps.patched(item, at);
        // The Play button follows its item's marks: a new resume point, or played. A series' next episode marked played is no longer
        // its next: the series is among the dependents, whose reload a second later asks for the next one (`dependents_changed`).
        self.play.send_if_modified(|play| match play {
            Some(target) if &target.item.item == item && &target.item.user != state => {
                let mut episode = target.item.clone();
                episode.user = state.clone();
                *play = PlayTarget::for_item(&episode);
                true
            }
            _ => false,
        });
        self.details.send_if_modified(|details| match details {
            Some(details) if &details.summary.item == item && &details.summary.user != state => {
                details.summary.user = state.clone();
                true
            }
            _ => false,
        });
        for list in [&self.seasons, &self.episodes, &self.similar] {
            patch_list(list, item, state);
        }
    }

    fn dependents_changed(&self, items: &HashSet<ItemRef>) {
        // The page's item, a season it lists, or (an episode's page) the episode's series or season: a played mark on a series
        // reaches every episode below it on the server.
        let links = self
            .details
            .borrow()
            .as_ref()
            .map(|details| states::parents(&details.summary))
            .unwrap_or_default();
        let shows = items.contains(&self.item)
            || links.iter().any(|link| items.contains(link))
            || self
                .seasons
                .snapshot()
                .iter()
                .any(|season| items.contains(&season.item));
        if shows {
            self.load();
        }
    }

    fn refresh(&self) {
        self.load();
    }

    fn find(&self, item: &ItemRef) -> Option<(u64, ItemSummary)> {
        let stamps = lock(&self.lists);
        if let Some(details) = self.details.borrow().as_ref()
            && &details.summary.item == item
        {
            return Some((stamps.of(item), details.summary.clone()));
        }
        [&self.seasons, &self.episodes, &self.similar]
            .into_iter()
            .find_map(|list| find_in(list, item))
            .map(|summary| (stamps.of(item), summary))
    }

    fn find_below(&self, parent: &ItemRef) -> Vec<ItemSummary> {
        let _guard = lock(&self.lists);
        let mut below = Vec::new();
        if let Some(details) = self.details.borrow().as_ref()
            && states::parents(&details.summary).contains(parent)
        {
            below.push(details.summary.clone());
        }
        // This series' seasons.
        if parent == &self.item {
            below.extend(self.seasons.snapshot());
        }
        for list in [&self.episodes, &self.similar] {
            below.extend(below_in(list, parent));
        }
        below
    }

    fn parent_of(&self, item: &ItemRef) -> Option<ItemRef> {
        let _guard = lock(&self.lists);
        find_in(&self.seasons, item).map(|_| self.item.clone())
    }
}
