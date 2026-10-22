//! The UniFFI surface over `current-app`: what the C# and Swift app shells call (ADR 0002, ADR 0004).
//!
//! - One runtime, owned here and started by [`Core::new`].
//! - Observable values: `subscribe_x(listener) -> TaskHandle`. The listener gets the current value first, then every change, on a
//!   core worker thread. Lists use the same shape with batches of diffs (`diff::list_diff!`).
//! - Cancellable calls are `async` and take a [`CancelToken`]; they end with [`CoreError::Cancelled`].
//! - Listener callbacks return nothing and must never throw: each UI's listener catches everything.

mod cancel;
mod diff;
mod error;
mod records;
mod runtime;
mod subscribe;
mod task_handle;

use std::sync::Arc;

use current_app::App;
use tokio::runtime::Handle;

pub use cancel::CancelToken;
pub use error::CoreError;
pub use records::{
    AppInfo, CapabilitiesSnapshot, Reachability, ServerStatusSnapshot, UserSnapshot,
};
pub use runtime::WORKER_THREAD_NAME;
pub use task_handle::TaskHandle;

use subscribe::LiveListeners;

uniffi::setup_scaffolding!();

/// Receives the capabilities state. Called with the current value first, then on every change, on a core worker thread. It must only
/// hand the value to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait CapabilitiesListener: Send + Sync {
    fn on_value(&self, value: CapabilitiesSnapshot);
}

/// The core, created once per process by the app shell.
#[derive(uniffi::Object)]
pub struct Core {
    app: App,
    runtime: Handle,
    listeners: LiveListeners,
}

#[uniffi::export]
impl Core {
    /// Starts the core's runtime on first use.
    ///
    /// # Errors
    /// [`CoreError::Runtime`] if the runtime cannot start.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self {
            app: App::new(),
            runtime: runtime::handle()?,
            listeners: LiveListeners::default(),
        }))
    }

    pub fn app_info(&self) -> AppInfo {
        self.app.info().into()
    }

    /// Subscribes to the capabilities state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_capabilities(
        &self,
        listener: Box<dyn CapabilitiesListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(
            &self.runtime,
            self.app.capabilities(),
            move |capabilities| {
                listener
                    .listener
                    .on_value(CapabilitiesSnapshot::from(&capabilities));
            },
        )
    }

    /// The request log as text, oldest first, redacted (ADR 0006).
    pub fn diagnostics_text(&self) -> String {
        self.app.diagnostics_text()
    }

    /// A diagnostics report for a bug report: the core's version and the request log.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn export_diagnostics(&self, token: Arc<CancelToken>) -> Result<String, CoreError> {
        let version = self.app.info().version;
        let log = self.app.diagnostics_text();
        cancel::run_cancellable(&self.runtime, token, async move {
            Ok(format!(
                "Current {version}\n\nRequests, oldest first:\n{log}"
            ))
        })
        .await
    }

    /// How many listeners the core still holds. A diagnostic for leak checks: it must fall when a view cancels its handles.
    pub fn live_listeners(&self) -> u64 {
        self.listeners.count()
    }
}

#[cfg(test)]
mod tests;
