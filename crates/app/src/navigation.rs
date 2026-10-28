//! How the user arranged the pane's groups and Home's rows (ADR 0024): the order and the hidden state of each group's items, kept
//! in Current per server and user (`current-store`'s `navigation`), never in the server's configuration.
//!
//! Each group has a default order (the base): the libraries in the order `GET UserViews` gives them, hidden while the server keeps
//! them out of "My media"; the Movies and Series tabs, Favourites and Recently added, and Home's rows in the app's order. The merge
//! rule (owner, 2026-10-28):
//! 1. An item the user never moved keeps its place in the base relative to the other unmoved items, so it follows a change in the
//!    server's order.
//! 2. Moving an item stores the item just above it at the drop (or "first"), and makes it the group's latest move. Items placed
//!    after the moved item go after the item that was above it, so they stay where they were.
//! 3. Resolving takes the unmoved items in base order, then puts each moved item, oldest move first, right after its anchor. An
//!    anchor that is not in the list now (a removed library, or a moved item not placed yet) is followed along its own anchor; when
//!    nothing on that chain is, the item goes first.
//! 4. The user's hide or show wins over the base's; an item without one is hidden as the base says.
//! 5. An item with no entry (a new library, a new row) shows in its base place. A removed item's entry is kept: it still anchors
//!    others, and the item comes back to its place if its id returns. Reset forgets the group.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use current_store::{
    Anchor, NavigationScope, NavigationSettings, Placement, SettingsStore, StoreError,
};
use tokio::sync::watch;

/// The pane's Movies and Series tabs, keyed by the kind each shows (`movie`, `series`).
pub const TABS: &str = "tabs";
/// Favourites and Recently added, keyed `favourites` and `recently_added`.
pub const VIEWS: &str = "views";
/// The libraries, keyed by their item id.
pub const LIBRARIES: &str = "libraries";
/// Home's rows, keyed as [`crate::HomeRow::key`].
pub const HOME_ROWS: &str = "home_rows";

/// The key of the Favourites view in [`VIEWS`].
pub const FAVOURITES: &str = "favourites";
/// The key of the Recently added view in [`VIEWS`].
pub const RECENTLY_ADDED: &str = "recently_added";

/// One item of a group's default order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseItem {
    pub key: String,
    /// Whether the item is hidden until the user shows it: a library the server keeps out of "My media".
    pub hidden: bool,
}

impl BaseItem {
    pub fn shown(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            hidden: false,
        }
    }
}

/// One item of a group as the user arranged it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arranged {
    pub key: String,
    pub hidden: bool,
    /// Whether the base hides it: a library hidden on the server, which the editor says.
    pub hidden_by_default: bool,
}

/// The group's items in the user's order: every item of `base`, arranged by `saved` (the group's entries, oldest move first).
pub fn arrange(base: &[BaseItem], saved: &[Placement]) -> Vec<Arranged> {
    let present: HashSet<&str> = base.iter().map(|item| item.key.as_str()).collect();
    let entry = |key: &str| saved.iter().find(|entry| entry.key == key);
    let moved = |key: &str| entry(key).is_some_and(|entry| entry.after.is_some());
    let mut order: Vec<&str> = base
        .iter()
        .map(|item| item.key.as_str())
        .filter(|key| !moved(key))
        .collect();
    let mut seen = HashSet::new();
    for placement in saved {
        let key = placement.key.as_str();
        if !present.contains(key) || placement.after.is_none() || !seen.insert(key) {
            continue;
        }
        let at = anchor_position(&order, placement, &entry);
        order.insert(at, key);
    }
    order
        .into_iter()
        .filter_map(|key| {
            let item = base.iter().find(|item| item.key == key)?;
            Some(Arranged {
                key: item.key.clone(),
                hidden: entry(key)
                    .and_then(|entry| entry.hidden)
                    .unwrap_or(item.hidden),
                hidden_by_default: item.hidden,
            })
        })
        .collect()
}

/// Where `placement` goes in `order`: right after its anchor, else after its anchor's own anchor, and so on, else first.
fn anchor_position<'a>(
    order: &[&str],
    placement: &'a Placement,
    entry: &impl Fn(&str) -> Option<&'a Placement>,
) -> usize {
    let mut visited = HashSet::new();
    let mut anchor = placement.after.as_ref();
    while let Some(Anchor::After(key)) = anchor {
        if let Some(at) = order.iter().position(|known| known == key) {
            return at + 1;
        }
        // A chain that loops (not one a move makes) ends first.
        if !visited.insert(key.as_str()) {
            break;
        }
        anchor = entry(key).and_then(|entry| entry.after.as_ref());
    }
    0
}

/// The group's entries once the user moved `key` to `to` (an index in [`arrange`]'s list, clamped to its end).
pub fn moved(base: &[BaseItem], saved: &[Placement], key: &str, to: usize) -> Vec<Placement> {
    let arranged = arrange(base, saved);
    let Some(from) = arranged.iter().position(|item| item.key == key) else {
        return saved.to_vec();
    };
    let above_before = from
        .checked_sub(1)
        .map_or(Anchor::First, |at| Anchor::After(arranged[at].key.clone()));
    let mut order: Vec<&str> = arranged.iter().map(|item| item.key.as_str()).collect();
    order.remove(from);
    let to = to.min(order.len());
    let anchor = to
        .checked_sub(1)
        .map_or(Anchor::First, |at| Anchor::After(order[at].to_owned()));
    let mut entries: Vec<Placement> = saved.to_vec();
    // What was placed right after the moved item stays where it was: after the item above it.
    for entry in &mut entries {
        if entry.key != key && entry.after == Some(Anchor::After(key.to_owned())) {
            entry.after = Some(above_before.clone());
        }
    }
    let mut mine = match entries.iter().position(|entry| entry.key == key) {
        Some(at) => entries.remove(at),
        None => Placement::new(key),
    };
    mine.after = Some(anchor);
    entries.push(mine);
    entries
}

/// The group's entries once the user hid or showed `key`. A choice that matches the base forgets the user's, so the item follows
/// the server again.
pub fn with_hidden(
    base: &[BaseItem],
    saved: &[Placement],
    key: &str,
    hidden: bool,
) -> Vec<Placement> {
    let by_default = base
        .iter()
        .find(|item| item.key == key)
        .is_some_and(|item| item.hidden);
    let mut entries: Vec<Placement> = saved.to_vec();
    let choice = (hidden != by_default).then_some(hidden);
    match entries.iter_mut().find(|entry| entry.key == key) {
        Some(entry) => entry.hidden = choice,
        None if choice.is_some() => {
            let mut entry = Placement::new(key);
            entry.hidden = choice;
            entries.push(entry);
        }
        None => {}
    }
    entries.retain(|entry| !entry.is_empty());
    entries
}

/// Where the arrangements are kept: the settings file, or for an app that stores nothing, memory for the run. Every change is
/// published, so the pane and Home follow it.
pub(crate) struct Arrangements {
    /// The arrangements of an app that stores nothing; unused when there is a settings store, which is then the only copy.
    memory: Mutex<NavigationSettings>,
    /// Counts the changes saved.
    revision: watch::Sender<u64>,
}

impl Arrangements {
    pub(crate) fn new() -> Self {
        Self {
            memory: Mutex::new(NavigationSettings::default()),
            revision: watch::Sender::new(0),
        }
    }

    /// Told after each saved change.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// `scope`'s entries in `group`, oldest move first.
    pub(crate) fn group(
        &self,
        store: Option<&SettingsStore>,
        scope: &NavigationScope,
        group: &str,
    ) -> Vec<Placement> {
        match store {
            Some(store) => store.get().navigation.group(scope, group).to_vec(),
            None => self.lock().group(scope, group).to_vec(),
        }
    }

    /// Replaces `scope`'s entries in `group` and saves them: blocking file I/O. An unchanged group writes nothing.
    pub(crate) fn set_group(
        &self,
        store: Option<&SettingsStore>,
        scope: &NavigationScope,
        group: &str,
        entries: Vec<Placement>,
    ) -> Result<(), StoreError> {
        if self.group(store, scope, group) == entries {
            return Ok(());
        }
        match store {
            Some(store) => {
                store.update(|settings| settings.navigation.set_group(scope, group, entries))?;
            }
            None => self.lock().set_group(scope, group, entries),
        }
        self.revision.send_modify(|revision| *revision += 1);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NavigationSettings> {
        // A panic while holding the lock leaves usable settings behind (each change is one assignment), so poisoning is safe to
        // ignore.
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(keys: &str) -> Vec<BaseItem> {
        keys.split(' ').map(BaseItem::shown).collect()
    }

    fn keys(arranged: &[Arranged]) -> String {
        arranged
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn order(base: &[BaseItem], saved: &[Placement]) -> String {
        keys(&arrange(base, saved))
    }

    #[test]
    fn nothing_saved_is_the_base() {
        assert_eq!(order(&base("a b c"), &[]), "a b c");
        assert!(arrange(&[], &[]).is_empty());
    }

    #[test]
    fn a_moved_item_stays_where_it_was_dropped() {
        let base = base("a b c d");
        let saved = moved(&base, &[], "d", 1);
        assert_eq!(order(&base, &saved), "a d b c");
        let saved = moved(&base, &saved, "a", 4);
        assert_eq!(order(&base, &saved), "d b c a");
        let saved = moved(&base, &saved, "c", 0);
        assert_eq!(order(&base, &saved), "c d b a");
        // Moving to where it is changes nothing that shows.
        let again = moved(&base, &saved, "c", 0);
        assert_eq!(order(&base, &again), "c d b a");
    }

    #[test]
    fn unmoved_items_follow_the_servers_order() {
        let saved = moved(&base("a b c d"), &[], "d", 0);
        assert_eq!(order(&base("a b c d"), &saved), "d a b c");
        // The server's order changes: the unmoved items follow it, the moved one keeps its place.
        assert_eq!(order(&base("c b a d"), &saved), "d c b a");
    }

    #[test]
    fn a_new_item_shows_in_its_base_place() {
        let saved = moved(&base("a b c"), &[], "a", 3);
        assert_eq!(order(&base("a b c"), &saved), "b c a");
        assert_eq!(order(&base("a b n c"), &saved), "b n c a");
        // A new library comes last, as the server lists it.
        assert_eq!(order(&base("a b c n"), &saved), "b c a n");
    }

    #[test]
    fn a_removed_anchor_falls_back_along_its_own_anchor() {
        let base4 = base("a b c d");
        // b first, then d after b.
        let saved = moved(&base4, &[], "b", 0);
        let saved = moved(&base4, &saved, "d", 1);
        assert_eq!(order(&base4, &saved), "b d a c");
        // b is removed: d goes where b went, first.
        assert_eq!(order(&base("a c d"), &saved), "d a c");
        // An anchor never saved and gone: first.
        let mut lost = Placement::new("c");
        lost.after = Some(Anchor::After("gone".into()));
        assert_eq!(order(&base("a b c"), &[lost]), "c a b");
        // b comes back: both are where they were.
        assert_eq!(order(&base4, &saved), "b d a c");
    }

    #[test]
    fn items_placed_after_a_moved_item_stay_in_place() {
        let base = base("a b c d e");
        let saved = moved(&base, &[], "e", 2); // a b e c d
        let saved = moved(&base, &saved, "d", 3); // a b e d c
        assert_eq!(order(&base, &saved), "a b e d c");
        // e leaves: d was placed after it and stays after b's neighbourhood, where it showed.
        let saved = moved(&base, &saved, "e", 0);
        assert_eq!(order(&base, &saved), "e a b d c");
    }

    #[test]
    fn a_loop_of_anchors_ends_first() {
        let mut x = Placement::new("x");
        x.after = Some(Anchor::After("y".into()));
        let mut y = Placement::new("y");
        y.after = Some(Anchor::After("x".into()));
        assert_eq!(order(&base("a x y"), &[x, y]), "x y a");
    }

    /// Every move shows what the user dropped: the list with the item taken out and put at the index, whatever came before.
    #[test]
    fn every_move_resolves_to_what_was_dropped() {
        let base = base("a b c d e f g");
        let mut saved = Vec::new();
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            // A small xorshift: the same moves on every run.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % bound as u64).unwrap()
        };
        for _ in 0..500 {
            let shown: Vec<String> = arrange(&base, &saved)
                .into_iter()
                .map(|item| item.key)
                .collect();
            let from = next(shown.len());
            let to = next(shown.len());
            let mut want = shown.clone();
            let key = want.remove(from);
            want.insert(to, key.clone());
            saved = moved(&base, &saved, &key, to);
            assert_eq!(order(&base, &saved), want.join(" "));
        }
    }

    #[test]
    fn hiding_wins_over_the_base_and_matching_it_forgets_the_choice() {
        let base = vec![
            BaseItem::shown("a"),
            BaseItem {
                key: "h".into(),
                hidden: true,
            },
        ];
        let arranged = arrange(&base, &[]);
        assert!(!arranged[0].hidden);
        assert!(arranged[1].hidden && arranged[1].hidden_by_default);

        let saved = with_hidden(&base, &[], "h", false);
        let arranged = arrange(&base, &saved);
        assert!(!arranged[1].hidden && arranged[1].hidden_by_default);
        let saved = with_hidden(&base, &saved, "a", true);
        assert!(arrange(&base, &saved)[0].hidden);
        // Back to the base: nothing is kept for it.
        let saved = with_hidden(&base, &saved, "h", true);
        let saved = with_hidden(&base, &saved, "a", false);
        assert!(saved.is_empty());
    }

    #[test]
    fn hiding_keeps_a_moved_items_place() {
        let base = base("a b c");
        let saved = moved(&base, &[], "c", 0);
        let saved = with_hidden(&base, &saved, "c", true);
        let arranged = arrange(&base, &saved);
        assert_eq!(keys(&arranged), "c a b");
        assert!(arranged[0].hidden);
        let saved = with_hidden(&base, &saved, "c", false);
        assert_eq!(order(&base, &saved), "c a b");
    }
}
