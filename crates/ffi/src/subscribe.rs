//! The two observable shapes (ADR 0004): values pumped from a `watch` channel and lists pumped as batches of diffs. Each runs as one
//! task on the core's runtime per subscription, so a listener's callbacks are ordered and run on a core worker thread.
//!
//! UniFFI turns an exception thrown by a foreign callback into a Rust panic (ADR 0004). Each delivery is guarded so that a failing
//! callback costs that delivery only, not the subscription: the next value still goes out, and a list's next batch is a fresh Reset.

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
            // The listener's own state is foreign and outlives a failed call; nothing of ours is left half-changed.
            let _ = catch_unwind(AssertUnwindSafe(|| deliver(value)));
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
            // A lost batch leaves the UI's mirror behind: send it the whole list next.
            if catch_unwind(AssertUnwindSafe(|| deliver(batch))).is_err() {
                subscription.request_reset();
            }
        }
    })))
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
