//! The user's libraries for the app's navigation, as one value per session that follows the server (found on the owner's run,
//! 2026-10-25: a library added on the server showed on Home but not in the navigation until a restart).
//!
//! The list loads when the session's browse state is made, and loads again on a refresh, on the foreground refresh, when Home
//! reloads, after a library request the server refused ("not available"), and when the server is reached again after a load
//! failed. It keeps the user's order and passes unknown collection kinds through, as `GET UserViews` gives them. A failed load
//! keeps the list as it was. Loads never overlap: asking while one runs runs one more after it, and Home joins the one running.

use std::sync::{Arc, Mutex, Weak};

use current_jellyfin::AuthError;
use current_model::{Capabilities, ItemRef, Library, Reachability, ServerRef};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::{Context, lock};

/// A load that ended: its number and its result.
#[derive(Clone)]
struct Ended {
    number: u64,
    result: Result<Vec<Library>, AuthError>,
}

#[derive(Default)]
struct Loads {
    /// The number of the last load asked for. Loads are numbered from 1.
    wanted: u64,
    /// The number of the last load that ended.
    done: u64,
    running: bool,
    /// Whether the last load failed: the server's being reached again loads once more.
    failed: bool,
}

/// One session's libraries.
pub(super) struct Libraries {
    /// `None` until a load succeeded.
    list: watch::Sender<Option<Vec<Library>>>,
    loads: Mutex<Loads>,
    ended: watch::Sender<Ended>,
    /// The task that loads again once the server is reached after a failure.
    follower: Mutex<Option<AbortHandle>>,
}

impl Default for Libraries {
    fn default() -> Self {
        Self {
            list: watch::Sender::new(None),
            loads: Mutex::new(Loads::default()),
            ended: watch::Sender::new(Ended {
                number: 0,
                result: Err(AuthError::NotSignedIn),
            }),
            follower: Mutex::new(None),
        }
    }
}

impl Drop for Libraries {
    fn drop(&mut self) {
        if let Some(follower) = lock(&self.follower).take() {
            follower.abort();
        }
    }
}

impl Libraries {
    pub(super) fn subscribe(&self) -> watch::Receiver<Option<Vec<Library>>> {
        self.list.subscribe()
    }
}

impl Context {
    /// Starts loading the libraries, and once the server is reached again after a failed load, loads again. Called once, when the
    /// session's browse state is made.
    pub(super) fn start_libraries(self: &Arc<Self>, capabilities: watch::Receiver<Capabilities>) {
        self.reload_libraries();
        let follower = self.spawn(follow(
            Arc::downgrade(self),
            self.server.clone(),
            capabilities,
        ));
        *lock(&self.libraries.follower) = Some(follower);
    }

    /// Loads the libraries again: a load that starts after this call. Returns its number.
    pub(super) fn reload_libraries(self: &Arc<Self>) -> u64 {
        let mut loads = lock(&self.libraries.loads);
        loads.wanted += 1;
        let number = loads.wanted;
        if !loads.running {
            loads.running = true;
            let context = self.clone();
            self.spawn(async move { context.load_libraries().await });
        }
        number
    }

    /// The libraries as a load that starts after this call reads them.
    pub(super) async fn libraries_again(self: &Arc<Self>) -> Result<Vec<Library>, AuthError> {
        let number = self.reload_libraries();
        self.library_load(number).await
    }

    /// The libraries as the load running or asked for reads them, or as a new load does when none is: Home's reload joins a refresh's.
    pub(super) async fn libraries_joined(self: &Arc<Self>) -> Result<Vec<Library>, AuthError> {
        let pending = {
            let loads = lock(&self.libraries.loads);
            (loads.wanted > loads.done).then_some(loads.wanted)
        };
        let number = match pending {
            Some(number) => number,
            None => self.reload_libraries(),
        };
        self.library_load(number).await
    }

    /// The result of load `number`, or of a later one.
    async fn library_load(&self, number: u64) -> Result<Vec<Library>, AuthError> {
        let mut ended = self.libraries.ended.subscribe();
        match ended.wait_for(|ended| ended.number >= number).await {
            Ok(ended) => ended.result.clone(),
            Err(_) => Err(AuthError::NotSignedIn),
        }
    }

    /// Runs loads until none is wanted.
    async fn load_libraries(self: Arc<Self>) {
        loop {
            let number = {
                let mut loads = lock(&self.libraries.loads);
                if loads.done >= loads.wanted {
                    loads.running = false;
                    return;
                }
                loads.wanted
            };
            let result = match self.client() {
                Ok(client) => client.libraries().await,
                Err(error) => Err(error),
            };
            if let Ok(libraries) = &result {
                self.libraries.list.send_if_modified(|list| {
                    let changed = list.as_ref() != Some(libraries);
                    *list = Some(libraries.clone());
                    changed
                });
            }
            let mut loads = lock(&self.libraries.loads);
            loads.done = number;
            loads.failed = result.is_err();
            self.libraries.ended.send_replace(Ended { number, result });
        }
    }

    /// Whether `library` is gone from the user's libraries, read again now: a request under it failed as one does for a
    /// library the server no longer lists ([`might_be_gone`]). A failed load says it is not gone.
    pub(super) async fn library_gone(self: &Arc<Self>, library: &ItemRef) -> bool {
        self.libraries_again()
            .await
            .is_ok_and(|libraries| libraries.iter().all(|known| &known.item != library))
    }
}

/// Whether a request under a library failed as it does once the library is gone: Jellyfin 12.2 answers `GET Items` with 401 for
/// a parent the user cannot see (a valid token then answers [`AuthError::NotAvailable`]) and 400 for a parent it does not know
/// (ADR 0015 as amended 2026-10-25); 404 for an item it cannot find.
pub(super) fn might_be_gone(error: &AuthError) -> bool {
    matches!(
        error,
        AuthError::NotAvailable
            | AuthError::Network(current_net::NetError::Status {
                status: 400 | 404,
                ..
            })
    )
}

/// Loads the libraries again each time the server's status changes while it is reachable after a failed load, until the
/// session's browse state is gone. Another server's changes do not count.
async fn follow(
    context: Weak<Context>,
    server: ServerRef,
    mut capabilities: watch::Receiver<Capabilities>,
) {
    let mut last = capabilities.borrow_and_update().server(&server).cloned();
    while capabilities.changed().await.is_ok() {
        let status = capabilities.borrow_and_update().server(&server).cloned();
        if status == last {
            continue;
        }
        last.clone_from(&status);
        let reachable = status.is_some_and(|status| status.reachability == Reachability::Reachable);
        let Some(context) = context.upgrade() else {
            return;
        };
        let failed = {
            let loads = lock(&context.libraries.loads);
            loads.failed && !loads.running
        };
        if reachable && failed {
            context.reload_libraries();
        }
    }
}
