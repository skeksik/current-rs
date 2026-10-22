//! The core's composition root and the state objects the app shells render (ADR 0002, ADR 0004).
//!
//! [`App`] is created once per process. It owns what every feature shares: the request log and the capabilities state. Server setup,
//! sign-in and storage (M1) add to it. The crate holds no async runtime: `current-ffi` and the Linux app bring their own.

mod list;

use std::sync::Arc;

use current_model::Capabilities;
use current_net::RequestLog;
use tokio::sync::watch;

pub use list::{ListSubscription, ObservableList, VectorDiff};

/// What the about and diagnostics screens show about the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// The core's version, from the workspace manifest.
    pub version: String,
}

/// The core: one per process.
#[derive(Debug)]
pub struct App {
    request_log: Arc<RequestLog>,
    capabilities: watch::Sender<Capabilities>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// A core with no servers configured: empty capabilities and an empty request log.
    pub fn new() -> Self {
        Self {
            request_log: Arc::new(RequestLog::default()),
            capabilities: watch::Sender::new(Capabilities::default()),
        }
    }

    pub fn info(&self) -> AppInfo {
        AppInfo {
            version: current_model::version().to_owned(),
        }
    }

    /// The log every server client writes to.
    pub fn request_log(&self) -> &Arc<RequestLog> {
        &self.request_log
    }

    /// The capabilities state: the current value now, and every change after. Empty until server setup exists (M1).
    pub fn capabilities(&self) -> watch::Receiver<Capabilities> {
        self.capabilities.subscribe()
    }

    /// Replaces the capabilities state. Subscribers see the new value; an equal value notifies no one.
    pub fn set_capabilities(&self, capabilities: Capabilities) {
        self.capabilities.send_if_modified(|current| {
            if *current == capabilities {
                false
            } else {
                *current = capabilities;
                true
            }
        });
    }

    /// The request log as text, oldest first: what "copy diagnostics" puts on the clipboard. Redacted by construction (ADR 0006).
    pub fn diagnostics_text(&self) -> String {
        self.request_log.to_text()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use current_model::{ServerKey, ServerRef, ServerStatus, ServiceType};
    use current_net::{LogEntry, Outcome};

    use super::*;

    fn jellyfin() -> ServerRef {
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
    }

    #[test]
    fn info_has_the_workspace_version() {
        assert_eq!(App::new().info().version, "0.1.0");
    }

    #[test]
    fn capabilities_start_empty() {
        assert!(App::new().capabilities().borrow().servers.is_empty());
    }

    #[tokio::test]
    async fn subscribers_see_changes() {
        let app = App::new();
        let mut rx = app.capabilities();
        let next = Capabilities {
            servers: vec![ServerStatus::new(jellyfin())],
        };
        app.set_capabilities(next.clone());
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), next);
    }

    #[test]
    fn an_equal_value_notifies_no_one() {
        let app = App::new();
        let rx = app.capabilities();
        app.set_capabilities(Capabilities::default());
        assert!(!rx.has_changed().unwrap());
    }

    #[test]
    fn diagnostics_are_the_request_log() {
        let app = App::new();
        assert_eq!(app.diagnostics_text(), "");
        app.request_log().push(LogEntry {
            at: SystemTime::UNIX_EPOCH,
            server: jellyfin(),
            method: "GET".into(),
            path: "/System/Info".into(),
            query: Vec::new(),
            outcome: Outcome::Status(200),
            duration: Duration::from_millis(12),
        });
        assert_eq!(
            app.diagnostics_text(),
            "1970-01-01T00:00:00.000Z jellyfin/1 GET /System/Info -> 200 (12 ms)\n"
        );
    }
}
