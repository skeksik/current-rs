//! One session's played and favourite marks (ADR 0018, section 5): what each loaded item showed last, and the changes the user
//! wants that the server has not confirmed yet. A wanted change shows at once; the request for an item goes out one at a time, and
//! only the last wanted state follows one in flight; a failed request rolls the change back.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use current_jellyfin::AuthError;
use current_model::{ItemRef, ItemSummary, UserState};

use super::lock;

/// A change to an item's marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    SetPlayed(bool),
    SetFavourite(bool),
}

/// Something the UI tells the user once, as a message. Debug leaves out the item's name.
#[derive(Clone, PartialEq, Eq)]
pub struct BrowseNotice {
    /// Counts up with each notice, so two identical failures can be told apart.
    pub seq: u64,
    pub kind: NoticeKind,
}

/// Debug leaves out the item's name.
#[derive(Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// The server did not take a played or favourite change: it was rolled back. `item_name` is for the message only; it never
    /// goes to the log.
    ActionFailed {
        action: ItemAction,
        item_name: Option<String>,
        problem: AuthError,
    },
}

impl std::fmt::Debug for BrowseNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowseNotice")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::fmt::Debug for NoticeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActionFailed {
                action, problem, ..
            } => f
                .debug_struct("ActionFailed")
                .field("action", action)
                .field("problem", problem)
                .finish_non_exhaustive(),
        }
    }
}

/// The changes wanted for one item.
struct Pending {
    /// The state the server confirmed last (or the lists showed before the first change).
    server: UserState,
    played: Option<bool>,
    favourite: Option<bool>,
    /// The item's name for a notice, as a list showed it when the change was wanted.
    name: Option<String>,
    /// The series and season of an episode: lists of them depend on its played mark.
    parents: Vec<ItemRef>,
}

impl Pending {
    /// What the lists show: the server's state with the wanted changes.
    fn shown(&self) -> UserState {
        let mut shown = self.server.clone();
        if let Some(played) = self.played {
            set_played(&mut shown, played);
        }
        if let Some(favourite) = self.favourite {
            shown.favourite = favourite;
        }
        shown
    }
}

/// Marks an item played or unplayed as the server would: a played item has no resume point and nothing unplayed inside.
fn set_played(state: &mut UserState, played: bool) {
    if state.played == played {
        return;
    }
    state.played = played;
    state.position = None;
    state.progress = None;
    if played && let Some(unplayed) = &mut state.unplayed_count {
        *unplayed = 0;
    }
}

/// The series and season of an episode summary.
pub(super) fn parents(summary: &ItemSummary) -> Vec<ItemRef> {
    summary
        .episode
        .as_ref()
        .map(|links| {
            links
                .series
                .iter()
                .chain(links.season.iter())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// How a request ended.
pub(super) struct Answered {
    /// What the lists show now.
    pub(super) shown: UserState,
    /// On failure: the item's name for the notice, and why.
    pub(super) failure: Option<(Option<String>, AuthError)>,
    /// The lists depending on the item: its series and season.
    pub(super) parents: Vec<ItemRef>,
}

/// Only the items with changes not yet confirmed are kept: what an item shows otherwise is read from the live lists, so nothing
/// here grows with what the session loaded.
#[derive(Default)]
pub(super) struct ItemStates {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    pending: HashMap<ItemRef, Pending>,
    /// Items whose changes are being sent.
    sending: HashSet<ItemRef>,
}

impl ItemStates {
    /// Gives `summary` the state of a change still pending for it.
    pub(super) fn observe(&self, summary: &mut ItemSummary) {
        if let Some(pending) = lock(&self.inner).pending.get(&summary.item) {
            summary.user = pending.shown();
        }
    }

    /// The state of `item` while a change for it is pending.
    pub(super) fn pending(&self, item: &ItemRef) -> Option<UserState> {
        lock(&self.inner).pending.get(item).map(Pending::shown)
    }

    /// Records a wanted change; `shown` is the item as a live list shows it, if one does. Returns what the lists show now, and
    /// whether a sender must start (none runs for the item).
    pub(super) fn want(
        &self,
        item: &ItemRef,
        action: ItemAction,
        shown: Option<&ItemSummary>,
    ) -> (UserState, bool) {
        let mut inner = lock(&self.inner);
        let pending = inner
            .pending
            .entry(item.clone())
            .or_insert_with(|| Pending {
                server: shown
                    .map(|summary| summary.user.clone())
                    .unwrap_or_default(),
                played: None,
                favourite: None,
                name: shown.map(|summary| summary.name.clone()),
                parents: shown.map(parents).unwrap_or_default(),
            });
        match action {
            ItemAction::SetPlayed(played) => pending.played = Some(played),
            ItemAction::SetFavourite(favourite) => pending.favourite = Some(favourite),
        }
        let state = pending.shown();
        let start = inner.sending.insert(item.clone());
        (state, start)
    }

    /// The next request to send for `item`, or `None` when nothing differs from the server's state any more: the item's changes
    /// are then done and its sender stops.
    pub(super) fn next(&self, item: &ItemRef) -> Option<ItemAction> {
        let mut inner = lock(&self.inner);
        let action = inner.pending.get_mut(item).and_then(|pending| {
            if pending.played == Some(pending.server.played) {
                pending.played = None;
            }
            if pending.favourite == Some(pending.server.favourite) {
                pending.favourite = None;
            }
            pending
                .played
                .map(ItemAction::SetPlayed)
                .or(pending.favourite.map(ItemAction::SetFavourite))
        });
        if action.is_none() {
            inner.pending.remove(item);
            inner.sending.remove(item);
        }
        action
    }

    /// Takes the server's answer to `action`: its state on success; on failure, the change is dropped, which rolls it back.
    pub(super) fn answered(
        &self,
        item: &ItemRef,
        action: ItemAction,
        result: Result<UserState, AuthError>,
    ) -> Answered {
        let mut inner = lock(&self.inner);
        // Only the sender removes an item's changes, so they are there while it waits.
        let Some(pending) = inner.pending.get_mut(item) else {
            return Answered {
                shown: result.clone().unwrap_or_default(),
                failure: result.err().map(|problem| (None, problem)),
                parents: Vec::new(),
            };
        };
        let failure = match result {
            Ok(state) => {
                pending.server = state;
                None
            }
            Err(problem) => {
                match action {
                    ItemAction::SetPlayed(_) => pending.played = None,
                    ItemAction::SetFavourite(_) => pending.favourite = None,
                }
                Some((pending.name.clone(), problem))
            }
        };
        Answered {
            shown: pending.shown(),
            failure,
            parents: pending.parents.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ItemId, MediaKind, ServerKey, ServiceType};

    use super::*;

    fn item(id: &str) -> ItemRef {
        ItemRef::new(
            current_model::ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new(id),
        )
    }

    fn summary(id: &str, user: UserState) -> ItemSummary {
        ItemSummary {
            item: item(id),
            kind: MediaKind::MOVIE,
            name: "Example Film".into(),
            year: None,
            runtime: None,
            artwork: Vec::new(),
            user,
            episode: None,
            child_count: None,
        }
    }

    fn played(on: bool) -> UserState {
        UserState {
            played: on,
            ..UserState::default()
        }
    }

    #[test]
    fn only_the_last_wanted_state_follows_one_in_flight() {
        let states = ItemStates::default();
        let film = summary("a", played(false));
        let (shown, start) = states.want(&item("a"), ItemAction::SetPlayed(true), Some(&film));
        assert!(shown.played && start);
        assert_eq!(states.next(&item("a")), Some(ItemAction::SetPlayed(true)));
        // While it is in flight: unplayed, played, unplayed. No second sender.
        for on in [false, true, false] {
            assert!(!states.want(&item("a"), ItemAction::SetPlayed(on), None).1);
        }
        let answered = states.answered(&item("a"), ItemAction::SetPlayed(true), Ok(played(true)));
        assert!(!answered.shown.played, "the last wish still shows");
        assert_eq!(states.next(&item("a")), Some(ItemAction::SetPlayed(false)));
        states.answered(&item("a"), ItemAction::SetPlayed(false), Ok(played(false)));
        assert_eq!(states.next(&item("a")), None);
        assert_eq!(
            states.pending(&item("a")),
            None,
            "nothing is kept once done"
        );
    }

    #[test]
    fn a_change_undone_before_it_is_sent_sends_nothing() {
        let states = ItemStates::default();
        let film = summary("a", played(false));
        states.want(&item("a"), ItemAction::SetFavourite(true), Some(&film));
        states.want(&item("a"), ItemAction::SetFavourite(false), Some(&film));
        assert_eq!(states.next(&item("a")), None);
    }

    #[test]
    fn a_failure_rolls_back_and_names_the_item_for_the_notice() {
        let states = ItemStates::default();
        let film = summary("a", played(false));
        states.want(&item("a"), ItemAction::SetPlayed(true), Some(&film));
        states.next(&item("a"));
        let answered = states.answered(
            &item("a"),
            ItemAction::SetPlayed(true),
            Err(AuthError::NotSignedIn),
        );
        assert!(!answered.shown.played);
        assert_eq!(
            answered.failure,
            Some((Some("Example Film".into()), AuthError::NotSignedIn))
        );
        assert_eq!(states.next(&item("a")), None);
    }

    #[test]
    fn items_loaded_while_a_change_is_pending_take_it() {
        let states = ItemStates::default();
        states.want(&item("a"), ItemAction::SetPlayed(true), None);
        let mut late = summary("a", played(false));
        states.observe(&mut late);
        assert!(late.user.played);
    }

    #[test]
    fn a_notice_debugs_without_the_name() {
        let notice = BrowseNotice {
            seq: 1,
            kind: NoticeKind::ActionFailed {
                action: ItemAction::SetPlayed(true),
                item_name: Some("Example Film".into()),
                problem: AuthError::NotSignedIn,
            },
        };
        let debug = format!("{notice:?}");
        assert!(
            debug.contains("SetPlayed") && !debug.contains("Example"),
            "{debug}"
        );
    }

    #[test]
    fn marking_played_clears_the_resume_point_and_the_unplayed_count() {
        let mut state = UserState {
            position: Some(std::time::Duration::from_secs(60)),
            progress: Some(0.5),
            unplayed_count: Some(3),
            ..UserState::default()
        };
        set_played(&mut state, true);
        assert_eq!(state.position, None);
        assert_eq!(state.progress, None);
        assert_eq!(state.unplayed_count, Some(0));
    }
}
