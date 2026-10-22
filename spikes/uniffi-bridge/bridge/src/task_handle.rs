//! The handle a UI keeps for each subscription or background job.

use std::sync::Mutex;

use tokio::task::JoinHandle;

/// Owns one core task. `cancel()` aborts it; so does dropping the handle (C# `Dispose()`/finaliser, Swift `deinit`).
/// Aborting drops the task's future, and with it the listener it holds: that is what releases the foreign object.
#[derive(uniffi::Object)]
pub struct TaskHandle {
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TaskHandle {
    pub(crate) fn new(task: JoinHandle<()>) -> Self {
        Self {
            task: Mutex::new(Some(task)),
        }
    }
}

#[uniffi::export]
impl TaskHandle {
    /// Idempotent. The task stops at its next await point; no callback starts after `cancel()` returns on the
    /// task's own thread, but one already running on a worker thread may still finish.
    pub fn cancel(&self) {
        // A poisoned lock still holds a valid Option.
        if let Some(task) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.abort();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
