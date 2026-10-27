//! One session's user state of every item its live lists hold (ADR 0018, section 5, amended 2026-10-27 by the owner): the one source
//! every list shows an item's marks from. Each entry is the item as the server gave it last, on the session's clock
//! ([`super::Context::tick`]): a list's read takes the clock when its request goes out, an answer from the server (a mark's, a
//! read-back's, a stop's) when it arrives. The newest wins: a read newer than the entry replaces it and every list shows it; an
//! older one (a slow answer that left the server before a later change) shows the entry instead. The changes the user wants that
//! the server has not confirmed yet ([`super::states::ItemStates`]) show on top of it.
//!
//! It grows with what the live lists hold, not with what the session loaded: after each read, the entries are dropped that no live
//! list holds, no change waits for, and no read still in flight could be older than ([`ItemStore::retain`]): a read that left before
//! an answer must still find that answer when it lands, whatever lists came and went meanwhile.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use current_model::{ItemRef, ItemSummary, UserState};

use super::{lock, states};

struct Entry {
    /// The server's state as it stands last.
    state: UserState,
    /// The item as a list read it last; `None` for an answer about an item no list held ([`ItemStore::answer`]).
    summary: Option<ItemSummary>,
    /// When that state was read or answered.
    at: u64,
    /// The series of a season, from the page that listed it: a season names no series itself.
    series: Option<ItemRef>,
}

impl Entry {
    /// The item with the state held.
    fn summary(&self) -> Option<ItemSummary> {
        let mut summary = self.summary.clone()?;
        summary.user = self.state.clone();
        Some(summary)
    }

    /// The series and season of an episode, the series of a season.
    fn parents(&self) -> Vec<ItemRef> {
        let mut parents = self
            .summary
            .as_ref()
            .map(states::parents)
            .unwrap_or_default();
        parents.extend(self.series.iter().cloned());
        parents
    }
}

/// The reads in flight, by when they went out, and how many went out then.
type Reads = Arc<Mutex<BTreeMap<u64, usize>>>;

/// A list's read in flight, from when its request goes out until its answer is shown or dropped ([`ItemStore::begin`]).
pub(super) struct ReadTicket {
    reads: Reads,
    pub(super) at: u64,
}

impl Drop for ReadTicket {
    fn drop(&mut self) {
        let mut reads = lock(&self.reads);
        if let Some(count) = reads.get_mut(&self.at) {
            *count -= 1;
            if *count == 0 {
                reads.remove(&self.at);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct ItemStore {
    entries: Mutex<HashMap<ItemRef, Entry>>,
    reads: Reads,
}

impl ItemStore {
    /// A list's read whose request goes out at `at`: entries it may be older than stay until it is shown or dropped.
    pub(super) fn begin(&self, at: u64) -> ReadTicket {
        *lock(&self.reads).entry(at).or_default() += 1;
        ReadTicket {
            reads: self.reads.clone(),
            at,
        }
    }

    /// Takes `summary`, read at `at` (below `series`, for a season on its series' page): if it is newer than what the store holds,
    /// it becomes the item's state and the result says whether that changed; if older, `summary` takes the store's state.
    pub(super) fn read(
        &self,
        summary: &mut ItemSummary,
        at: u64,
        series: Option<&ItemRef>,
    ) -> bool {
        let mut entries = lock(&self.entries);
        match entries.get_mut(&summary.item) {
            Some(entry) if entry.at > at => {
                summary.user = entry.state.clone();
                if entry.summary.is_none() {
                    entry.summary = Some(summary.clone());
                }
                if entry.series.is_none() {
                    entry.series = series.cloned();
                }
                false
            }
            Some(entry) => {
                let changed = entry.state != summary.user;
                entry.state = summary.user.clone();
                entry.summary = Some(summary.clone());
                entry.at = at;
                if series.is_some() {
                    entry.series = series.cloned();
                }
                changed
            }
            None => {
                entries.insert(
                    summary.item.clone(),
                    Entry {
                        state: summary.user.clone(),
                        summary: Some(summary.clone()),
                        at,
                        series: series.cloned(),
                    },
                );
                false
            }
        }
    }

    /// The server's answer for `item` at `at` (a mark, a read-back, a stop): it becomes the item's state unless a newer one is held.
    /// Returns whether the state changed. For an item no list holds it is kept too, for a read in flight that is older.
    pub(super) fn answer(&self, item: &ItemRef, state: UserState, at: u64) -> bool {
        let mut entries = lock(&self.entries);
        let Some(entry) = entries.get_mut(item) else {
            entries.insert(
                item.clone(),
                Entry {
                    state,
                    summary: None,
                    at,
                    series: None,
                },
            );
            return false;
        };
        if entry.at > at {
            return false;
        }
        entry.at = at;
        let changed = entry.state != state;
        entry.state = state;
        changed
    }

    /// `item` with the server's state as held, if a list read it.
    pub(super) fn get(&self, item: &ItemRef) -> Option<ItemSummary> {
        lock(&self.entries).get(item).and_then(Entry::summary)
    }

    /// The series of `item`, a season, as the page that listed it said.
    pub(super) fn series_of(&self, item: &ItemRef) -> Option<ItemRef> {
        lock(&self.entries)
            .get(item)
            .and_then(|entry| entry.series.clone())
    }

    /// The items held below `parent`: the episodes whose series or season it is, and a series' seasons.
    pub(super) fn below(&self, parent: &ItemRef) -> Vec<ItemSummary> {
        lock(&self.entries)
            .values()
            .filter(|entry| entry.parents().contains(parent))
            .filter_map(Entry::summary)
            .collect()
    }

    /// Keeps the items in `held` (what the live lists hold and the changes wait for), and those answered or read after a read still
    /// in flight went out.
    pub(super) fn retain(&self, held: &HashSet<ItemRef>) {
        let oldest = lock(&self.reads).keys().next().copied();
        lock(&self.entries).retain(|item, entry| {
            held.contains(item) || oldest.is_some_and(|oldest| entry.at > oldest)
        });
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ItemId, MediaKind, ServerKey, ServerRef, ServiceType};

    use super::*;

    fn summary(played: bool) -> ItemSummary {
        ItemSummary {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new("a"),
            ),
            kind: MediaKind::MOVIE,
            name: "Example Film".into(),
            year: None,
            runtime: None,
            artwork: Vec::new(),
            user: UserState {
                played,
                ..UserState::default()
            },
            episode: None,
            child_count: None,
        }
    }

    #[test]
    fn the_newest_read_or_answer_wins() {
        let store = ItemStore::default();
        let mut first = summary(false);
        assert!(
            !store.read(&mut first, 2, None),
            "a first read changes nothing shown"
        );
        // Read after: it replaces the state, and the lists are told.
        let mut newer = summary(true);
        assert!(store.read(&mut newer, 5, None));
        // Read before, answered after: it shows the newer state.
        let mut older = summary(false);
        assert!(!store.read(&mut older, 3, None));
        assert!(older.user.played);
        // An answer older than the read is dropped; a newer one wins.
        let item = first.item.clone();
        assert!(!store.answer(&item, UserState::default(), 4));
        assert!(store.answer(&item, UserState::default(), 6));
        assert_eq!(store.get(&item).map(|held| held.user.played), Some(false));

        store.retain(&HashSet::new());
        assert!(store.get(&item).is_none(), "no list holds it");
        assert!(!store.answer(&item, UserState::default(), 7));
    }
}
