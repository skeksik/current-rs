//! The core's composition root and the state objects the app shells render (ADR 0002, ADR 0004).
//!
//! [`App`] is created once per process. It owns what every feature shares: the request log, the capabilities state and, when opened
//! on a data folder, the settings, the secrets and the device id (ADR 0013). Server setup ([`AddServer`]) builds on it; sign-in (M1)
//! adds to it. The crate holds no async runtime: `current-ffi` and the Linux app bring their own.

mod add_server;
mod list;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use current_jellyfin::JellyfinIdentity;
use current_model::{Capabilities, NetworkFacts, ServerAddress, ServerId, ServerRef};
use current_net::{RequestLog, RoamingAddresses, RoamingConfig, RoamingError, RuntimeHandle};
use current_store::{DeviceId, LoadReport, Secrets, SettingsStore, StoreError};
use tokio::sync::watch;

pub use add_server::{AddServer, AddServerState, NewServer};
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
    roaming: Mutex<Roaming>,
}

/// Every server's address provider, so that network facts reach them all (ADR 0014).
#[derive(Debug, Default)]
struct Roaming {
    /// Dropped providers are pruned on the next fan-out.
    providers: Vec<Weak<RoamingAddresses>>,
    /// The facts last reported, for providers made later.
    facts: Option<NetworkFacts>,
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
            roaming: Mutex::new(Roaming::default()),
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

    /// The platform's report of the network, on start and on every change (ADR 0014). Every server's address provider hears it, and
    /// providers made later start from it.
    pub fn network_changed(&self, facts: NetworkFacts) {
        let providers = {
            let mut roaming = self.lock_roaming();
            roaming
                .providers
                .retain(|provider| provider.strong_count() > 0);
            roaming.facts = Some(facts.clone());
            roaming.providers.clone()
        };
        for provider in providers.iter().filter_map(Weak::upgrade) {
            provider.network_changed(&facts);
        }
    }

    /// The app came back to the foreground: servers that are unreachable try again at once (ADR 0014).
    pub fn returned_to_foreground(&self) {
        let providers = self.lock_roaming().providers.clone();
        for provider in providers.iter().filter_map(Weak::upgrade) {
            provider.retry_now();
        }
    }

    /// The address provider of a Jellyfin server with id `id` reached at `addresses` (the user's order), for its `ServerClient`. Its
    /// probes check the id and go to the shared request log; its races run on `runtime`. It hears network changes from now on.
    ///
    /// # Errors
    /// [`RoamingError`] for an address that is not an http(s) URL or too many addresses.
    pub fn jellyfin_addresses(
        &self,
        server: ServerRef,
        id: ServerId,
        addresses: &[ServerAddress],
        runtime: &RuntimeHandle,
    ) -> Result<Arc<RoamingAddresses>, RoamingError> {
        let provider = RoamingAddresses::new(
            server,
            addresses,
            Arc::new(JellyfinIdentity::new(id)),
            self.request_log.clone(),
            RoamingConfig::default(),
            runtime,
        )?;
        let facts = {
            let mut roaming = self.lock_roaming();
            roaming.providers.push(Arc::downgrade(&provider));
            roaming.facts.clone()
        };
        if let Some(facts) = facts {
            provider.network_changed(&facts);
        }
        Ok(provider)
    }

    fn lock_roaming(&self) -> std::sync::MutexGuard<'_, Roaming> {
        // Each change is one assignment or one `retain`, so a poisoned lock leaves a usable value.
        self.roaming.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The request log as text, oldest first: what "copy diagnostics" puts on the clipboard. Redacted by construction (ADR 0006).
    pub fn diagnostics_text(&self) -> String {
        self.request_log.to_text()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use current_model::{Connectivity, NetworkKey, ServerKey, ServerStatus, ServiceType};
    use current_net::{AddressProvider, LogEntry, Outcome};
    use current_store::MemorySecrets;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
            address: None,
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

    const ID: &str = "0123456789abcdef0123456789abcdef";

    /// A Jellyfin stand-in on 127.0.0.1 answering every request with its public info.
    async fn jellyfin_stand_in() -> ServerAddress {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = socket.read(&mut buffer).await;
                    let body = format!(
                        r#"{{"Version":"12.2.0","Id":"{ID}","StartupWizardCompleted":true}}"#
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        ServerAddress {
            url: format!("http://127.0.0.1:{port}"),
            plain_http_confirmed: false,
        }
    }

    fn network(key: &str) -> NetworkFacts {
        NetworkFacts {
            connectivity: Connectivity::Full,
            network_key: Some(NetworkKey::new(key)),
            ..NetworkFacts::default()
        }
    }

    async fn selected(provider: &RoamingAddresses) -> Option<u8> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        provider.wait(deadline).await.and_then(|s| s.position())
    }

    #[tokio::test]
    async fn network_changes_reach_every_server() {
        let app = App::new();
        let address = jellyfin_stand_in().await;
        let runtime = RuntimeHandle::current();
        let providers: Vec<_> = (0..2)
            .map(|key| {
                let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key.to_string()));
                app.jellyfin_addresses(
                    server,
                    ServerId::new(ID),
                    std::slice::from_ref(&address),
                    &runtime,
                )
                .unwrap()
            })
            .collect();
        for provider in &providers {
            assert_eq!(selected(provider).await, Some(1));
        }
        app.network_changed(network("away"));
        // Each provider races again before it serves on the new network.
        for provider in &providers {
            assert_eq!(provider.current(), None);
        }
        for provider in &providers {
            assert_eq!(selected(provider).await, Some(1));
        }
        assert!(
            app.diagnostics_text()
                .contains("jellyfin/0 address 1 GET /System/Info/Public -> 200")
        );

        // A dropped provider is pruned.
        drop(providers);
        app.network_changed(network("home"));
        assert!(app.lock_roaming().providers.is_empty());
    }

    #[tokio::test]
    async fn a_provider_made_later_starts_from_the_last_facts() {
        let app = App::new();
        let address = jellyfin_stand_in().await;
        app.network_changed(network("home"));
        let provider = app
            .jellyfin_addresses(
                jellyfin(),
                ServerId::new(ID),
                &[address],
                &RuntimeHandle::current(),
            )
            .unwrap();
        assert_eq!(selected(&provider).await, Some(1));
        // The same facts again are no change for it: it already had them.
        app.network_changed(network("home"));
        assert!(provider.current().is_some());
    }

    #[tokio::test]
    async fn an_invalid_address_is_refused() {
        let app = App::new();
        let address = ServerAddress {
            url: "not an address".into(),
            plain_http_confirmed: false,
        };
        let result = app.jellyfin_addresses(
            jellyfin(),
            ServerId::new(ID),
            &[address],
            &RuntimeHandle::current(),
        );
        assert_eq!(result.map(|_| ()), Err(RoamingError::InvalidAddress(1)));
    }
}
