//! The handle a subscription returns (ADR 0004): the UI keeps it while it listens and cancels it when its view goes away.

use std::sync::{Mutex, MutexGuard};

use tokio::task::JoinHandle;

/// A task the core runs for the UI. Cancelling it, or dropping the last reference, aborts the task, which drops its listener and so
/// frees the foreign object.
#[derive(Debug, uniffi::Object)]
pub struct TaskHandle {
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TaskHandle {
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self {
            task: Mutex::new(Some(task)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        // The guarded value is only taken or read, never left half-changed: a poisoned lock is safe.
        self.task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[uniffi::export]
impl TaskHandle {
    /// Stops the task at its next await. A callback already running may still finish, so a UI must tolerate one late delivery.
    pub fn cancel(&self) {
        if let Some(task) = self.lock().take() {
            task.abort();
        }
    }

    /// Whether the task has ended, by itself or by `cancel`.
    pub fn is_finished(&self) -> bool {
        self.lock().as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
