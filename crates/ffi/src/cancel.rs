//! Explicit cancellation for async calls (ADR 0004). UniFFI does not cancel a Rust future when the foreign task is cancelled, so every
//! cancellable call takes a [`CancelToken`] and each UI wraps it so that its own cancellation cancels the token.

use std::future::Future;
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::CoreError;

/// Cancels one async call. Create one per call; cancelling is final.
#[derive(Debug, uniffi::Object)]
pub struct CancelToken {
    tx: watch::Sender<bool>,
}

#[uniffi::export]
impl CancelToken {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tx: watch::Sender::new(false),
        })
    }

    /// Makes the call end with `CoreError::Cancelled` as soon as it notices.
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }
}

impl CancelToken {
    /// Completes once the token is cancelled.
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        // The sender lives in `self`, so the channel cannot close while this waits.
        let _ = rx.wait_for(|cancelled| *cancelled).await;
    }
}

/// Aborts the spawned work when the future awaiting it is dropped.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs `work` on the core's runtime until it finishes or `token` is cancelled, whichever comes first. A token cancelled before the
/// call wins. Dropping the returned future aborts the work.
pub(crate) async fn run_cancellable<T, F>(
    runtime: &Handle,
    token: Arc<CancelToken>,
    work: F,
) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, CoreError>> + Send + 'static,
{
    let mut task = AbortOnDrop(runtime.spawn(async move {
        tokio::select! {
            biased;
            () = token.cancelled() => Err(CoreError::Cancelled),
            result = work => result,
        }
    }));
    (&mut task.0).await.unwrap_or_else(|error| {
        Err(CoreError::Runtime {
            message: error.to_string(),
        })
    })
}

/// Runs `work` on the core's runtime to its end, whatever happens to the call: `token`, or dropping the returned future, only
/// stops the waiting. For actions whose outcome is the flow's state (a probe with gateway credentials, a save), so that a cancel
/// never leaves that state half-changed.
pub(crate) async fn run_detached<T, F>(
    runtime: &Handle,
    token: Arc<CancelToken>,
    work: F,
) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, CoreError>> + Send + 'static,
{
    // Dropping a JoinHandle does not abort its task: `work` runs on when the waiting below ends first.
    let work = runtime.spawn(work);
    let mut waiting = AbortOnDrop(runtime.spawn(async move {
        tokio::select! {
            biased;
            () = token.cancelled() => Err(CoreError::Cancelled),
            result = work => result.unwrap_or_else(|error| Err(CoreError::Runtime {
                message: error.to_string(),
            })),
        }
    }));
    (&mut waiting.0).await.unwrap_or_else(|error| {
        Err(CoreError::Runtime {
            message: error.to_string(),
        })
    })
}
