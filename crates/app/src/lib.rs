//! The core's composition root and the state objects the app shells render (ADR 0002, ADR 0004).
//!
//! [`App`] is created once per process. It owns what every feature shares: the request log, the capabilities state and, when opened
//! on a data folder, the settings, the secrets and the device id (ADR 0013). Server setup ([`AddServer`]) finds a server; signing in
//! ([`SignIn`]) saves it with its user, and the sessions are restored at the next start (ADR 0015). The crate holds no async runtime:
//! `current-ffi` and the Linux app bring their own, and its async methods expect tokio's.

mod add_server;
mod discovery;
mod list;
mod session;
mod sign_in;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use current_jellyfin::{
    AuthError, ClientIdentity, JellyfinClient, JellyfinIdentity, platform_name,
};
use current_model::{Capabilities, NetworkFacts, SavedServer, ServerAddress, ServerId, ServerRef};
use current_net::{
    NetError, RequestLog, RoamingAddresses, RoamingConfig, RoamingError, RuntimeHandle, Url,
};
use current_store::{DeviceId, LoadReport, Secrets, SettingsStore, StoreError};
use tokio::sync::watch;

pub use add_server::{AddServer, AddServerState, NewServer};
pub use discovery::{DiscoveredServer, Discovery, DiscoveryState, PROBES_AT_ONCE};
pub use list::{ListSubscription, ObservableList, VectorDiff};
pub use sign_in::{SignIn, SignInError, SignInState};

use session::{Shared, Storage, Target};

/// What the about and diagnostics screens show about the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// The core's version, from the workspace manifest.
    pub version: String,
}

/// The core: one per process.
pub struct App {
    shared: Arc<Shared>,
    device_id: Option<DeviceId>,
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

impl std::fmt::Debug for App {
    /// Leaves out the device name (often the computer's name) and the addresses of saved servers.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("stored", &self.shared.storage.is_some())
            .field("servers", &self.shared.servers().len())
            .finish_non_exhaustive()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// A core with no servers configured and nothing stored: empty capabilities and an empty request log. Sign-ins work for the
    /// run, under a device id made for it. For tests, and for the shells until they pass a data folder to [`App::open`].
    pub fn new() -> Self {
        // A random id for this run only. The OS random number generator failing is not worth failing over here: the fallback
        // still names a device, and nothing is stored under it.
        let device_id = DeviceId::generate()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "current-unsaved".to_owned());
        Self {
            shared: Arc::new(Shared::new(
                Arc::new(RequestLog::default()),
                None,
                ClientIdentity::current(platform_name(), &device_id),
                Vec::new(),
            )),
            device_id: None,
            roaming: Mutex::new(Roaming::default()),
        }
    }

    /// A core on the data folder the UI passes in (ADR 0013): loads the settings, creating them and the device id on first start,
    /// and the saved servers, each signed in if it has an active user until [`App::restore_sessions`] checks. `secrets` is the OS
    /// keyring in the apps and memory in tests. `device_name` is what Jellyfin shows for this device: the UI passes the platform's
    /// name for the computer (ADR 0015); an empty one becomes the platform's name. Blocking file I/O: call it at start, off the
    /// async runtime.
    pub fn open(
        data_dir: &Path,
        secrets: Arc<dyn Secrets>,
        device_name: &str,
    ) -> Result<(Self, LoadReport), StoreError> {
        let (settings, report) = SettingsStore::open(data_dir)?;
        let stored = settings.get();
        let identity = ClientIdentity::current(device_name, stored.device_id.as_str());
        let app = Self {
            shared: Arc::new(Shared::new(
                Arc::new(RequestLog::default()),
                Some(Storage { settings, secrets }),
                identity,
                stored.servers,
            )),
            device_id: Some(stored.device_id),
            roaming: Mutex::new(Roaming::default()),
        };
        Ok((app, report))
    }

    /// This install's id, sent to servers as Jellyfin's `DeviceId`. `None` when nothing is stored ([`App::new`]).
    pub fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
    }

    /// The settings store. `None` when nothing is stored ([`App::new`]).
    pub fn settings(&self) -> Option<&SettingsStore> {
        self.shared.storage.as_ref().map(|s| &s.settings)
    }

    /// Where secrets are kept. `None` when nothing is stored ([`App::new`]).
    pub fn secrets(&self) -> Option<&Arc<dyn Secrets>> {
        self.shared.storage.as_ref().map(|s| &s.secrets)
    }

    pub fn info(&self) -> AppInfo {
        AppInfo {
            version: current_model::version().to_owned(),
        }
    }

    /// The log every server client writes to.
    pub fn request_log(&self) -> &Arc<RequestLog> {
        &self.shared.request_log
    }

    /// The capabilities state: the current value now, and every change after. One status per saved server.
    pub fn capabilities(&self) -> watch::Receiver<Capabilities> {
        self.shared.capabilities.subscribe()
    }

    /// Replaces the capabilities state. Subscribers see the new value; an equal value notifies no one.
    pub fn set_capabilities(&self, capabilities: Capabilities) {
        self.shared.capabilities.send_if_modified(|current| {
            if *current == capabilities {
                false
            } else {
                *current = capabilities;
                true
            }
        });
    }

    /// The saved servers, in the order they were added.
    pub fn servers(&self) -> Vec<SavedServer> {
        self.shared.servers()
    }

    /// The saved servers now, and every change after (a sign-in, a sign-out, the 401 rule, a restore that learns a new user name).
    /// An unchanged list notifies no one.
    pub fn saved_servers(&self) -> watch::Receiver<Vec<SavedServer>> {
        self.shared.servers.subscribe()
    }

    /// The signed-in client of a server, for the features that browse and play.
    pub fn jellyfin(&self, server: &ServerRef) -> Option<Arc<JellyfinClient>> {
        self.shared.session(server)
    }

    /// A new "add a server" flow. Its probes go to the shared request log.
    pub fn add_server(&self) -> AddServer {
        AddServer::new(self.shared.request_log.clone())
    }

    /// A sign-in to the server [`AddServer`] found (its `Ready` state). Success saves the server, or adds the address and user to
    /// the saved server with the same id. Its requests are logged as `jellyfin/setup`.
    pub fn sign_in(&self, server: NewServer) -> Result<SignIn, NetError> {
        let setup = ServerRef::new(
            current_model::ServiceType::JELLYFIN,
            current_model::ServerKey::new("setup"),
        );
        let client = self.shared.client(setup, &server.address)?;
        let expected = server.info.id.clone();
        let target = Target::New {
            id: server.info.id,
            name: server.info.name,
            address: server.address,
            version: server.info.version,
        };
        Ok(SignIn::new(
            self.shared.clone(),
            target,
            client,
            expected,
            None,
        ))
    }

    /// A sign-in to a saved server, at its first address. Each attempt first checks that the address answers as the saved server
    /// (ADR 0014) before any credential is sent. `None` if no such server is saved or it has no address.
    pub fn sign_in_again(&self, server: &ServerRef) -> Option<Result<SignIn, NetError>> {
        let saved = self.shared.saved(server)?;
        let address = saved.addresses.first()?;
        let Ok(verify) = Url::parse(&address.url) else {
            return Some(Err(NetError::InvalidRequest));
        };
        Some(self.shared.client(server.clone(), address).map(|client| {
            SignIn::new(
                self.shared.clone(),
                Target::Saved(server.clone()),
                client,
                saved.id.clone(),
                Some(verify),
            )
        }))
    }

    /// Checks every saved session, all servers at once; the shell runs it in the background after [`App::open`], and may run it
    /// again (for example when the network returns) for servers without a live session. For each server with an active user: the
    /// token from the keyring, then the identity probe on its address (the token goes only to the same server, ADR 0014), then the
    /// current user. The capabilities state follows: signed in with the user and version, signed out when the token is gone or the
    /// server answers 401, unreachable (still signed in) when the server does not answer.
    pub async fn restore_sessions(&self) {
        self.shared.restore_all().await;
    }

    /// Signs out of a server: tells the server, then deletes the token and the active user whatever it answered. Returns the
    /// server's answer, for the UI to mention that the server could not be told; the app is signed out either way.
    pub async fn sign_out(&self, server: &ServerRef) -> Result<(), AuthError> {
        self.shared.sign_out(server).await
    }

    /// The platform's report of the network, on start and on every change (ADR 0014). Every server's address provider hears it, and
    /// providers made later start from it. Call it from one thread (the UI's network observer): the providers hear the facts after
    /// the lock is released, so two concurrent reports could reach a provider in either order.
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
            self.shared.request_log.clone(),
            RoamingConfig::default(),
            runtime,
        )?;
        let facts = {
            let mut roaming = self.lock_roaming();
            roaming
                .providers
                .retain(|provider| provider.strong_count() > 0);
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

    /// A new search for servers on the local network, for the "add a server" screen (ADR 0016). Its probes go to the shared
    /// request log.
    pub fn discover_servers(&self) -> Discovery {
        Discovery::new(self.shared.request_log.clone())
    }

    /// The request log as text, oldest first: what "copy diagnostics" puts on the clipboard. Redacted by construction (ADR 0006).
    pub fn diagnostics_text(&self) -> String {
        self.shared.request_log.to_text()
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
        let (app, report) = App::open(temp.path(), secrets.clone(), "Office PC").unwrap();
        assert_eq!(report, LoadReport::Created);
        let id = app.device_id().unwrap().clone();
        assert_eq!(app.settings().unwrap().get().device_id, id);
        assert!(app.secrets().is_some());
        drop(app);

        let (app, report) = App::open(temp.path(), secrets, "Office PC").unwrap();
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
