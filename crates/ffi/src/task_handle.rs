//! The handle a subscription returns (ADR 0004): the UI keeps it while it listens and cancels it when its view goes away.

use tokio::task::JoinHandle;

/// A task the core runs for the UI. Cancelling it, or dropping the last reference, aborts the task, which drops its listener and so
/// frees the foreign object.
#[derive(Debug, uniffi::Object)]
pub struct TaskHandle {
    task: JoinHandle<()>,
}

impl TaskHandle {
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self { task }
    }
}

#[uniffi::export]
impl TaskHandle {
    /// Stops the task at its next await. A callback already running may still finish, so a UI must tolerate one late delivery.
    pub fn cancel(&self) {
        self.task.abort();
    }

    /// Whether the task has ended, by itself or by `cancel`. Right after `cancel` it may still be running for a moment.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}
