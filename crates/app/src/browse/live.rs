//! Live updates of the items' user state (ADR 0023, proposed): one WebSocket per browse session, whose `UserDataChanged` go into
//! the session's store like any server answer ([`super::store::ItemStore::answer`], newest wins, wanted changes on top), so a mark
//! made on another device, by playback or by the server shows on every page without a refresh.
//!
//! - **Lifetime**: from the session's first `Browse` until its last list is dropped or the session ends (sign-out, removal, a new
//!   sign-in). The app has no signal for going to the background yet, so the socket stays open there too (a follow-up).
//! - **Keep-alive**: Jellyfin drops a socket that sends no `KeepAlive` within the time its `ForceKeepAlive` gives (60 s in 12.3);
//!   one goes at once and then every half of it, and the server answers each. A socket that hears nothing for twice that time is
//!   taken for lost and opened again.
//! - **Reconnect**: after a broken or failed connection, with backoff: 1 s doubling to 60 s, with full jitter, back to 1 s once a
//!   socket lived 60 s; at once when the app returns to the foreground or the server's status changes (another address, reached
//!   again). A refusal (Jellyfin's, a gateway's challenge or sign-in page, no token) is not retried until then: the session's own
//!   requests and the sign-in and gateway flows deal with it.
//! - **Resync**: after each connection opens, the items the lists hold read their state again ([`Context::resync`], as on focus),
//!   for what changed while it was down.

use std::hash::BuildHasher;
use std::sync::{Arc, Weak};
use std::time::Duration;

use current_jellyfin::{AuthError, LiveMessage};
use current_model::ServerStatus;
use tokio::time::Instant;

use super::{Context, RESYNC_LIMIT};

/// The first wait before a reconnect, doubled after each failure up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long a socket must live for the backoff to start from [`MIN_BACKOFF`] again.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// The keep-alive time until the server's `ForceKeepAlive` says (Jellyfin 12.3's `WebSocketLostTimeout`).
const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(60);

/// How a connection ended.
enum Ended {
    /// The session or the browse state is gone: the task stops.
    Gone,
    /// Refused: wait for the foreground or a status change.
    Refused,
    /// Broken or not reached: try again after the backoff. `lived` is how long it was open.
    Broken { lived: Duration },
}

/// What of the server's status makes the socket try again at once: another address, reached again, a gateway signed in to.
fn status_key(status: Option<&ServerStatus>) -> Option<(Option<u8>, bool, bool)> {
    status.map(|status| {
        (
            status.address,
            status.is_available(),
            status.gateway_sign_in_needed,
        )
    })
}

impl Context {
    /// Starts the session's live updates. The task holds the context weakly: it ends with it.
    pub(super) fn start_live(self: &Arc<Self>) {
        let context = Arc::downgrade(self);
        let task = self.spawn(async move { run(context).await });
        *super::lock(&self.live) = Some(task);
    }
}

async fn run(context: Weak<Context>) {
    let mut backoff = MIN_BACKOFF;
    let mut attempt: u64 = 0;
    loop {
        let (ended, mut status) = {
            let Some(strong) = context.upgrade() else {
                return;
            };
            let Some(shared) = strong.shared.upgrade() else {
                return;
            };
            let status = shared.capabilities.subscribe();
            drop(strong);
            (connect(&context).await, status)
        };
        let wait = match ended {
            Ended::Gone => return,
            Ended::Refused => None,
            Ended::Broken { lived } => {
                if lived >= STABLE_AFTER {
                    backoff = MIN_BACKOFF;
                }
                attempt += 1;
                // Full jitter, from the standard library's random hasher keys: no random crate.
                let jitter = std::collections::hash_map::RandomState::new().hash_one(attempt);
                let wait = backoff.mul_f64((jitter % 1000) as f64 / 1000.0);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                Some(wait)
            }
        };
        let Some(strong) = context.upgrade() else {
            return;
        };
        let server = strong.server.clone();
        let wake = strong.live_wake.clone();
        drop(strong);
        let before = status_key(status.borrow_and_update().server(&server));
        let changed = async {
            loop {
                if status.changed().await.is_err() {
                    return;
                }
                if status_key(status.borrow_and_update().server(&server)) != before {
                    return;
                }
            }
        };
        let sleep = async {
            match wait {
                Some(wait) => tokio::time::sleep(wait).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = sleep => {}
            () = changed => backoff = MIN_BACKOFF,
            () = wake.notified() => backoff = MIN_BACKOFF,
        }
    }
}

/// One connection, from opening to its end.
async fn connect(context: &Weak<Context>) -> Ended {
    let client = {
        let Some(context) = context.upgrade() else {
            return Ended::Gone;
        };
        if !context.is_current() {
            return Ended::Gone;
        }
        match context.client() {
            Ok(client) => client,
            Err(_) => return Ended::Gone,
        }
    };
    let mut socket = match client.live().await {
        Ok(socket) => socket,
        Err(AuthError::NotSignedIn | AuthError::Refused | AuthError::Gateway(_)) => {
            return Ended::Refused;
        }
        Err(_) => {
            return Ended::Broken {
                lived: Duration::ZERO,
            };
        }
    };
    let opened = Instant::now();
    let broken = || Ended::Broken {
        lived: opened.elapsed(),
    };
    {
        let Some(context) = context.upgrade() else {
            return Ended::Gone;
        };
        // What changed while no socket was open.
        let held: Vec<_> = context.held().into_iter().take(RESYNC_LIMIT).collect();
        context.resync(held);
    }
    let mut keep_alive = DEFAULT_KEEP_ALIVE;
    let mut heard = Instant::now();
    let mut next_keep_alive = Instant::now() + keep_alive / 2;
    loop {
        let lost = heard + keep_alive * 2;
        let message = tokio::select! {
            message = socket.next() => message,
            () = tokio::time::sleep_until(next_keep_alive) => {
                if socket.keep_alive().await.is_err() {
                    return broken();
                }
                next_keep_alive = Instant::now() + keep_alive / 2;
                continue;
            }
            () = tokio::time::sleep_until(lost) => return broken(),
        };
        let Ok(message) = message else {
            return broken();
        };
        heard = Instant::now();
        let Some(context) = context.upgrade() else {
            return Ended::Gone;
        };
        if !context.is_current() {
            return Ended::Gone;
        }
        match message {
            LiveMessage::ForceKeepAlive(time) => {
                keep_alive = time;
                if socket.keep_alive().await.is_err() {
                    return broken();
                }
                next_keep_alive = Instant::now() + keep_alive / 2;
            }
            LiveMessage::UserData(changed) => context.pushed(changed),
            LiveMessage::Other => {}
        }
    }
}

impl Context {
    /// The server says these items' user data changed: each goes into the store as an answer that arrived now, and every list
    /// shows it; the lists chosen by marks (Home's rows, the Favourites tab, a filtered grid) reload a moment later, as after a
    /// mark, since an item may join or leave them.
    fn pushed(self: &Arc<Self>, changed: Vec<(current_model::ItemRef, current_model::UserState)>) {
        if changed.is_empty() {
            return;
        }
        let at = self.tick();
        for (item, state) in changed {
            if self.store.answer(&item, state, at) {
                self.publish(&item);
            }
        }
        // No item of its own: a list that merely shows a pushed item has its state already; only those chosen by marks read again.
        self.dependents_changed(Vec::new());
    }
}
