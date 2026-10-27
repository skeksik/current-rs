//! One session's played and favourite marks (ADR 0018, section 5): what each loaded item showed last, and the changes the user
//! wants that the server has not confirmed yet. A wanted change shows at once; the request for an item goes out one at a time, and
//! only the last wanted state follows one in flight; a failed request rolls the change back.
//!
//! A played mark on a series or season is one on every episode below it: Jellyfin marks each of them and derives the folder's own
//! state from them (`Folder.MarkPlayed`, `Folder.FillUserDataDtoValues`, v12.2 at ca0f16eb, read 2026-10-27). So while such a change
//! is pending, the episodes and seasons below it show it too, unless they have a newer change of their own, and a failure rolls
//! them back with it.

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
    /// The series and season of an episode, or the series of a season: lists of them depend on its played mark, and a played change
    /// pending for them shows on it.
    parents: Vec<ItemRef>,
    /// When the played change was wanted, against every other item's: the newest of an item's and its parents' wins.
    played_seq: u64,
    /// The items below this one (a series' or season's episodes and seasons) as the lists showed them before its first played
    /// change, then as the server's answers left them: what a failure rolls them back to.
    below: HashMap<ItemRef, ItemSummary>,
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

/// Marks an item played or unplayed as the server would: a played item has no resume point and nothing unplayed inside, and has
/// been played at least once; an unplayed one has no resume point and no plays (`BaseItem.MarkPlayed`: "`data.PlayCount =
/// Math.Max(data.PlayCount, 1)`", `ResetPlayedState`: "`data.PlayCount = 0; data.PlaybackPositionTicks = 0`", v12.2, read
/// 2026-10-27). An unplayed folder's count of unplayed items is not known until the server answers: it keeps what it showed.
pub(super) fn set_played(state: &mut UserState, played: bool) {
    if state.played == played {
        return;
    }
    state.played = played;
    state.position = None;
    state.progress = None;
    if played {
        state.play_count = state.play_count.max(1);
        if let Some(unplayed) = &mut state.unplayed_count {
            *unplayed = 0;
        }
    } else {
        state.play_count = 0;
    }
}

/// What a parent (a series or season) shows at once when `child`, shown as `before`, is marked `played`: its unplayed count moves by
/// the child's (one for an episode, a season's own count when it is marked played), and it is played when nothing below is unplayed
/// (`Folder.FillUserDataDtoValues`: "`dto.Played = playedCount >= totalCount`"). Only an estimate: it is off where the server counts
/// what the change does not move (an episode marked on another device, one the lists never loaded), so the parent's marks are read
/// again from the server after the change (`Context::read_back`), and that answer replaces it.
pub(super) fn estimate_parent(parent: &mut UserState, before: &UserState, played: bool) {
    if before.played == played {
        return;
    }
    // An episode has no count of its own: it is one item.
    let moved = before.unplayed_count.unwrap_or(1);
    if played {
        if let Some(unplayed) = &mut parent.unplayed_count {
            *unplayed = unplayed.saturating_sub(moved);
            if *unplayed == 0 {
                set_played(parent, true);
            }
        }
    } else {
        if before.unplayed_count.is_none()
            && let Some(unplayed) = &mut parent.unplayed_count
        {
            *unplayed += 1;
        }
        // Something below is unplayed now; how much it played is not known until the server answers.
        parent.played = false;
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
    /// On a failed played change: the items below the item, to show again as they were (each through [`ItemStates::observe`]).
    pub(super) rolled_back: Vec<ItemSummary>,
}

/// Only the items with changes not yet confirmed are kept, and a count of the changes wanted per item: what an item shows otherwise
/// is read from the live lists, so nothing here grows with what the session loaded, only with what the user marked.
#[derive(Default)]
pub(super) struct ItemStates {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    pending: HashMap<ItemRef, Pending>,
    /// Items whose changes are being sent.
    sending: HashSet<ItemRef>,
    /// How many changes were wanted per item.
    wanted: HashMap<ItemRef, u64>,
    /// Counts the played changes wanted, for [`Pending::played_seq`] and [`ItemStates::played_wants`].
    seq: u64,
    /// Series and seasons showing an estimate: lists loaded before the server's marks are read again show it too, so an answer read
    /// before the change does not undo it.
    estimates: HashMap<ItemRef, Estimate>,
}

impl Inner {
    /// The state `item` shows: `base` (what a list or the server gave), with the item's own pending changes, and the newest played
    /// change pending for one of `parents` if it is newer than the item's own.
    fn shown(&self, item: &ItemRef, parents: &[ItemRef], base: UserState) -> UserState {
        let own = self.pending.get(item);
        let mut shown = own.map_or(base, Pending::shown);
        let since = own
            .filter(|pending| pending.played.is_some())
            .map_or(0, |pending| pending.played_seq);
        let inherited = parents
            .iter()
            .filter_map(|parent| self.pending.get(parent))
            .filter_map(|pending| pending.played.map(|played| (pending.played_seq, played)))
            .filter(|(seq, _)| *seq > since)
            .max_by_key(|(seq, _)| *seq);
        let since = match inherited {
            Some((seq, played)) => {
                set_played(&mut shown, played);
                seq
            }
            None => since,
        };
        // An estimate made after every change above stands until the server's marks are read again.
        if let Some(estimate) = self.estimates.get(item)
            && estimate.seq > since
        {
            shown.played = estimate.played;
            shown.unplayed_count = estimate.unplayed_count;
            shown.progress = estimate.progress;
            shown.position = estimate.position;
        }
        shown
    }
}

/// What a series or season shows of an estimate ([`estimate_parent`]) until its marks are read again.
struct Estimate {
    played: bool,
    unplayed_count: Option<u32>,
    progress: Option<f32>,
    position: Option<std::time::Duration>,
    /// [`Inner::seq`] when it was made.
    seq: u64,
}

impl ItemStates {
    /// Gives `summary` the state of a change still pending for it, or for its series or season.
    pub(super) fn observe(&self, summary: &mut ItemSummary) {
        self.observe_under(summary, None);
    }

    /// As [`ItemStates::observe`], for an item below `parent` too (a season on its series' page, which names no series itself).
    pub(super) fn observe_under(&self, summary: &mut ItemSummary, parent: Option<&ItemRef>) {
        let mut up = parents(summary);
        up.extend(parent.cloned());
        let inner = lock(&self.inner);
        if let Some(pending) = inner.pending.get(&summary.item) {
            up.extend(pending.parents.iter().cloned());
        }
        summary.user = inner.shown(&summary.item, &up, summary.user.clone());
    }

    /// Keeps `below` (an item below `item`, as a list shows it before `item`'s played change) for a rollback, unless one is kept.
    pub(super) fn keep_below(&self, item: &ItemRef, below: &ItemSummary) {
        if let Some(pending) = lock(&self.inner).pending.get_mut(item) {
            pending
                .below
                .entry(below.item.clone())
                .or_insert_with(|| below.clone());
        }
    }

    /// How many changes were wanted for `item` so far: an answer asked for before a later change must not show over it.
    pub(super) fn wanted(&self, item: &ItemRef) -> u64 {
        lock(&self.inner).wanted.get(item).copied().unwrap_or(0)
    }

    /// Records `state` as `item`'s estimate (a series or season whose child changed) until [`ItemStates::settle`].
    pub(super) fn estimate(&self, item: &ItemRef, state: &UserState) {
        let mut inner = lock(&self.inner);
        let seq = inner.seq;
        inner.estimates.insert(
            item.clone(),
            Estimate {
                played: state.played,
                unplayed_count: state.unplayed_count,
                progress: state.progress,
                position: state.position,
                seq,
            },
        );
    }

    /// `item`'s marks were read again (or could not be) after `played_wants` played changes: an estimate made no later than that is
    /// done.
    pub(super) fn settle(&self, item: &ItemRef, played_wants: u64) {
        let mut inner = lock(&self.inner);
        if inner
            .estimates
            .get(item)
            .is_some_and(|estimate| estimate.seq <= played_wants)
        {
            inner.estimates.remove(item);
        }
    }

    /// Whether a played change is wanted that the server has not answered yet: what the server counts now may not include it.
    pub(super) fn played_pending(&self) -> bool {
        lock(&self.inner)
            .pending
            .values()
            .any(|pending| pending.played.is_some())
    }

    /// How many played changes were wanted so far, for any item: an answer read before a later one may count what it changed wrong.
    pub(super) fn played_wants(&self) -> u64 {
        lock(&self.inner).seq
    }

    /// The state of `item` while a change for it, or a played change for its series or season, is pending.
    pub(super) fn pending(&self, item: &ItemRef) -> Option<UserState> {
        let inner = lock(&self.inner);
        let pending = inner.pending.get(item)?;
        Some(inner.shown(item, &pending.parents, pending.server.clone()))
    }

    /// `state` (what a list or the server gave for `item`) as the lists show it: with the changes pending for it and its estimate.
    pub(super) fn shown(&self, item: &ItemRef, state: UserState) -> UserState {
        let inner = lock(&self.inner);
        let parents = inner
            .pending
            .get(item)
            .map(|pending| pending.parents.clone())
            .unwrap_or_default();
        inner.shown(item, &parents, state)
    }

    /// Records a wanted change; `shown` is the item as a live list shows it, if one does, and `parents` its series and season.
    /// Returns what the lists show now, and whether a sender must start (none runs for the item).
    pub(super) fn want(
        &self,
        item: &ItemRef,
        action: ItemAction,
        shown: Option<&ItemSummary>,
        parents: Vec<ItemRef>,
    ) -> (UserState, bool) {
        let mut inner = lock(&self.inner);
        *inner.wanted.entry(item.clone()).or_default() += 1;
        if matches!(action, ItemAction::SetPlayed(_)) {
            inner.seq += 1;
        }
        let seq = inner.seq;
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
                parents: Vec::new(),
                played_seq: 0,
                below: HashMap::new(),
            });
        for parent in parents {
            if !pending.parents.contains(&parent) {
                pending.parents.push(parent);
            }
        }
        match action {
            ItemAction::SetPlayed(played) => {
                pending.played = Some(played);
                pending.played_seq = seq;
            }
            ItemAction::SetFavourite(favourite) => pending.favourite = Some(favourite),
        }
        let up = pending.parents.clone();
        let state = inner.shown(item, &up, shown.map(|s| s.user.clone()).unwrap_or_default());
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
                rolled_back: Vec::new(),
            };
        };
        let mut rolled_back = Vec::new();
        let failure = match result {
            Ok(state) => {
                pending.server = state;
                // The server marked everything below as well: a later failure rolls back to that.
                if let ItemAction::SetPlayed(played) = action {
                    for below in pending.below.values_mut() {
                        set_played(&mut below.user, played);
                    }
                }
                None
            }
            Err(problem) => {
                match action {
                    ItemAction::SetPlayed(_) => {
                        pending.played = None;
                        rolled_back = pending.below.values().cloned().collect();
                    }
                    ItemAction::SetFavourite(_) => pending.favourite = None,
                }
                Some((pending.name.clone(), problem))
            }
        };
        let parents = pending.parents.clone();
        let server = pending.server.clone();
        Answered {
            shown: inner.shown(item, &parents, server),
            failure,
            parents,
            rolled_back,
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
        let (shown, start) = states.want(
            &item("a"),
            ItemAction::SetPlayed(true),
            Some(&film),
            Vec::new(),
        );
        assert!(shown.played && start);
        assert_eq!(states.next(&item("a")), Some(ItemAction::SetPlayed(true)));
        // While it is in flight: unplayed, played, unplayed. No second sender.
        for on in [false, true, false] {
            assert!(
                !states
                    .want(&item("a"), ItemAction::SetPlayed(on), None, Vec::new())
                    .1
            );
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
        states.want(
            &item("a"),
            ItemAction::SetFavourite(true),
            Some(&film),
            Vec::new(),
        );
        states.want(
            &item("a"),
            ItemAction::SetFavourite(false),
            Some(&film),
            Vec::new(),
        );
        assert_eq!(states.next(&item("a")), None);
    }

    #[test]
    fn a_failure_rolls_back_and_names_the_item_for_the_notice() {
        let states = ItemStates::default();
        let film = summary("a", played(false));
        states.want(
            &item("a"),
            ItemAction::SetPlayed(true),
            Some(&film),
            Vec::new(),
        );
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
        states.want(&item("a"), ItemAction::SetPlayed(true), None, Vec::new());
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

    fn episode(id: &str, user: UserState) -> ItemSummary {
        ItemSummary {
            kind: MediaKind::EPISODE,
            episode: Some(current_model::EpisodeLinks {
                series: Some(item("s")),
                season: Some(item("t")),
                ..current_model::EpisodeLinks::default()
            }),
            ..summary(id, user)
        }
    }

    #[test]
    fn a_series_change_shows_below_it_unless_an_episode_s_own_is_newer() {
        let states = ItemStates::default();
        let series = summary("s", played(false));
        states.want(
            &item("s"),
            ItemAction::SetPlayed(true),
            Some(&series),
            Vec::new(),
        );
        let mut loaded = episode("e", played(false));
        states.observe(&mut loaded);
        assert!(loaded.user.played, "an episode loaded meanwhile takes it");
        // A season names no series: its page says which.
        let mut season = summary("t", played(false));
        states.observe_under(&mut season, Some(&item("s")));
        assert!(season.user.played);

        // The episode's own change, made later, wins.
        let shown = episode("e", played(true));
        let (state, _) = states.want(
            &item("e"),
            ItemAction::SetPlayed(false),
            Some(&shown),
            parents(&shown),
        );
        assert!(!state.played);
        assert_eq!(
            states.pending(&item("e")).map(|state| state.played),
            Some(false)
        );
        // A newer series change wins again.
        states.want(&item("s"), ItemAction::SetPlayed(true), None, Vec::new());
        assert_eq!(
            states.pending(&item("e")).map(|state| state.played),
            Some(true)
        );
    }

    #[test]
    fn a_failed_series_change_rolls_back_what_was_below_it() {
        let states = ItemStates::default();
        states.want(&item("s"), ItemAction::SetPlayed(true), None, Vec::new());
        states.keep_below(&item("s"), &episode("e", played(false)));
        states.keep_below(&item("s"), &episode("e", played(true)));
        states.next(&item("s"));
        let answered = states.answered(
            &item("s"),
            ItemAction::SetPlayed(true),
            Err(AuthError::NotSignedIn),
        );
        assert_eq!(answered.rolled_back.len(), 1);
        assert!(!answered.rolled_back[0].user.played, "as it was first");
    }

    #[test]
    fn an_estimate_stands_until_settled_and_a_newer_change_wins_over_it() {
        let states = ItemStates::default();
        // A read of the series' marks begins before the change.
        let read_began = states.played_wants();
        let shown = episode("e", played(false));
        states.want(
            &item("e"),
            ItemAction::SetPlayed(true),
            Some(&shown),
            parents(&shown),
        );
        let estimate = UserState {
            played: true,
            unplayed_count: Some(0),
            ..UserState::default()
        };
        states.estimate(&item("s"), &estimate);
        // A load that read the server before the change.
        let mut loaded = summary(
            "s",
            UserState {
                unplayed_count: Some(1),
                ..UserState::default()
            },
        );
        states.observe(&mut loaded);
        assert!(loaded.user.played && loaded.user.unplayed_count == Some(0));
        // That read's answer predates the estimate: it stays.
        states.settle(&item("s"), read_began);
        assert!(states.shown(&item("s"), UserState::default()).played);
        states.settle(&item("s"), states.played_wants());
        assert!(!states.shown(&item("s"), UserState::default()).played);

        // The series' own change, made after an estimate, wins over it.
        states.estimate(&item("s"), &estimate);
        states.want(&item("s"), ItemAction::SetPlayed(false), None, Vec::new());
        assert_eq!(
            states.pending(&item("s")).map(|state| state.played),
            Some(false)
        );
    }

    #[test]
    fn a_parent_s_count_moves_with_its_child() {
        let mut series = UserState {
            unplayed_count: Some(2),
            ..UserState::default()
        };
        estimate_parent(&mut series, &played(false), true);
        assert_eq!(series.unplayed_count, Some(1));
        assert!(!series.played);
        estimate_parent(&mut series, &played(false), true);
        assert_eq!(series.unplayed_count, Some(0));
        assert!(series.played);
        // Already in that state: nothing moves.
        estimate_parent(&mut series, &played(true), true);
        assert_eq!(series.unplayed_count, Some(0));
        estimate_parent(&mut series, &played(true), false);
        assert_eq!(series.unplayed_count, Some(1));
        assert!(!series.played);
        // A season marked played takes its own count off its series'.
        let season = UserState {
            unplayed_count: Some(1),
            ..UserState::default()
        };
        estimate_parent(&mut series, &season, true);
        assert!(series.played);
        // A season marked unplayed: its series is unplayed, by a count only the server knows.
        let season = UserState {
            played: true,
            unplayed_count: Some(0),
            ..UserState::default()
        };
        estimate_parent(&mut series, &season, false);
        assert!(!series.played);
        assert_eq!(series.unplayed_count, Some(0));
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
