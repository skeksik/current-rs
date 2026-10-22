//! Saved servers and their sessions: what the app remembers, the signed-in clients, restoring them at start, the 401 rule and
//! sign out (ADR 0015).
//!
//! Blocking work (the settings file, the OS keyring) runs on tokio's blocking pool (`spawn_blocking`), so the async methods here
//! need a tokio runtime, as the HTTP client does anyway.
//!
//! Every change to a server's session (saving a sign-in, signing out, the 401 rule, restoring) runs under one async lock and
//! checks under it that the session it acts on is still the current one, so a late answer to an old session never touches a newer
//! one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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

use crate::sign_in::SignInError;

/// What an app opened on a data folder keeps between runs.
pub(crate) struct Storage {
    pub(crate) settings: SettingsStore,
    pub(crate) secrets: Arc<dyn Secrets>,
}

/// A live session: the signed-in client of one user on one server. `id` tells it apart from earlier and later sessions of the
/// same server.
struct Session {
    id: u64,
    user: UserId,
    client: Arc<JellyfinClient>,
}

/// Everything the app's parts share. Held in an `Arc` so that background work (restoring sessions, the 401 hook) can reach it.
pub(crate) struct Shared {
    pub(crate) request_log: Arc<RequestLog>,
    pub(crate) capabilities: watch::Sender<Capabilities>,
    pub(crate) storage: Option<Storage>,
    pub(crate) identity: ClientIdentity,
    /// The saved servers. The settings file mirrors this list: [`Shared::commit`] changes both. Held only to read or swap the
    /// list, never during file I/O, so async code may read it.
    servers: Mutex<Vec<SavedServer>>,
    /// Serializes [`Shared::commit`]: held during the settings write, on the blocking pool only.
    write: Mutex<()>,
    /// One live session per server.
    sessions: Mutex<HashMap<ServerRef, Session>>,
    next_session: AtomicU64,
    /// Orders session changes (see the module documentation).
    changes: tokio::sync::Mutex<()>,
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
            write: Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            changes: tokio::sync::Mutex::new(()),
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
        lock(&self.sessions)
            .get(server)
            .map(|session| session.client.clone())
    }

    fn session_id(&self, server: &ServerRef) -> Option<u64> {
        lock(&self.sessions).get(server).map(|session| session.id)
    }

    fn active_user(&self, server: &ServerRef) -> Option<UserId> {
        self.saved(server).and_then(|saved| saved.active_user)
    }

    /// Fails with [`StoreError::NewerVersion`] when the settings belong to a newer version and cannot be saved: a sign-in would
    /// get a token (logging out this device's earlier one on the server) and then fail to save it.
    pub(crate) fn check_writable(&self) -> Result<(), StoreError> {
        match self
            .storage
            .as_ref()
            .and_then(|storage| storage.settings.newer_version())
        {
            Some(found) => Err(StoreError::NewerVersion { found }),
            None => Ok(()),
        }
    }

    /// Changes the saved servers and writes them to the settings file; on a failed write neither changes. Blocking: run it on the
    /// blocking pool.
    fn commit<R>(&self, change: impl FnOnce(&mut Vec<SavedServer>) -> R) -> Result<R, StoreError> {
        let _write = lock(&self.write);
        let mut next = self.servers();
        let result = change(&mut next);
        if let Some(storage) = &self.storage {
            storage
                .settings
                .update(|settings| settings.servers = next.clone())?;
        }
        *lock(&self.servers) = next;
        Ok(result)
    }

    /// Clears the server's active user if it is still `user`, and deletes that user's token. Blocking. Called under `changes`.
    fn forget_user(&self, server: &ServerRef, user: &UserId) {
        let cleared =
            self.commit(
                |servers| match servers.iter_mut().find(|saved| &saved.server == server) {
                    Some(saved) if saved.active_user.as_ref() == Some(user) => {
                        saved.active_user = None;
                        true
                    }
                    _ => false,
                },
            );
        if let (Ok(true), Some(storage)) = (cleared, &self.storage) {
            // Nothing more can be done if the keyring fails: the token no longer works anyway.
            let _ = storage
                .secrets
                .delete(&SecretKey::new(server.clone(), user.clone()));
        }
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

    /// Changes the status only while session `id` is the server's current one.
    fn update_session_status(
        &self,
        server: &ServerRef,
        id: u64,
        change: impl FnOnce(&mut ServerStatus),
    ) {
        if self.session_id(server) == Some(id) {
            self.update_status(server, change);
        }
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

    /// Makes `client` the server's session for `user`, with the 401 hook installed, replacing any earlier one. Returns its id.
    fn install(self: &Arc<Self>, client: Arc<JellyfinClient>, user: UserId) -> u64 {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let server = client.server().clone();
        let shared: Weak<Self> = Arc::downgrade(self);
        let hooked = server.clone();
        client.on_revoked(Arc::new(move || {
            if let Some(shared) = shared.upgrade() {
                shared.revoked(&hooked, id);
            }
        }));
        lock(&self.sessions).insert(server, Session { id, user, client });
        id
    }

    /// The 401 rule (ADR 0015): session `id` is over. Nothing happens if a newer session replaced it. Otherwise the status changes
    /// at once, and the saved active user and that user's token are removed in the background.
    fn revoked(self: &Arc<Self>, server: &ServerRef, id: u64) {
        let user = {
            let mut sessions = lock(&self.sessions);
            match sessions.get(server) {
                Some(session) if session.id == id => {
                    sessions.remove(server).map(|session| session.user)
                }
                _ => return,
            }
        };
        let Some(user) = user else {
            return;
        };
        self.signed_out(server);
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // Without a runtime there is no request either; the hook only runs inside one.
            return;
        };
        let shared = self.clone();
        let server = server.clone();
        handle.spawn(async move {
            let _changes = shared.changes.lock().await;
            // A sign-in completed meanwhile: its user and token are not ours to remove.
            if shared.session_id(&server).is_some() {
                return;
            }
            let worker = shared.clone();
            let _ = blocking("save", move || worker.forget_user(&server, &user)).await;
        });
    }

    /// Saves a successful sign-in: the server (a new key, or the saved record with the same server id), the user as the active
    /// one, the token in the keyring, the live session and the status. Returns the server and whether the token was remembered:
    /// without a keyring the session lasts for this run only (ADR 0013). Runs to the end even if the caller stops waiting.
    pub(crate) async fn save_sign_in(
        self: &Arc<Self>,
        target: Target,
        signed_in: SignedIn,
    ) -> Result<(ServerRef, bool), SignInError> {
        let shared = self.clone();
        tokio::spawn(async move { shared.apply_sign_in(target, signed_in).await })
            .await
            .unwrap_or(Err(SignInError::Save(StoreError::Io {
                op: "save",
                kind: std::io::ErrorKind::Other,
            })))
    }

    async fn apply_sign_in(
        self: &Arc<Self>,
        target: Target,
        signed_in: SignedIn,
    ) -> Result<(ServerRef, bool), SignInError> {
        let _changes = self.changes.lock().await;
        let shared = self.clone();
        let user = signed_in.user.clone();
        let token = signed_in.token.clone();
        let (server, address, version, remembered) = blocking("save", move || {
            let Some((server, address, version)) =
                shared.commit(|servers| record(servers, &target, &user))?
            else {
                return Err(SignInError::ServerRemoved);
            };
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
            Ok((server, address, version, remembered))
        })
        .await??;

        let mut user = signed_in.user;
        user.server = server.clone();
        // Building a client fails only for an address that is not a URL, which setup never saves.
        if let Ok(client) = self.client(server.clone(), &address) {
            client.set_token(signed_in.token);
            self.install(Arc::new(client), user.id.clone());
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

    /// Checks every saved Jellyfin session, all servers at once. See [`App::restore_sessions`](crate::App::restore_sessions).
    pub(crate) async fn restore_all(self: &Arc<Self>) {
        let mut tasks = JoinSet::new();
        for saved in self.servers() {
            if saved.server.service == ServiceType::JELLYFIN
                && saved.active_user.is_some()
                && self.session_id(&saved.server).is_none()
            {
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
                blocking("read the keyring for", move || secrets.get(&key)).await
            }
            None => Ok(Ok(None)),
        };
        let token = match token {
            Ok(Ok(Some(secret))) => AccessToken::new(secret.expose()),
            Ok(Ok(None)) => {
                // Nothing stored (removed from the keyring by hand, or never remembered): signed out, unless a sign-in came first.
                let _changes = self.changes.lock().await;
                if self.session_id(&server).is_none() {
                    let shared = self.clone();
                    let cleared = server.clone();
                    let _ = blocking("save", move || shared.forget_user(&cleared, &user_id)).await;
                    self.signed_out(&server);
                }
                return;
            }
            // The keyring is locked or missing: signed out for this run, the saved user stays for a later one.
            Ok(Err(_)) | Err(_) => {
                if self.session_id(&server).is_none() {
                    self.signed_out(&server);
                }
                return;
            }
        };

        // The address must answer as the same server before the token goes there (ADR 0014).
        let unreachable =
            |status: &mut ServerStatus| status.reachability = Reachability::Unreachable;
        let Some(address) = saved.addresses.first() else {
            self.update_status(&server, unreachable);
            return;
        };
        let Ok(url) = Url::parse(&address.url) else {
            self.update_status(&server, unreachable);
            return;
        };
        let prober = Prober::new(server.clone(), self.request_log.clone());
        let found = match prober.probe_expecting(&url, &saved.id).await {
            Ok(found) => found,
            // Not reachable, or another server answers there (`DifferentServer`): no token is sent.
            Err(_) => {
                if self.session_id(&server).is_none() {
                    self.update_status(&server, unreachable);
                }
                return;
            }
        };

        let Ok(client) = self.client(server.clone(), address) else {
            return;
        };
        client.set_token(token);
        let client = Arc::new(client);
        let id = {
            let _changes = self.changes.lock().await;
            // A sign-in or sign-out while the probe ran decides instead.
            if self.session_id(&server).is_some()
                || self.active_user(&server) != Some(user_id.clone())
            {
                return;
            }
            self.install(client.clone(), user_id.clone())
        };
        self.update_session_status(&server, id, |status| {
            status.reachability = Reachability::Reachable;
            status.version = Some(found.info.version);
        });
        match client.current_user().await {
            Ok(user) => {
                let name = user.name.clone();
                self.update_session_status(&server, id, |status| {
                    status.signed_in = true;
                    status.user = Some(user);
                });
                if saved.active().is_none_or(|saved| saved.name != name) {
                    let shared = self.clone();
                    let _ = blocking("save", move || {
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
            Err(_) => self.update_session_status(&server, id, unreachable),
        }
    }

    fn signed_out(&self, server: &ServerRef) {
        self.update_status(server, |status| {
            status.signed_in = false;
            status.user = None;
        });
    }

    /// See [`App::sign_out`](crate::App::sign_out). The session signed out is the one current when it starts; a sign-in that
    /// completes while the server is being told is kept.
    pub(crate) async fn sign_out(self: &Arc<Self>, server: &ServerRef) -> Result<(), AuthError> {
        let session = lock(&self.sessions).remove(server);
        let user = match &session {
            Some(session) => Some(session.user.clone()),
            None => self.active_user(server),
        };
        if session.is_some() {
            self.signed_out(server);
        }
        let told = match &session {
            Some(session) => session.client.sign_out().await,
            None => Err(AuthError::NotSignedIn),
        };
        let _changes = self.changes.lock().await;
        if self.session_id(server).is_some() {
            return told;
        }
        if let Some(user) = user {
            let shared = self.clone();
            let cleared = server.clone();
            let _ = blocking("save", move || shared.forget_user(&cleared, &user)).await;
        }
        self.signed_out(server);
        told
    }
}

/// Runs blocking work on tokio's blocking pool. Fails only if the work panicked; `op` names the step for the error.
async fn blocking<R: Send + 'static>(
    op: &'static str,
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, StoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| StoreError::Io {
            op,
            kind: std::io::ErrorKind::Other,
        })
}

/// Adds or updates the target's record for a signed-in user and makes the user active. Returns the server, the address to use and
/// the version setup saw; `None` when the saved server it was for is gone (removed while signing in).
fn record(
    servers: &mut Vec<SavedServer>,
    target: &Target,
    user: &User,
) -> Option<(ServerRef, ServerAddress, Option<String>)> {
    let (index, version) = match target {
        Target::Saved(server) => (
            servers.iter().position(|saved| &saved.server == server)?,
            None,
        ),
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
    let address = match target {
        Target::New { address, .. } => address.clone(),
        Target::Saved(_) => saved.addresses.first()?.clone(),
    };
    upsert_user(saved, user.id.clone(), user.name.clone());
    saved.active_user = Some(user.id.clone());
    Some((saved.server.clone(), address, version))
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        let (server, address, version) =
            record(&mut servers, &target, &user("u1", "Example User")).unwrap();
        assert_eq!(server.key, ServerKey::new("3"));
        assert_eq!(address.url, "http://192.168.1.10:8096");
        assert_eq!(version, None);
    }

    #[test]
    fn a_saved_server_that_vanished_is_not_saved_again() {
        let mut servers = vec![saved("3", "a")];
        let target = Target::Saved(ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("9")));
        assert_eq!(
            record(&mut servers, &target, &user("u1", "Example User")),
            None
        );
        assert_eq!(servers, [saved("3", "a")]);
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
