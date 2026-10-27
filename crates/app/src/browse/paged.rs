//! A library grid as a paged list (ADR 0018, section 3): a list of slots sized to the query's total, filled a page at a time around
//! what the UI shows.
//!
//! The first request reads page 0 with the total; the list is then reset once to `total` slots. From then on the UI reports the
//! visible range and the pages that cover it, plus one page either side, are loaded: at most [`MAX_PAGE_REQUESTS`] at a time, the
//! visible pages first, and requests for pages that left the window are cancelled. A loaded page fills its slots with `Set` diffs
//! in one batch. A refresh never resets: loaded pages keep their items until the new answers arrive, a changed total removes or
//! appends slots at the end, and the pages that are not visible reload when they next are. Only a new query resets.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use current_jellyfin::{AuthError, Page as Answer};
use current_model::{ItemQuery, ItemRef, ItemSummary, UserState};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::libraries::might_be_gone;
use super::{Context, Live, lock, states};
use crate::list::{ObservableList, VectorDiff};

/// How many items one request reads (what jellyfin-web uses; `docs/notes/jellyfin-browse.md`).
pub const PAGE_SIZE: usize = 100;
/// How many page requests one list sends at a time.
pub const MAX_PAGE_REQUESTS: usize = 2;
/// How many pages of diffs a subscriber may fall behind before it is reset (a reset costs the UI its scroll position).
pub const BACKLOG_PAGES: usize = 8;

/// One place in a grid. Never null across the bridge: WinUI 3 does not take null placeholders.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Loading,
    /// Boxed, so the slots still loading take little room in a long list.
    Item(Box<ItemSummary>),
    /// Its page could not be loaded: the tile shows a retry glyph.
    Failed,
}

/// What the message above a grid shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PagedState {
    /// How many items the query holds; 0 until the first page answered.
    pub total: u32,
    /// Whether requests are in flight.
    pub loading: bool,
    /// Why the last page or refresh failed, while a page is failed or until a later request succeeds: one message with Retry.
    pub failed: Option<AuthError>,
}

/// The pages to load for a list of `total` items of which `visible` (first and last index, either order) are shown, most wanted
/// first: the visible pages in order, then the page before and the page after. A range past the end (the total shrank) keeps to
/// the last page.
pub(crate) fn plan(total: usize, visible: Option<(usize, usize)>) -> Vec<usize> {
    let count = total.div_ceil(PAGE_SIZE);
    if count == 0 {
        return Vec::new();
    }
    let (a, b) = visible.unwrap_or((0, 0));
    let last_page = count - 1;
    let first = (a.min(b) / PAGE_SIZE).min(last_page);
    let last = (a.max(b) / PAGE_SIZE).min(last_page);
    let mut pages: Vec<usize> = (first..=last).collect();
    if first > 0 {
        pages.push(first - 1);
    }
    if last < last_page {
        pages.push(last + 1);
    }
    pages
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    /// Never loaded: its slots are `Loading`.
    Empty,
    /// `stale` after a refresh, until it is read again; its items show meanwhile.
    Loaded { stale: bool },
    /// Loaded, but reading it again failed: its items still show, and it waits for Retry or for coming into view again.
    ReloadFailed,
    /// Its slots are `Failed`. `retried` once a scroll retried it by itself.
    Failed { retried: bool },
    /// Failed, and to be loaded again now; `auto` when a scroll asked rather than Retry.
    Retry { auto: bool },
}

struct Flight {
    id: u64,
    task: AbortHandle,
    with_total: bool,
    auto: bool,
}

struct Control {
    query: ItemQuery,
    /// Whether the first page answered and the list holds `total` slots.
    started: bool,
    total: usize,
    pages: Vec<Page>,
    visible: Option<(usize, usize)>,
    flights: HashMap<usize, Flight>,
    /// A refresh waits to read the total.
    refresh_wanted: bool,
    error: Option<AuthError>,
    /// The slots of each loaded item, for played and favourite patches.
    index: HashMap<ItemRef, Vec<usize>>,
    next_id: u64,
}

impl Control {
    fn new(query: ItemQuery) -> Self {
        Self {
            query,
            started: false,
            total: 0,
            pages: Vec::new(),
            visible: None,
            flights: HashMap::new(),
            refresh_wanted: false,
            error: None,
            index: HashMap::new(),
            next_id: 0,
        }
    }

    fn cancel_all(&mut self) {
        for (_, flight) in self.flights.drain() {
            flight.task.abort();
        }
    }
}

struct Paged {
    ctx: Arc<Context>,
    /// The library a library grid shows: a request that fails as one does once the library is gone reads the libraries again.
    library: Option<ItemRef>,
    slots: Arc<ObservableList<Slot>>,
    state: watch::Sender<PagedState>,
    control: Mutex<Control>,
    me: Weak<Paged>,
}

impl Drop for Paged {
    fn drop(&mut self) {
        lock(&self.control).cancel_all();
    }
}

/// A grid of a query's items. Cheap to clone: clones share the list. Dropping every clone stops its requests.
#[derive(Clone)]
pub struct PagedItems {
    inner: Arc<Paged>,
    shown: Arc<super::ShownAgain>,
}

impl std::fmt::Debug for PagedItems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PagedItems")
            .field("state", &*self.inner.state.borrow())
            .finish_non_exhaustive()
    }
}

impl PagedItems {
    pub(super) fn open(ctx: Arc<Context>, query: ItemQuery, library: Option<ItemRef>) -> Self {
        let inner = Arc::new_cyclic(|me| Paged {
            ctx,
            library,
            slots: ObservableList::with_backlog(BACKLOG_PAGES * PAGE_SIZE),
            state: watch::Sender::new(PagedState {
                loading: true,
                ..PagedState::default()
            }),
            control: Mutex::new(Control::new(query)),
            me: me.clone(),
        });
        let live: Weak<dyn Live> = Arc::downgrade(&inner) as Weak<Paged>;
        inner.ctx.register(live);
        inner.pump(&mut lock(&inner.control));
        Self {
            inner,
            shown: Arc::default(),
        }
    }

    /// The page showing the grid was left (a UI calls it when it navigates away).
    pub fn hidden(&self) {
        self.shown.hidden();
    }

    /// The page showing the grid is shown again: refreshes, without a reset, if it was left five minutes or more ago (ADR 0018,
    /// section 3: [`super::REFRESH_WHEN_SHOWN_AFTER`]). Returns whether it refreshed.
    pub fn shown(&self) -> bool {
        let stale = self.shown.shown();
        if stale {
            self.refresh();
        }
        stale
    }

    /// The slots, for the UI to subscribe to.
    pub fn slots(&self) -> Arc<ObservableList<Slot>> {
        self.inner.slots.clone()
    }

    pub fn state(&self) -> watch::Receiver<PagedState> {
        self.inner.state.subscribe()
    }

    /// The UI shows slots `first` to `last` (inclusive, either order): load the pages around them, cancel the rest. A failed page
    /// that comes into view is retried once by itself.
    pub fn set_visible_range(&self, first: usize, last: usize) {
        let mut control = lock(&self.inner.control);
        let before: HashSet<usize> = plan(control.total, control.visible).into_iter().collect();
        control.visible = Some((first.min(last), first.max(last)));
        let now = plan(control.total, control.visible);
        for page in now.into_iter().filter(|page| !before.contains(page)) {
            match control.pages.get_mut(page) {
                Some(slot @ Page::Failed { retried: false }) => *slot = Page::Retry { auto: true },
                Some(slot @ Page::ReloadFailed) => *slot = Page::Loaded { stale: true },
                _ => {}
            }
        }
        self.inner.pump(&mut control);
    }

    /// Reads the list again without resetting it (ADR 0018, section 3).
    pub fn refresh(&self) {
        self.inner.refresh();
    }

    /// Reloads every failed page (the Retry button).
    pub fn retry(&self) {
        let mut control = lock(&self.inner.control);
        control.error = None;
        for page in &mut control.pages {
            match page {
                Page::Failed { .. } => *page = Page::Retry { auto: false },
                Page::ReloadFailed => *page = Page::Loaded { stale: true },
                _ => {}
            }
        }
        self.inner.pump(&mut control);
    }

    /// A new sort or filter: a new query, which resets the list and starts at the top.
    pub fn set_query(&self, query: ItemQuery) {
        let mut control = lock(&self.inner.control);
        control.cancel_all();
        let query = ItemQuery {
            parent: query.parent.or(control.query.parent.take()),
            ..query
        };
        let next_id = control.next_id;
        *control = Control::new(query);
        control.next_id = next_id;
        self.inner.slots.reset(Vec::new());
        self.inner.pump(&mut control);
    }
}

impl Paged {
    fn refresh(&self) {
        let mut control = lock(&self.control);
        if control.started {
            for page in &mut control.pages {
                if matches!(page, Page::Loaded { .. } | Page::ReloadFailed) {
                    *page = Page::Loaded { stale: true };
                }
            }
            control.refresh_wanted = true;
        } else {
            control.error = None;
        }
        self.pump(&mut control);
    }

    /// Starts the requests the list wants, cancels those it no longer wants, and publishes the state.
    fn pump(&self, control: &mut Control) {
        if !control.started {
            if control.flights.is_empty() && control.error.is_none() {
                self.launch(control, 0, true, false);
            }
            self.publish_state(control);
            return;
        }
        let window = plan(control.total, control.visible);
        let gone: Vec<usize> = control
            .flights
            .keys()
            .copied()
            .filter(|page| !window.contains(page))
            .collect();
        for page in gone {
            if let Some(flight) = control.flights.remove(&page) {
                flight.task.abort();
                // A refresh that was cancelled reads the total from the next page it reaches.
                control.refresh_wanted |= flight.with_total;
            }
        }
        if control.refresh_wanted && !control.flights.values().any(|flight| flight.with_total) {
            if control.flights.len() >= MAX_PAGE_REQUESTS {
                self.publish_state(control);
                return;
            }
            let page = window.first().copied().unwrap_or(0);
            if let Some(flight) = control.flights.remove(&page) {
                flight.task.abort();
            }
            control.refresh_wanted = false;
            self.launch(control, page, true, false);
        }
        for page in window {
            if control.flights.len() >= MAX_PAGE_REQUESTS {
                break;
            }
            if control.flights.contains_key(&page) {
                continue;
            }
            match control.pages.get(page) {
                Some(Page::Empty | Page::Loaded { stale: true }) => {
                    self.launch(control, page, false, false);
                }
                Some(Page::Retry { auto }) => {
                    let auto = *auto;
                    self.launch(control, page, false, auto);
                }
                _ => {}
            }
        }
        self.publish_state(control);
    }

    fn launch(&self, control: &mut Control, page: usize, with_total: bool, auto: bool) {
        control.next_id += 1;
        let id = control.next_id;
        let query = control.query.clone();
        let ctx = self.ctx.clone();
        let me = self.me.clone();
        let library = self.library.clone();
        // The task's answer waits for the control lock, held here until the flight is recorded.
        let task = self.ctx.spawn(async move {
            let start = u32::try_from(page * PAGE_SIZE).unwrap_or(u32::MAX);
            let result = match ctx.client() {
                Ok(client) => {
                    client
                        .items(&query, start, PAGE_SIZE as u32, with_total)
                        .await
                }
                Err(error) => Err(error),
            };
            // The library may be gone: the libraries are read again before the failure shows, so the navigation has dropped it
            // by then, and the grid says so in one way whatever the server answered.
            let result = match (result, &library) {
                (Err(error), Some(library)) if might_be_gone(&error) => {
                    if ctx.library_gone(library).await {
                        Err(AuthError::NotAvailable)
                    } else {
                        Err(error)
                    }
                }
                (result, _) => result,
            };
            if let Some(me) = me.upgrade() {
                me.finish(page, id, result);
            }
        });
        control.flights.insert(
            page,
            Flight {
                id,
                task,
                with_total,
                auto,
            },
        );
    }

    fn finish(&self, page: usize, id: u64, result: Result<Answer, AuthError>) {
        let mut control = lock(&self.control);
        if control
            .flights
            .get(&page)
            .is_none_or(|flight| flight.id != id)
        {
            return;
        }
        let Some(flight) = control.flights.remove(&page) else {
            return;
        };
        match result {
            Ok(answer) if !control.started => self.start(&mut control, answer),
            Ok(answer) => {
                if flight.with_total {
                    self.resize(&mut control, answer.total);
                }
                // Fewer items than the page's slots: the total shrank since it was read. Read it again; a page read with the total
                // is already sized to it.
                if !flight.with_total && answer.items.len() < page_range(page, control.total).len()
                {
                    control.refresh_wanted = true;
                }
                self.fill(&mut control, page, answer.items);
                if !control
                    .pages
                    .iter()
                    .any(|page| matches!(page, Page::Failed { .. } | Page::ReloadFailed))
                {
                    control.error = None;
                }
            }
            Err(error) => {
                if control.started {
                    // A failed request is not sent again by itself, whether it read the total or not: the page waits for Retry or
                    // for coming into view again, so a server that keeps failing gets one request per user action.
                    match control.pages.get(page) {
                        // A loaded page keeps its items.
                        Some(Page::Loaded { .. } | Page::ReloadFailed) => {
                            control.pages[page] = Page::ReloadFailed;
                        }
                        Some(_) => {
                            control.pages[page] = Page::Failed {
                                retried: flight.auto,
                            };
                            let range = page_range(page, control.total);
                            let diffs = range
                                .map(|index| VectorDiff::Set {
                                    index,
                                    value: Slot::Failed,
                                })
                                .collect();
                            let _ = self.slots.change_many(diffs);
                            forget(&mut control.index, |index| {
                                page_range(page, usize::MAX).contains(&index)
                            });
                        }
                        None => {}
                    }
                }
                control.error = Some(error);
            }
        }
        self.pump(&mut control);
    }

    /// The first page answered: the list becomes `total` slots, page 0 filled and the rest loading.
    fn start(&self, control: &mut Control, answer: Answer) {
        let total = answer
            .total
            .map_or(answer.items.len(), |total| total as usize);
        let mut items = answer.items;
        items.truncate(PAGE_SIZE.min(total));
        self.ctx.observe(&mut items);
        control.index.clear();
        for (index, item) in items.iter().enumerate() {
            control
                .index
                .entry(item.item.clone())
                .or_default()
                .push(index);
        }
        let mut slots: Vec<Slot> = items
            .into_iter()
            .map(|item| Slot::Item(Box::new(item)))
            .collect();
        slots.resize(total, Slot::Loading);
        control.total = total;
        control.pages = vec![Page::Empty; total.div_ceil(PAGE_SIZE)];
        if let Some(first) = control.pages.first_mut() {
            *first = Page::Loaded { stale: false };
        }
        control.started = true;
        control.error = None;
        self.slots.reset(slots);
    }

    /// The total changed: slots are removed or appended at the end, never reset.
    fn resize(&self, control: &mut Control, total: Option<u32>) {
        let Some(total) = total.map(|total| total as usize) else {
            return;
        };
        let old = control.total;
        if total == old {
            return;
        }
        let count = total.div_ceil(PAGE_SIZE);
        let diffs = if total < old {
            control.pages.truncate(count);
            let gone: Vec<usize> = control
                .flights
                .keys()
                .copied()
                .filter(|page| *page >= count)
                .collect();
            for page in gone {
                if let Some(flight) = control.flights.remove(&page) {
                    flight.task.abort();
                }
            }
            forget(&mut control.index, |index| index >= total);
            (total..old)
                .rev()
                .map(|index| VectorDiff::Remove { index })
                .collect()
        } else {
            // The old last page was short: the items after it are new.
            if !old.is_multiple_of(PAGE_SIZE)
                && let Some(Page::Loaded { stale }) = control.pages.last_mut()
            {
                *stale = true;
            }
            control.pages.resize(count, Page::Empty);
            vec![VectorDiff::Append {
                values: vec![Slot::Loading; total - old],
            }]
        };
        control.total = total;
        let _ = self.slots.change_many(diffs);
    }

    /// Sets the page's slots whose item changed, in one batch.
    fn fill(&self, control: &mut Control, page: usize, mut items: Vec<ItemSummary>) {
        if page >= control.pages.len() {
            return;
        }
        items.truncate(page_range(page, control.total).len());
        self.ctx.observe(&mut items);
        let start = page * PAGE_SIZE;
        let mut diffs = Vec::new();
        for (offset, item) in items.into_iter().enumerate() {
            let index = start + offset;
            let value = Slot::Item(Box::new(item));
            let old = self.slots.get(index);
            if old.as_ref() == Some(&value) {
                continue;
            }
            if let Some(Slot::Item(old)) = old
                && let Some(indices) = control.index.get_mut(&old.item)
            {
                indices.retain(|at| *at != index);
            }
            if let Slot::Item(item) = &value {
                let indices = control.index.entry(item.item.clone()).or_default();
                if !indices.contains(&index) {
                    indices.push(index);
                }
            }
            diffs.push(VectorDiff::Set { index, value });
        }
        control.index.retain(|_, indices| !indices.is_empty());
        control.pages[page] = Page::Loaded { stale: false };
        let _ = self.slots.change_many(diffs);
    }

    fn publish_state(&self, control: &Control) {
        let state = PagedState {
            total: u32::try_from(control.total).unwrap_or(u32::MAX),
            loading: !control.flights.is_empty(),
            failed: control.error.clone(),
        };
        self.state.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
    }
}

impl Live for Paged {
    fn patch(&self, item: &ItemRef, state: &UserState) {
        let control = lock(&self.control);
        let Some(indices) = control.index.get(item) else {
            return;
        };
        let diffs: Vec<_> = indices
            .iter()
            .filter_map(|index| match self.slots.get(*index) {
                Some(Slot::Item(mut summary))
                    if &summary.item == item && &summary.user != state =>
                {
                    summary.user = state.clone();
                    Some(VectorDiff::Set {
                        index: *index,
                        value: Slot::Item(summary),
                    })
                }
                _ => None,
            })
            .collect();
        let _ = self.slots.change_many(diffs);
    }

    fn dependents_changed(&self, items: &HashSet<ItemRef>) {
        // A list chosen by the user's marks (favourites, unplayed, ...) reads again on any mark: an item can join it as well as leave
        // it (the owner's run, 2026-10-26: a new favourite did not show in the Favourites tab).
        let reload = {
            let control = lock(&self.control);
            control.query.depends_on_marks()
                || items.iter().any(|item| control.index.contains_key(item))
        };
        if reload {
            self.refresh();
        }
    }

    fn refresh(&self) {
        Paged::refresh(self);
    }

    fn find(&self, item: &ItemRef) -> Option<ItemSummary> {
        let control = lock(&self.control);
        control
            .index
            .get(item)?
            .iter()
            .find_map(|index| match self.slots.get(*index) {
                Some(Slot::Item(summary)) if &summary.item == item => Some(*summary),
                _ => None,
            })
    }
    fn find_below(&self, parent: &ItemRef) -> Vec<ItemSummary> {
        let _control = lock(&self.control);
        self.slots
            .snapshot()
            .into_iter()
            .filter_map(|slot| match slot {
                Slot::Item(summary) if states::parents(&summary).contains(parent) => Some(*summary),
                _ => None,
            })
            .collect()
    }
}

/// The indices of `page`'s slots in a list of `total`.
fn page_range(page: usize, total: usize) -> std::ops::Range<usize> {
    let start = page.saturating_mul(PAGE_SIZE);
    start.min(total)..start.saturating_add(PAGE_SIZE).min(total)
}

/// Drops the slots for which `gone` holds from the index.
fn forget(index: &mut HashMap<ItemRef, Vec<usize>>, gone: impl Fn(usize) -> bool) {
    index.retain(|_, indices| {
        indices.retain(|at| !gone(*at));
        !indices.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_load_in_an_empty_list() {
        assert!(plan(0, None).is_empty());
        assert!(plan(0, Some((10, 20))).is_empty());
    }

    #[test]
    fn a_list_below_one_page_is_one_page() {
        assert_eq!(plan(1, None), vec![0]);
        assert_eq!(plan(99, Some((0, 98))), vec![0]);
        assert_eq!(plan(100, Some((0, 99))), vec![0]);
    }

    #[test]
    fn the_visible_pages_come_first_then_one_either_side() {
        assert_eq!(plan(1_000, Some((250, 420))), vec![2, 3, 4, 1, 5]);
        assert_eq!(plan(1_000, Some((420, 250))), vec![2, 3, 4, 1, 5]);
        assert_eq!(plan(1_000, None), vec![0, 1]);
    }

    #[test]
    fn the_last_partial_page_counts() {
        // 450 items: pages 0 to 4, the last holding 50.
        assert_eq!(plan(450, Some((440, 449))), vec![4, 3]);
        assert_eq!(plan(401, Some((400, 400))), vec![4, 3]);
        assert_eq!(page_range(4, 450), 400..450);
    }

    #[test]
    fn a_range_past_the_end_keeps_to_the_last_page() {
        // The total shrank from 1,000 to 250 while slots 900 to 950 showed.
        assert_eq!(plan(250, Some((900, 950))), vec![2, 1]);
        assert_eq!(page_range(5, 250), 250..250);
    }
}
