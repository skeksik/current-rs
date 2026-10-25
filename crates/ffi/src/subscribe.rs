//! The two observable shapes (ADR 0004): values pumped from a `watch` channel and lists pumped as batches of diffs. Each runs as one
//! task on the core's runtime per subscription. The task never calls the listener itself: every delivery runs on the runtime's
//! blocking pool and the task awaits it before taking the next value, so a listener's callbacks are ordered, one at a time, while
//! a slow or blocked listener holds a pool thread, never one of the two workers every core task shares.
//!
//! UniFFI turns an exception thrown by a foreign callback into a Rust panic (ADR 0004), and the process's panic hook runs first, on
//! the thread that called the listener: a pool thread too, so a costly hook (a backtrace under `RUST_BACKTRACE`) stalls that one
//! subscription only. Each delivery is guarded so that a failing callback costs that delivery only, not the subscription: the next
//! value still goes out, and a list's next batch is a fresh Reset.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use current_app::{ListSubscription, VectorDiff};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::TaskHandle;

/// Sends the current value, then every change, until the handle is cancelled or dropped or the value's owner goes away. A slow
/// listener skips intermediate values rather than queueing them.
pub(crate) fn watch_value<T, F>(
    runtime: &Handle,
    mut rx: watch::Receiver<T>,
    mut deliver: F,
) -> Arc<TaskHandle>
where
    T: Clone + Send + Sync + 'static,
    F: FnMut(T) + Send + 'static,
{
    Arc::new(TaskHandle::new(runtime.spawn(async move {
        loop {
            // Clone first so the callback never runs while the channel's lock is held.
            let value = rx.borrow_and_update().clone();
            let Some((returned, _)) = deliver_on_pool(deliver, value).await else {
                break;
            };
            deliver = returned;
            if rx.changed().await.is_err() {
                break;
            }
        }
    })))
}

/// Sends `[Reset]`, then batches of diffs, until the handle is cancelled or dropped or the list goes away.
pub(crate) fn watch_list<T, F>(
    runtime: &Handle,
    mut subscription: ListSubscription<T>,
    mut deliver: F,
) -> Arc<TaskHandle>
where
    T: Clone + Send + 'static,
    F: FnMut(Vec<VectorDiff<T>>) + Send + 'static,
{
    Arc::new(TaskHandle::new(runtime.spawn(async move {
        while let Some(batch) = subscription.next_batch().await {
            let Some((returned, delivered)) = deliver_on_pool(deliver, batch).await else {
                break;
            };
            deliver = returned;
            // A lost batch leaves the UI's mirror behind: send it the whole list next.
            if !delivered {
                subscription.request_reset();
            }
        }
    })))
}

/// Calls the listener once on the runtime's blocking pool and hands it back with whether the call returned normally. `None` only if
/// the pool refused the work (the runtime is shutting down): the subscription ends.
///
/// The listener moves to the pool thread and back, so while a delivery runs the subscription's task holds nothing of it: cancelling
/// the handle then lets that delivery finish, drops the listener on the pool thread, and sends nothing more.
async fn deliver_on_pool<T, F>(mut deliver: F, value: T) -> Option<(F, bool)>
where
    T: Send + 'static,
    F: FnMut(T) + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        // The listener's own state is foreign and outlives a failed call; nothing of ours is left half-changed.
        let delivered = catch_unwind(AssertUnwindSafe(|| deliver(value))).is_ok();
        (deliver, delivered)
    })
    .await
    .ok()
}

/// Counts the foreign listeners the core still holds: a diagnostic for leak checks.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveListeners(Arc<AtomicU64>);

impl LiveListeners {
    pub(crate) fn track<L>(&self, listener: L) -> Tracked<L> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Tracked {
            listener,
            counter: Arc::clone(&self.0),
        }
    }

    pub(crate) fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A listener counted by [`LiveListeners`] until it is dropped.
pub(crate) struct Tracked<L> {
    pub(crate) listener: L,
    counter: Arc<AtomicU64>,
}

impl<L> Drop for Tracked<L> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}
