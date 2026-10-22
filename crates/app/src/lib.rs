//! The core's composition root and the state objects the app shells render (ADR 0002, ADR 0004).
//!
//! [`App`] is created once per process. It owns what every feature shares: the request log, the capabilities state and, when opened
//! on a data folder, the settings, the secrets and the device id (ADR 0013). Server setup ([`AddServer`]) builds on it; sign-in (M1)
//! adds to it. The crate holds no async runtime: `current-ffi` and the Linux app bring their own.

mod add_server;
mod discovery;
mod list;

use std::path::Path;
use std::sync::Arc;

use current_model::Capabilities;
use current_net::RequestLog;
use current_store::{DeviceId, LoadReport, Secrets, SettingsStore, StoreError};
use tokio::sync::watch;

pub use add_server::{AddServer, AddServerState, NewServer};
pub use discovery::{DiscoveredServer, Discovery, DiscoveryState, PROBES_AT_ONCE};
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
    storage: Option<Storage>,
}

/// What an app opened on a data folder keeps between runs.
#[derive(Debug)]
struct Storage {
    settings: SettingsStore,
    secrets: Arc<dyn Secrets>,
    device_id: DeviceId,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// A core with no servers configured and nothing stored: empty capabilities and an empty request log. For tests, and for the
    /// shells until they pass a data folder to [`App::open`].
    pub fn new() -> Self {
        Self {
            request_log: Arc::new(RequestLog::default()),
            capabilities: watch::Sender::new(Capabilities::default()),
            storage: None,
        }
    }

    /// A core on the data folder the UI passes in (ADR 0013): loads the settings, creating them and the device id on first start.
    /// `secrets` is the OS keyring in the apps and memory in tests. Blocking file I/O: call it at start, off the async runtime.
    pub fn open(
        data_dir: &Path,
        secrets: Arc<dyn Secrets>,
    ) -> Result<(Self, LoadReport), StoreError> {
        let (settings, report) = SettingsStore::open(data_dir)?;
        let device_id = settings.get().device_id;
        let app = Self {
            storage: Some(Storage {
                settings,
                secrets,
                device_id,
            }),
            ..Self::new()
        };
        Ok((app, report))
    }

    /// This install's id, sent to servers as Jellyfin's `DeviceId`. `None` when nothing is stored ([`App::new`]).
    pub fn device_id(&self) -> Option<&DeviceId> {
        self.storage.as_ref().map(|s| &s.device_id)
    }

    /// The settings store. `None` when nothing is stored ([`App::new`]).
    pub fn settings(&self) -> Option<&SettingsStore> {
        self.storage.as_ref().map(|s| &s.settings)
    }

    /// Where secrets are kept. `None` when nothing is stored ([`App::new`]).
    pub fn secrets(&self) -> Option<&Arc<dyn Secrets>> {
        self.storage.as_ref().map(|s| &s.secrets)
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

    /// A new "add a server" flow. Its probes go to the shared request log.
    pub fn add_server(&self) -> AddServer {
        AddServer::new(self.request_log.clone())
    }

    /// A new search for servers on the local network, for the "add a server" screen (ADR 0016). Its probes go to the shared
    /// request log.
    pub fn discover_servers(&self) -> Discovery {
        Discovery::new(self.request_log.clone())
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
    use current_store::MemorySecrets;

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

    #[tokio::test]
    async fn server_setup_probes_go_to_the_shared_log() {
        let app = App::new();
        app.add_server().probe("http://127.0.0.1:1").await;
        let text = app.diagnostics_text();
        assert!(
            text.contains("jellyfin/setup GET /System/Info/Public -> unreachable"),
            "{text}"
        );
    }

    #[test]
    fn a_new_app_stores_nothing() {
        let app = App::new();
        assert!(app.device_id().is_none());
        assert!(app.settings().is_none());
        assert!(app.secrets().is_none());
    }

    #[test]
    fn open_loads_the_settings_and_keeps_the_device_id() {
        let temp = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn Secrets> = Arc::new(MemorySecrets::new());
        let (app, report) = App::open(temp.path(), secrets.clone()).unwrap();
        assert_eq!(report, LoadReport::Created);
        let id = app.device_id().unwrap().clone();
        assert_eq!(app.settings().unwrap().get().device_id, id);
        assert!(app.secrets().is_some());
        drop(app);

        let (app, report) = App::open(temp.path(), secrets).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(app.device_id(), Some(&id));
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
