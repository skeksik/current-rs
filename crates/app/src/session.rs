//! Saved servers and their sessions: what the app remembers, the signed-in clients, restoring them at start, the 401 rule and
//! sign out (ADR 0015).
//!
//! Blocking work (the settings file, the OS keyring) runs on tokio's blocking pool (`spawn_blocking`), so the async methods here
//! need a tokio runtime, as the HTTP client does anyway.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use current_jellyfin::{AccessToken, AuthError, ClientIdentity, JellyfinClient, Prober, SignedIn};
use current_model::{
    Capabilities, Reachability, SavedServer, SavedUser, ServerAddress, ServerId, ServerKey,
    ServerRef, ServerStatus, ServiceType, User, UserId,
};
use current_net::{FixedAddress, NetError, RequestLog, Url};
use current_store::{Secret, SecretKey, Secrets, SettingsStore, StoreError};
use tokio::sync::watch;
use tokio::task::JoinSet;

/// What an app opened on a data folder keeps between runs.
pub(crate) struct Storage {
    pub(crate) settings: SettingsStore,
    pub(crate) secrets: Arc<dyn Secrets>,
}

/// Everything the app's parts share. Held in an `Arc` so that background work (restoring sessions, the 401 hook) can reach it.
pub(crate) struct Shared {
    pub(crate) request_log: Arc<RequestLog>,
    pub(crate) capabilities: watch::Sender<Capabilities>,
    pub(crate) storage: Option<Storage>,
    pub(crate) identity: ClientIdentity,
    /// The saved servers. The settings file mirrors this list: [`Shared::commit`] changes both. Lock order: this list, then the
    /// settings store's own lock.
    servers: Mutex<Vec<SavedServer>>,
    /// One signed-in client per server with a live session.
    sessions: Mutex<HashMap<ServerRef, Arc<JellyfinClient>>>,
}

/// Where a sign-in goes: a server just found by setup, or one already saved.
#[derive(Clone, Debug)]
pub(crate) enum Target {
    New {
        id: ServerId,
        name: String,
        address: ServerAddress,
        version: String,
    },
    Saved(ServerRef),
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every value behind these locks is replaced whole, never left half-changed, so a poisoned lock is still consistent.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn new(
        request_log: Arc<RequestLog>,
        storage: Option<Storage>,
        identity: ClientIdentity,
        servers: Vec<SavedServer>,
    ) -> Self {
        let capabilities = Capabilities {
            servers: servers.iter().map(saved_status).collect(),
        };
        Self {
            request_log,
            capabilities: watch::Sender::new(capabilities),
            storage,
            identity,
            servers: Mutex::new(servers),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn servers(&self) -> Vec<SavedServer> {
        lock(&self.servers).clone()
    }

    pub(crate) fn saved(&self, server: &ServerRef) -> Option<SavedServer> {
        lock(&self.servers)
            .iter()
            .find(|saved| &saved.server == server)
            .cloned()
    }

    pub(crate) fn session(&self, server: &ServerRef) -> Option<Arc<JellyfinClient>> {
        lock(&self.sessions).get(server).cloned()
    }

    /// Changes the saved servers and writes them to the settings file; on a failed write neither changes. Blocking: run it off
    /// the async runtime.
    fn commit<R>(&self, change: impl FnOnce(&mut Vec<SavedServer>) -> R) -> Result<R, StoreError> {
        let mut servers = lock(&self.servers);
        let mut next = servers.clone();
        let result = change(&mut next);
        if let Some(storage) = &self.storage {
            storage
                .settings
                .update(|settings| settings.servers = next.clone())?;
        }
        *servers = next;
        Ok(result)
    }

    /// Changes one server's status in the capabilities state, adding it if it is missing.
    pub(crate) fn update_status(&self, server: &ServerRef, change: impl FnOnce(&mut ServerStatus)) {
        self.capabilities.send_if_modified(|capabilities| {
            let index = match capabilities
                .servers
                .iter()
                .position(|status| &status.server == server)
            {
                Some(index) => index,
                None => {
                    capabilities.servers.push(ServerStatus::new(server.clone()));
                    capabilities.servers.len() - 1
                }
            };
            let before = capabilities.servers[index].clone();
            change(&mut capabilities.servers[index]);
            capabilities.servers[index] != before
        });
    }

    /// A client for a saved server at its first address. Roaming (ADR 0014) replaces the fixed address here.
    pub(crate) fn client(
        &self,
        server: ServerRef,
        address: &ServerAddress,
    ) -> Result<JellyfinClient, NetError> {
        let url = Url::parse(&address.url).map_err(|_| NetError::InvalidRequest)?;
        JellyfinClient::new(
            server,
            Arc::new(FixedAddress(url)),
            self.identity.clone(),
            self.request_log.clone(),
        )
    }

    /// Keeps `client` as the server's session, with the 401 hook installed.
    fn install(self: &Arc<Self>, client: Arc<JellyfinClient>) {
        let server = client.server().clone();
        let shared: Weak<Self> = Arc::downgrade(self);
        let hooked = server.clone();
        client.on_revoked(Arc::new(move || {
            if let Some(shared) = shared.upgrade() {
                shared.revoked(&hooked);
            }
        }));
        lock(&self.sessions).insert(server, client);
    }

    /// The 401 rule (ADR 0015): the session is over. The status changes at once; the saved active user and the stored token are
    /// removed on the blocking pool.
    fn revoked(self: &Arc<Self>, server: &ServerRef) {
        lock(&self.sessions).remove(server);
        self.update_status(server, |status| {
            status.signed_in = false;
            status.user = None;
        });
        let shared = self.clone();
        let server = server.clone();
        let work = move || {
            let user = shared.commit(|servers| {
                servers
                    .iter_mut()
                    .find(|saved| saved.server == server)
                    .and_then(|saved| saved.active_user.take())
            });
            if let (Ok(Some(user)), Some(storage)) = (user, &shared.storage) {
                // Nothing more can be done if the keyring fails: the token no longer works anyway.
                let _ = storage.secrets.delete(&SecretKey::new(server, user));
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(work);
            }
            Err(_) => work(),
        }
    }

    /// Saves a successful sign-in: the server (a new key, or the saved record with the same server id), the user as the active
    /// one, and the token in the keyring. Returns the server and whether the token was remembered: without a keyring the session
    /// lasts for this run only (ADR 0013).
    pub(crate) async fn save_sign_in(
        self: &Arc<Self>,
        target: Target,
        signed_in: SignedIn,
    ) -> Result<(ServerRef, bool), StoreError> {
        let shared = self.clone();
        let user = signed_in.user.clone();
        let token = signed_in.token.clone();
        let (server, address, version, remembered) = blocking(move || {
            let (server, address, version) =
                shared.commit(|servers| record(servers, &target, &user))?;
            let remembered = match &shared.storage {
                Some(storage) => storage
                    .secrets
                    .set(
                        &SecretKey::new(server.clone(), user.id.clone()),
                        &Secret::new(token.expose()),
                    )
                    .is_ok(),
                None => false,
            };
            Ok::<_, StoreError>((server, address, version, remembered))
        })
        .await??;

        let mut user = signed_in.user;
        user.server = server.clone();
        // Building a client fails only for an address that is not a URL, which setup never saves.
        if let Ok(client) = self.client(server.clone(), &address) {
            client.set_token(signed_in.token);
            self.install(Arc::new(client));
        }
        self.update_status(&server, |status| {
            status.signed_in = true;
            status.reachability = Reachability::Reachable;
            status.user = Some(user);
            if version.is_some() {
                status.version = version;
            }
        });
        Ok((server, remembered))
    }

    /// Checks every saved session, all servers at once. See [`App::restore_sessions`](crate::App::restore_sessions).
    pub(crate) async fn restore_all(self: &Arc<Self>) {
        let mut tasks = JoinSet::new();
        for saved in self.servers() {
            if saved.active_user.is_some() && self.session(&saved.server).is_none() {
                let shared = self.clone();
                tasks.spawn(async move { shared.restore(saved).await });
            }
        }
        while tasks.join_next().await.is_some() {}
    }

    async fn restore(self: &Arc<Self>, saved: SavedServer) {
        let server = saved.server.clone();
        let Some(user_id) = saved.active_user.clone() else {
            return;
        };
        let key = SecretKey::new(server.clone(), user_id.clone());
        let token = match &self.storage {
            Some(storage) => {
                let secrets = storage.secrets.clone();
                blocking(move || secrets.get(&key)).await
            }
            None => Ok(Ok(None)),
        };
        let token = match token {
            Ok(Ok(Some(secret))) => AccessToken::new(secret.expose()),
            Ok(Ok(None)) => {
                // Nothing stored (removed from the keyring by hand, or never remembered): signed out.
                let shared = self.clone();
                let cleared = server.clone();
                let _ = blocking(move || {
                    shared.commit(|servers| {
                        if let Some(saved) = servers.iter_mut().find(|s| s.server == cleared) {
                            saved.active_user = None;
                        }
                    })
                })
                .await;
                self.signed_out(&server);
                return;
            }
            // The keyring is locked or missing: signed out for this run, the saved user stays for a later one.
            Ok(Err(_)) | Err(_) => {
                self.signed_out(&server);
                return;
            }
        };

        // The address must answer as the same server before the token goes there (ADR 0014).
        let Some(address) = saved.addresses.first() else {
            self.update_status(&server, |status| {
                status.reachability = Reachability::Unreachable
            });
            return;
        };
        let Ok(url) = Url::parse(&address.url) else {
            self.update_status(&server, |status| {
                status.reachability = Reachability::Unreachable
            });
            return;
        };
        let prober = Prober::new(server.clone(), self.request_log.clone());
        match prober.probe_expecting(&url, &saved.id).await {
            Ok(found) => self.update_status(&server, |status| {
                status.reachability = Reachability::Reachable;
                status.version = Some(found.info.version);
            }),
            // Not reachable, or another server answers there (`DifferentServer`): no token is sent.
            Err(_) => {
                self.update_status(&server, |status| {
                    status.reachability = Reachability::Unreachable
                });
                return;
            }
        }

        let Ok(client) = self.client(server.clone(), address) else {
            return;
        };
        client.set_token(token);
        let client = Arc::new(client);
        self.install(client.clone());
        match client.current_user().await {
            Ok(user) => {
                let name = user.name.clone();
                self.update_status(&server, |status| {
                    status.signed_in = true;
                    status.user = Some(user);
                });
                if saved.active().is_none_or(|saved| saved.name != name) {
                    let shared = self.clone();
                    let _ = blocking(move || {
                        shared.commit(|servers| {
                            if let Some(saved) = servers.iter_mut().find(|s| s.server == server) {
                                upsert_user(saved, user_id, name);
                            }
                        })
                    })
                    .await;
                }
            }
            // The 401 hook already signed the server out.
            Err(AuthError::SessionRevoked) => {}
            // The token works; this user is refused right now (for example outside a parental time limit).
            Err(AuthError::Refused) => {}
            Err(_) => self.update_status(&server, |status| {
                status.reachability = Reachability::Unreachable
            }),
        }
    }

    fn signed_out(&self, server: &ServerRef) {
        self.update_status(server, |status| {
            status.signed_in = false;
            status.user = None;
        });
    }

    /// See [`App::sign_out`](crate::App::sign_out).
    pub(crate) async fn sign_out(self: &Arc<Self>, server: &ServerRef) -> Result<(), AuthError> {
        let client = lock(&self.sessions).remove(server);
        let told = match client {
            Some(client) => client.sign_out().await,
            None => Err(AuthError::NotSignedIn),
        };
        let shared = self.clone();
        let cleared = server.clone();
        let _ = blocking(move || {
            let user = shared.commit(|servers| {
                servers
                    .iter_mut()
                    .find(|saved| saved.server == cleared)
                    .and_then(|saved| saved.active_user.take())
            });
            if let (Ok(Some(user)), Some(storage)) = (user, &shared.storage) {
                let _ = storage.secrets.delete(&SecretKey::new(cleared, user));
            }
        })
        .await;
        self.signed_out(server);
        told
    }
}

/// Runs blocking work on tokio's blocking pool. Fails only if the work panicked.
async fn blocking<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, StoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| StoreError::Io {
            op: "save",
            kind: std::io::ErrorKind::Other,
        })
}

/// Adds or updates the target's record for a signed-in user and makes the user active. Returns the server, the address to use and
/// the version setup saw.
fn record(
    servers: &mut Vec<SavedServer>,
    target: &Target,
    user: &User,
) -> (ServerRef, ServerAddress, Option<String>) {
    let (index, version) = match target {
        Target::Saved(server) => match servers.iter().position(|saved| &saved.server == server) {
            Some(index) => (index, None),
            // Removed while signing in: nothing to attach the user to, so it is saved again under its key with no address.
            None => {
                servers.push(SavedServer {
                    server: server.clone(),
                    id: ServerId::new(""),
                    name: String::new(),
                    addresses: Vec::new(),
                    users: Vec::new(),
                    active_user: None,
                });
                (servers.len() - 1, None)
            }
        },
        Target::New {
            id,
            name,
            address,
            version,
        } => {
            let index = match servers
                .iter()
                .position(|saved| saved.server.service == ServiceType::JELLYFIN && &saved.id == id)
            {
                Some(index) => {
                    let saved = &mut servers[index];
                    if !saved.addresses.iter().any(|a| a.url == address.url) {
                        saved.addresses.push(address.clone());
                    }
                    saved.name.clone_from(name);
                    index
                }
                None => {
                    servers.push(SavedServer {
                        server: ServerRef::new(ServiceType::JELLYFIN, next_key(servers)),
                        id: id.clone(),
                        name: name.clone(),
                        addresses: vec![address.clone()],
                        users: Vec::new(),
                        active_user: None,
                    });
                    servers.len() - 1
                }
            };
            (index, Some(version.clone()))
        }
    };
    let saved = &mut servers[index];
    upsert_user(saved, user.id.clone(), user.name.clone());
    saved.active_user = Some(user.id.clone());
    let address = match (target, saved.addresses.first()) {
        (Target::New { address, .. }, _) => address.clone(),
        (_, Some(first)) => first.clone(),
        (_, None) => ServerAddress {
            url: String::new(),
            plain_http_confirmed: false,
        },
    };
    (saved.server.clone(), address, version)
}

fn upsert_user(saved: &mut SavedServer, id: UserId, name: String) {
    match saved.users.iter_mut().find(|user| user.id == id) {
        Some(user) => user.name = name,
        None => saved.users.push(SavedUser { id, name }),
    }
}

/// The next server key: one more than the highest number in use. Keys of servers added by a newer version that are not numbers
/// are left alone.
fn next_key(servers: &[SavedServer]) -> ServerKey {
    let highest = servers
        .iter()
        .filter_map(|saved| saved.server.key.as_str().parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    ServerKey::new((highest + 1).to_string())
}

/// A saved server's status before its session is checked: signed in if a user is active, reachability unknown.
fn saved_status(saved: &SavedServer) -> ServerStatus {
    let user = saved.active_user.as_ref().map(|id| User {
        id: id.clone(),
        server: saved.server.clone(),
        name: saved
            .active()
            .map(|user| user.name.clone())
            .unwrap_or_default(),
        permissions: Vec::new(),
        avatar: None,
    });
    ServerStatus {
        signed_in: user.is_some(),
        user,
        ..ServerStatus::new(saved.server.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(key: &str, id: &str) -> SavedServer {
        SavedServer {
            server: ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key)),
            id: ServerId::new(id),
            name: "Test Server".into(),
            addresses: vec![ServerAddress {
                url: "http://192.168.1.10:8096".into(),
                plain_http_confirmed: false,
            }],
            users: Vec::new(),
            active_user: None,
        }
    }

    fn user(id: &str, name: &str) -> User {
        User {
            id: UserId::new(id),
            server: ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("setup")),
            name: name.into(),
            permissions: Vec::new(),
            avatar: None,
        }
    }

    fn new_target(id: &str, url: &str) -> Target {
        Target::New {
            id: ServerId::new(id),
            name: "Renamed Server".into(),
            address: ServerAddress {
                url: url.into(),
                plain_http_confirmed: true,
            },
            version: "12.2.0".into(),
        }
    }

    #[test]
    fn keys_are_one_more_than_the_highest_number() {
        assert_eq!(next_key(&[]), ServerKey::new("1"));
        let servers = [saved("1", "a"), saved("7", "b"), saved("x", "c")];
        assert_eq!(next_key(&servers), ServerKey::new("8"));
    }

    #[test]
    fn a_new_server_gets_a_key_and_its_user_becomes_active() {
        let mut servers = vec![saved("1", "a")];
        let (server, address, version) = record(
            &mut servers,
            &new_target("b", "https://media.example.com"),
            &user("u1", "Example User"),
        );
        assert_eq!(server.key, ServerKey::new("2"));
        assert_eq!(address.url, "https://media.example.com");
        assert_eq!(version.as_deref(), Some("12.2.0"));
        assert_eq!(servers[1].active_user, Some(UserId::new("u1")));
        assert_eq!(servers[1].users[0].name, "Example User");
    }

    #[test]
    fn the_same_server_found_again_keeps_its_key_and_gains_the_address() {
        let mut servers = vec![saved("1", "a")];
        servers[0].users.push(SavedUser {
            id: UserId::new("u1"),
            name: "Old Name".into(),
        });
        let (server, _, _) = record(
            &mut servers,
            &new_target("a", "https://media.example.com"),
            &user("u2", "Another User"),
        );
        assert_eq!(server.key, ServerKey::new("1"));
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].addresses.len(), 2);
        assert!(servers[0].addresses[1].plain_http_confirmed);
        assert_eq!(servers[0].users.len(), 2);
        assert_eq!(servers[0].active_user, Some(UserId::new("u2")));
        assert_eq!(servers[0].name, "Renamed Server");

        // The same address again is not added twice; the same user is updated, not added.
        record(
            &mut servers,
            &new_target("a", "https://media.example.com"),
            &user("u1", "New Name"),
        );
        assert_eq!(servers[0].addresses.len(), 2);
        assert_eq!(servers[0].users.len(), 2);
        assert_eq!(servers[0].users[0].name, "New Name");
    }

    #[test]
    fn a_saved_server_signs_in_at_its_first_address() {
        let mut servers = vec![saved("3", "a")];
        let target = Target::Saved(servers[0].server.clone());
        let (server, address, version) = record(&mut servers, &target, &user("u1", "Example User"));
        assert_eq!(server.key, ServerKey::new("3"));
        assert_eq!(address.url, "http://192.168.1.10:8096");
        assert_eq!(version, None);
    }

    #[test]
    fn a_saved_status_is_signed_in_when_a_user_is_active() {
        let mut server = saved("1", "a");
        assert!(!saved_status(&server).signed_in);
        server.users.push(SavedUser {
            id: UserId::new("u1"),
            name: "Example User".into(),
        });
        server.active_user = Some(UserId::new("u1"));
        let status = saved_status(&server);
        assert!(status.signed_in);
        assert_eq!(status.reachability, Reachability::Unknown);
        assert_eq!(status.user.unwrap().name, "Example User");
    }
}
