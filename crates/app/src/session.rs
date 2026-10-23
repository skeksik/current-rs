//! Saved servers and their sessions: what the app remembers, the signed-in clients, restoring them at start, the 401 rule, sign
//! out and removing a server (ADR 0015).
//!
//! Blocking work (the settings file, the OS keyring) runs on tokio's blocking pool (`spawn_blocking`), so the async methods here
//! need a tokio runtime, as the HTTP client does anyway.
//!
//! Every change to a server's session (saving a sign-in, signing out, removing the server, the 401 rule, restoring) runs under
//! one async lock and checks under it that the session it acts on is still the current one, so a late answer to an old session
//! never touches a newer one.
//!
//! Each saved Jellyfin server has one address provider ([`RoamingAddresses`], ADR 0014) over all its addresses, shared by its
//! session and its sign-ins. The provider serves an address only after it passed the identity check (the server's id) in the race
//! that selected it, so no token and no password goes to an address before that check: no request here probes on its own. The
//! capabilities state follows each provider's status (reachability, the position of the address in use) and the version its
//! identity check read.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, JellyfinClient, JellyfinIdentity, SignedIn,
};
use current_model::{
    Capabilities, NetworkFacts, Reachability, SavedServer, SavedUser, ServerAddress, ServerId,
    ServerKey, ServerRef, ServerStatus, ServiceType, User, UserId,
};
use current_net::{
    FixedAddress, NetError, RequestLog, RoamingAddresses, RoamingConfig, RoamingError,
    RoamingStatus, RuntimeHandle, Url,
};
use current_store::{Secret, SecretKey, Secrets, SecretsError, SettingsStore, StoreError};
use tokio::sync::watch;

use crate::Removal;
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
    /// Whether the server confirmed the user (`GET /Users/Me`) in this session: [`UNCHECKED`], [`CHECKING`] or [`CHECKED`]. A
    /// restored session whose server could not be reached is checked again when its provider next reaches the server.
    check: Arc<AtomicU8>,
}

const UNCHECKED: u8 = 0;
const CHECKING: u8 = 1;
const CHECKED: u8 = 2;

/// A saved Jellyfin server's address provider, with what it was built from.
struct Link {
    /// The saved addresses it was built over: a change rebuilds it.
    addresses: Vec<ServerAddress>,
    id: ServerId,
    /// Tells this link apart from the server's earlier and later ones.
    generation: u64,
    provider: Arc<RoamingAddresses>,
    identity: JellyfinIdentity,
    /// The task that copies the provider's status into the capabilities state.
    follow: tokio::task::AbortHandle,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.follow.abort();
    }
}

/// The server's provider if it was built from `saved`'s addresses and id.
fn current_link(
    links: &HashMap<ServerRef, Link>,
    saved: &SavedServer,
) -> Option<Arc<RoamingAddresses>> {
    links
        .get(&saved.server)
        .filter(|link| link.addresses == saved.addresses && link.id == saved.id)
        .map(|link| link.provider.clone())
}

/// Why a saved server's provider or client could not be made.
#[derive(Debug)]
pub(crate) enum LinkError {
    /// The server is no longer saved.
    Removed,
    /// Called outside the tokio runtime, or the blocking pool lost the work.
    NoRuntime,
    Roaming(RoamingError),
    Net(NetError),
}

impl LinkError {
    /// The error a request would have met. `Removed` has its own sign-in error and does not reach here in practice.
    pub(crate) fn into_net(self) -> NetError {
        match self {
            Self::Net(error) => error,
            Self::Roaming(RoamingError::Tls) => NetError::Tls,
            Self::Removed
            | Self::NoRuntime
            | Self::Roaming(RoamingError::InvalidAddress(_) | RoamingError::TooManyAddresses) => {
                NetError::InvalidRequest
            }
        }
    }
}

/// Puts a check that did not finish (its future was dropped, say a cancelled restore) back to unchecked, so that the next
/// reach of the server checks again.
struct CheckGuard<'a>(&'a AtomicU8);

impl Drop for CheckGuard<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .compare_exchange(CHECKING, UNCHECKED, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Every address provider, so that network facts reach them all (ADR 0014).
#[derive(Debug, Default)]
pub(crate) struct Roaming {
    /// Dropped providers are pruned on the next fan-out.
    pub(crate) providers: Vec<Weak<RoamingAddresses>>,
    /// The facts last reported, for providers made later.
    pub(crate) facts: Option<NetworkFacts>,
}

/// Everything the app's parts share. Held in an `Arc` so that background work (restoring sessions, the 401 hook) can reach it.
pub(crate) struct Shared {
    pub(crate) request_log: Arc<RequestLog>,
    pub(crate) capabilities: watch::Sender<Capabilities>,
    pub(crate) storage: Option<Storage>,
    pub(crate) identity: ClientIdentity,
    /// The saved servers, published to subscribers on every change. The settings file mirrors this list: [`Shared::commit`]
    /// changes both. Borrowed only to read or swap the list, never during file I/O, so async code may read it.
    pub(crate) servers: watch::Sender<Vec<SavedServer>>,
    /// Serializes [`Shared::commit`]: held during the settings write, on the blocking pool only.
    write: Mutex<()>,
    /// The highest numeric server key ever given out, saved with the list: a removed server's key is never given again. Changed
    /// only by [`Shared::commit`], under `write`.
    last_key: AtomicU64,
    /// One live session per server.
    sessions: Mutex<HashMap<ServerRef, Session>>,
    /// One address provider per saved Jellyfin server, made when first needed (restore, a sign-in) since it needs the runtime.
    links: Mutex<HashMap<ServerRef, Link>>,
    next_link: AtomicU64,
    pub(crate) roaming: Mutex<Roaming>,
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
        last_key: u64,
    ) -> Self {
        let capabilities = Capabilities {
            servers: servers.iter().map(saved_status).collect(),
        };
        let last_key = last_key.max(highest_key(&servers));
        Self {
            request_log,
            capabilities: watch::Sender::new(capabilities),
            storage,
            identity,
            servers: watch::Sender::new(servers),
            write: Mutex::new(()),
            last_key: AtomicU64::new(last_key),
            sessions: Mutex::new(HashMap::new()),
            links: Mutex::new(HashMap::new()),
            next_link: AtomicU64::new(1),
            roaming: Mutex::new(Roaming::default()),
            next_session: AtomicU64::new(1),
            changes: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn servers(&self) -> Vec<SavedServer> {
        self.servers.borrow().clone()
    }

    pub(crate) fn saved(&self, server: &ServerRef) -> Option<SavedServer> {
        self.servers
            .borrow()
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

    /// Changes the saved servers and writes them to the settings file, with the highest key given out so far; on a failed write
    /// neither changes. `change` gets that highest key, for a new server's. Blocking: run it on the blocking pool.
    fn commit<R>(
        &self,
        change: impl FnOnce(&mut Vec<SavedServer>, u64) -> R,
    ) -> Result<R, StoreError> {
        let _write = lock(&self.write);
        let mut next = self.servers();
        let last_key = self.last_key.load(Ordering::Relaxed);
        let result = change(&mut next, last_key);
        let last_key = last_key.max(highest_key(&next));
        if let Some(storage) = &self.storage {
            storage.settings.update(|settings| {
                settings.servers = next.clone();
                settings.last_server_key = last_key;
            })?;
        }
        self.last_key.store(last_key, Ordering::Relaxed);
        self.servers.send_if_modified(|servers| {
            let changed = *servers != next;
            *servers = next;
            changed
        });
        Ok(result)
    }

    /// Clears the server's active user if it is still `user`, and deletes that user's token. Blocking. Called under `changes`.
    fn forget_user(&self, server: &ServerRef, user: &UserId) {
        let cleared = self.commit(|servers, _| {
            match servers.iter_mut().find(|saved| &saved.server == server) {
                Some(saved) if saved.active_user.as_ref() == Some(user) => {
                    saved.active_user = None;
                    true
                }
                _ => false,
            }
        });
        if let (Ok(true), Some(storage)) = (cleared, &self.storage) {
            // Nothing more can be done if the keyring fails: the token no longer works anyway.
            let _ = storage
                .secrets
                .delete(&SecretKey::new(server.clone(), user.clone()));
        }
    }

    /// Changes one server's status in the capabilities state, adding it if it is missing. Only a saved server has a status: work
    /// that ends after its server was removed (a restore, say) writes nothing. Checked under the state's lock, which removing a
    /// server takes after its commit, so a status cannot come back after the removal.
    pub(crate) fn update_status(&self, server: &ServerRef, change: impl FnOnce(&mut ServerStatus)) {
        self.capabilities.send_if_modified(|capabilities| {
            if self.saved(server).is_none() {
                return false;
            }
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

    /// A client for a server setup has just found, at the one address setup probed. Saved servers use [`Shared::saved_client`].
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

    /// A client for a saved Jellyfin server over its address provider, and the provider. Async: the provider may have to be made.
    pub(crate) async fn saved_client(
        self: &Arc<Self>,
        server: &ServerRef,
    ) -> Result<(JellyfinClient, Arc<RoamingAddresses>), LinkError> {
        let provider = self.provider(server).await?;
        let client = JellyfinClient::new(
            server.clone(),
            provider.clone(),
            self.identity.clone(),
            self.request_log.clone(),
        )
        .map_err(LinkError::Net)?;
        Ok((client, provider))
    }

    /// The saved server's address provider, made on first use and made again when its saved addresses or id changed. Always built
    /// from the saved list as it is now, never from a caller's older copy, so a slow caller cannot replace a provider with one over
    /// fewer addresses. A new one hears network facts from now on and its status is followed into the capabilities state.
    pub(crate) async fn provider(
        self: &Arc<Self>,
        server: &ServerRef,
    ) -> Result<Arc<RoamingAddresses>, LinkError> {
        let runtime = RuntimeHandle::try_current().map_err(|_| LinkError::NoRuntime)?;
        loop {
            let saved = self.saved(server).ok_or(LinkError::Removed)?;
            if let Some(provider) = current_link(&lock(&self.links), &saved) {
                return Ok(provider);
            }
            let identity = JellyfinIdentity::new(saved.id.clone());
            let built = self.build_provider(&saved, &identity, &runtime).await?;
            {
                let mut links = lock(&self.links);
                // The saved list changed while building (a sign-in added an address): build again from the new one.
                if !self
                    .saved(server)
                    .is_some_and(|now| now.addresses == saved.addresses && now.id == saved.id)
                {
                    continue;
                }
                // Another caller made one for the same list meanwhile: use theirs.
                if let Some(provider) = current_link(&links, &saved) {
                    return Ok(provider);
                }
                let generation = self.next_link.fetch_add(1, Ordering::Relaxed);
                let follow = runtime
                    .spawn(follow(
                        Arc::downgrade(self),
                        server.clone(),
                        generation,
                        built.status(),
                        identity.clone(),
                    ))
                    .abort_handle();
                // Replacing a link drops the old one, which stops its follow task.
                links.insert(
                    server.clone(),
                    Link {
                        addresses: saved.addresses.clone(),
                        id: saved.id.clone(),
                        generation,
                        provider: built.clone(),
                        identity,
                        follow,
                    },
                );
            }
            self.register(&built);
            return Ok(built);
        }
    }

    /// A new address provider over `saved`'s addresses, checking `identity`. Built on the blocking pool: building sets up TLS for
    /// each address, which takes noticeable time.
    async fn build_provider(
        &self,
        saved: &SavedServer,
        identity: &JellyfinIdentity,
        runtime: &RuntimeHandle,
    ) -> Result<Arc<RoamingAddresses>, LinkError> {
        let (saved, identity, log, runtime) = (
            saved.clone(),
            identity.clone(),
            self.request_log.clone(),
            runtime.clone(),
        );
        tokio::task::spawn_blocking(move || {
            RoamingAddresses::new(
                saved.server.clone(),
                &saved.addresses,
                Arc::new(identity),
                log,
                RoamingConfig::default(),
                &runtime,
            )
        })
        .await
        .map_err(|_| LinkError::NoRuntime)?
        .map_err(LinkError::Roaming)
    }

    /// Makes `provider` hear network facts from now on, starting from the last ones reported.
    pub(crate) fn register(&self, provider: &Arc<RoamingAddresses>) {
        let facts = {
            let mut roaming = lock(&self.roaming);
            roaming
                .providers
                .retain(|provider| provider.strong_count() > 0);
            roaming.providers.push(Arc::downgrade(provider));
            roaming.facts.clone()
        };
        if let Some(facts) = facts {
            provider.network_changed(&facts);
        }
    }

    /// Copies the server's provider status into the capabilities state now (the follow task does it on every change).
    fn refresh_status(self: &Arc<Self>, server: &ServerRef) {
        let found = lock(&self.links).get(server).map(|link| {
            (
                link.generation,
                link.provider.status().borrow().clone(),
                link.identity.version(),
            )
        });
        if let Some((generation, status, version)) = found {
            self.apply_roaming(server, generation, &status, version);
        }
    }

    /// Writes a provider's status into the server's status, if `generation` is still the server's link: a follow task of a
    /// replaced provider writes nothing. A provider that has not tried yet (reachability unknown) leaves the status as it is: a
    /// sign-in has just reached the server.
    fn apply_roaming(
        self: &Arc<Self>,
        server: &ServerRef,
        generation: u64,
        roaming: &RoamingStatus,
        version: Option<String>,
    ) {
        if roaming.reachability == Reachability::Unknown {
            return;
        }
        {
            // Held while writing, so a rebuild cannot slip in between the check and the write.
            let links = lock(&self.links);
            if links.get(server).map(|link| link.generation) != Some(generation) {
                return;
            }
            self.update_status(server, |status| {
                status.reachability = roaming.reachability;
                status.address = roaming.position;
                if version.is_some() {
                    status.version = version;
                }
            });
        }
        if roaming.reachability == Reachability::Reachable {
            self.recheck(server);
        }
    }

    /// Checks the session's user again if no check has passed yet in this session and none runs.
    fn recheck(self: &Arc<Self>, server: &ServerRef) {
        let found = lock(&self.sessions).get(server).and_then(|session| {
            session
                .check
                .compare_exchange(UNCHECKED, CHECKING, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                .then(|| {
                    (
                        session.id,
                        session.user.clone(),
                        session.client.clone(),
                        session.check.clone(),
                    )
                })
        });
        let Some((id, user, client, check)) = found else {
            return;
        };
        let Ok(runtime) = RuntimeHandle::try_current() else {
            check.store(UNCHECKED, Ordering::Release);
            return;
        };
        let shared = self.clone();
        let server = server.clone();
        runtime.spawn(async move { shared.check_user(&server, id, user, &client, &check).await });
    }

    /// Makes `client` the server's session for `user`, with the 401 hook installed, replacing any earlier one. `check` says
    /// whether the server has confirmed the user yet. Returns its id and its check state.
    fn install(
        self: &Arc<Self>,
        client: Arc<JellyfinClient>,
        user: UserId,
        check: u8,
    ) -> (u64, Arc<AtomicU8>) {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let server = client.server().clone();
        let shared: Weak<Self> = Arc::downgrade(self);
        let hooked = server.clone();
        client.on_revoked(Arc::new(move || {
            if let Some(shared) = shared.upgrade() {
                shared.revoked(&hooked, id);
            }
        }));
        let check = Arc::new(AtomicU8::new(check));
        lock(&self.sessions).insert(
            server,
            Session {
                id,
                user,
                client,
                check: check.clone(),
            },
        );
        (id, check)
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
        let (saved, version, remembered) = blocking("save", move || {
            let Some((saved, version)) =
                shared.commit(|servers, last_key| record(servers, &target, &user, last_key))?
            else {
                return Err(SignInError::ServerRemoved);
            };
            let remembered = match &shared.storage {
                Some(storage) => storage
                    .secrets
                    .set(
                        &SecretKey::new(saved.server.clone(), user.id.clone()),
                        &Secret::new(token.expose()),
                    )
                    .is_ok(),
                None => false,
            };
            Ok((saved, version, remembered))
        })
        .await??;

        let server = saved.server.clone();
        let mut user = signed_in.user;
        user.server = server.clone();
        // The session goes over the server's provider, made again if this sign-in added an address. Its first request waits for
        // the provider's race, which checks the server's id. Building it fails only for an address that is not a URL, which setup
        // never saves, or a server removed meanwhile.
        if let Ok((client, provider)) = self.saved_client(&server).await {
            client.set_token(signed_in.token);
            self.install(Arc::new(client), user.id.clone(), CHECKED);
            // Race now, so that the status soon shows the address in use.
            provider.retry_now();
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
    ///
    /// Each server's check is spawned on its own and runs to its end even if the caller stops waiting: it takes the session lock,
    /// writes the settings and deletes tokens, and aborting it halfway could leave the lock released while a write still runs.
    /// Dropping this future only stops the waiting.
    pub(crate) async fn restore_all(self: &Arc<Self>) {
        let mut tasks = Vec::new();
        for saved in self.servers() {
            if saved.server.service != ServiceType::JELLYFIN {
                continue;
            }
            let shared = self.clone();
            // Detached: dropping a JoinHandle does not abort its task.
            tasks.push(tokio::spawn(async move {
                // Every saved server gets its provider, signed in or not, so that it hears network facts. An invalid address
                // leaves the server without one; its restore below says unreachable.
                let _ = shared.provider(&saved.server).await;
                if shared.session_id(&saved.server).is_some() {
                    // A restored session whose user was never confirmed: check again.
                    shared.recheck(&saved.server);
                } else if saved.active_user.is_some() {
                    shared.restore(saved).await;
                }
            }));
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    /// Restores one session: the token from the keyring, then the session over the server's provider, then the current user.
    /// No identity probe of its own: the provider serves no address before it passed the identity check in the race that
    /// selected it, so `GET /Users/Me`, the first request with the token, waits for that race (ADR 0014).
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

        let Ok((client, _)) = self.saved_client(&server).await else {
            // No usable address list (or the server was removed): nothing can be reached.
            if self.session_id(&server).is_none() {
                self.update_status(&server, |status| {
                    status.reachability = Reachability::Unreachable;
                });
            }
            return;
        };
        client.set_token(token);
        let client = Arc::new(client);
        let (id, check) = {
            let _changes = self.changes.lock().await;
            // A sign-in or sign-out since `restore_all` read the list decides instead.
            if self.session_id(&server).is_some()
                || self.active_user(&server) != Some(user_id.clone())
            {
                return;
            }
            self.install(client.clone(), user_id.clone(), CHECKING)
        };
        self.check_user(&server, id, user_id, &client, &check).await;
        self.refresh_status(&server);
    }

    /// Asks the server for the session's user (`GET /Users/Me`) and shows it. A failure to reach the server leaves the check to
    /// be done again when the provider next reaches it; the 401 hook handles a revoked token.
    async fn check_user(
        self: &Arc<Self>,
        server: &ServerRef,
        id: u64,
        user_id: UserId,
        client: &JellyfinClient,
        check: &AtomicU8,
    ) {
        let _unfinished = CheckGuard(check);
        match client.current_user().await {
            Ok(user) => {
                check.store(CHECKED, Ordering::Release);
                let name = user.name.clone();
                self.update_session_status(server, id, |status| {
                    status.signed_in = true;
                    status.user = Some(user);
                });
                let renamed = self.saved(server).is_some_and(|saved| {
                    saved.active_user.as_ref() == Some(&user_id)
                        && saved.active().is_none_or(|saved| saved.name != name)
                });
                if renamed {
                    let shared = self.clone();
                    let server = server.clone();
                    let _ = blocking("save", move || {
                        shared.commit(|servers, _| {
                            if let Some(saved) = servers.iter_mut().find(|s| s.server == server) {
                                upsert_user(saved, user_id, name);
                            }
                        })
                    })
                    .await;
                }
            }
            // The 401 hook already signed the server out; the token works but this user is refused right now (for example
            // outside a parental time limit). Either way the check is done.
            Err(AuthError::SessionRevoked | AuthError::Refused) => {
                check.store(CHECKED, Ordering::Release);
            }
            // Not reached (the reachability comes from the provider), or an answer that could not be read: try again later.
            Err(_) => check.store(UNCHECKED, Ordering::Release),
        }
    }

    fn signed_out(&self, server: &ServerRef) {
        self.update_status(server, |status| {
            status.signed_in = false;
            status.user = None;
        });
    }

    /// See [`App::sign_out`](crate::App::sign_out). Signs out on this device first, under the session lock: the session, the active
    /// user and the stored token are gone before the server is told, so nothing (a restore, say) can bring them back while the
    /// server answers. The server is then told with the token the old client still holds in memory. A sign-in that completes
    /// meanwhile is a new session and is kept.
    pub(crate) async fn sign_out(self: &Arc<Self>, server: &ServerRef) -> Result<(), AuthError> {
        let session = {
            let _changes = self.changes.lock().await;
            let session = lock(&self.sessions).remove(server);
            let user = match &session {
                Some(session) => Some(session.user.clone()),
                None => self.active_user(server),
            };
            if let Some(user) = user {
                let shared = self.clone();
                let cleared = server.clone();
                let _ = blocking("save", move || shared.forget_user(&cleared, &user)).await;
            }
            self.signed_out(server);
            session
        };
        match session {
            Some(session) => session.client.sign_out().await,
            None => Err(AuthError::NotSignedIn),
        }
    }

    /// See [`App::remove_server`](crate::App::remove_server). Runs to the end even if the caller stops waiting.
    pub(crate) async fn remove_server(
        self: &Arc<Self>,
        server: &ServerRef,
    ) -> Result<Removal, StoreError> {
        let shared = self.clone();
        let server = server.clone();
        tokio::spawn(async move { shared.apply_removal(server).await })
            .await
            .unwrap_or(Err(StoreError::Io {
                op: "save",
                kind: std::io::ErrorKind::Other,
            }))
    }

    /// Removes the server on this device first, then tells it. Under the session lock, kept short (no keyring I/O): the list
    /// without the server is saved, then its session, provider and status are dropped. From then on nothing can write a token
    /// under its key: a sign-in or restore finds no saved server, a sign-in saved earlier has already stored its token (read and
    /// deleted below), and the key is never given out again. After the lock: every saved user's token is read and deleted from
    /// the keyring, and each token found, with the live session's, is logged out on the server in the background, through the
    /// server's identity-checked provider (ADR 0014), never awaited.
    async fn apply_removal(self: &Arc<Self>, server: ServerRef) -> Result<Removal, StoreError> {
        let (saved, session, provider) = {
            let _changes = self.changes.lock().await;
            let Some(saved) = self.saved(&server) else {
                return Ok(Removal::NotSaved);
            };
            self.check_writable()?;
            let shared = self.clone();
            let removed = server.clone();
            blocking("save", move || {
                shared.commit(|servers, _| servers.retain(|saved| saved.server != removed))
            })
            .await??;
            let session = lock(&self.sessions).remove(&server);
            // Dropping the link stops its follow task; the provider itself lives on for the logouts below.
            let provider = lock(&self.links)
                .remove(&server)
                .map(|link| link.provider.clone());
            self.capabilities.send_if_modified(|capabilities| {
                let before = capabilities.servers.len();
                capabilities
                    .servers
                    .retain(|status| status.server != server);
                capabilities.servers.len() != before
            });
            (saved, session, provider)
        };

        let mut users: Vec<UserId> = saved.users.iter().map(|user| user.id.clone()).collect();
        if let Some(active) = &saved.active_user
            && !users.contains(active)
        {
            users.push(active.clone());
        }
        let (tokens, credentials_left) = match &self.storage {
            Some(storage) => {
                let secrets = storage.secrets.clone();
                let server = server.clone();
                blocking("delete from the keyring", move || {
                    forget_tokens(secrets.as_ref(), &server, &users)
                })
                .await
                .unwrap_or((Vec::new(), true))
            }
            None => (Vec::new(), false),
        };

        self.tell_removed(&saved, session, provider, tokens).await;
        Ok(Removal::Removed { credentials_left })
    }

    /// Logs out the removed server's live session and each stored token (but the live session's user's, which that session
    /// covers), in the background: best effort, never awaited, the answers ignored.
    async fn tell_removed(
        self: &Arc<Self>,
        saved: &SavedServer,
        session: Option<Session>,
        provider: Option<Arc<RoamingAddresses>>,
        tokens: Vec<(UserId, Secret)>,
    ) {
        let live = session.as_ref().map(|session| session.user.clone());
        let tokens: Vec<_> = tokens
            .into_iter()
            .filter(|(user, _)| Some(user) != live.as_ref())
            .collect();
        let Ok(runtime) = RuntimeHandle::try_current() else {
            return;
        };
        if let Some(session) = session {
            runtime.spawn(async move {
                let _ = session.client.sign_out().await;
            });
        }
        if tokens.is_empty() {
            return;
        }
        // A server never reached in this run has no provider yet: one over its last saved addresses, for these requests only.
        let provider = match provider {
            Some(provider) => provider,
            None => {
                let identity = JellyfinIdentity::new(saved.id.clone());
                match self.build_provider(saved, &identity, &runtime).await {
                    Ok(provider) => {
                        self.register(&provider);
                        provider
                    }
                    Err(_) => return,
                }
            }
        };
        for (_, token) in tokens {
            let Ok(client) = JellyfinClient::new(
                saved.server.clone(),
                provider.clone(),
                self.identity.clone(),
                self.request_log.clone(),
            ) else {
                return;
            };
            client.set_token(AccessToken::new(token.expose()));
            runtime.spawn(async move {
                let _ = client.sign_out().await;
            });
        }
    }
}

/// Reads and deletes each user's token on `server`. Returns the tokens found, and whether any may be left: a delete that failed,
/// or a keyring that is unavailable (locked, refused) on a platform with a backend, where a token stored while it worked may
/// still be there. Without a backend (Linux until its app task) nothing was ever stored, so nothing is left. Blocking.
fn forget_tokens(
    secrets: &dyn Secrets,
    server: &ServerRef,
    users: &[UserId],
) -> (Vec<(UserId, Secret)>, bool) {
    let mut tokens = Vec::new();
    let mut left = false;
    for user in users {
        let key = SecretKey::new(server.clone(), user.clone());
        match secrets.get(&key) {
            Ok(Some(token)) => tokens.push((user.clone(), token)),
            Err(SecretsError::Unavailable) if secrets.has_backend() => left = true,
            Ok(None) | Err(_) => {}
        }
        match secrets.delete(&key) {
            Ok(()) => {}
            Err(SecretsError::Unavailable) => left |= secrets.has_backend(),
            Err(SecretsError::TooLong | SecretsError::Failed) => left = true,
        }
    }
    (tokens, left)
}

/// Copies a provider's status into the capabilities state on every change, until the provider or the app is gone or the link is
/// replaced (which aborts it).
async fn follow(
    shared: Weak<Shared>,
    server: ServerRef,
    generation: u64,
    mut status: watch::Receiver<RoamingStatus>,
    identity: JellyfinIdentity,
) {
    loop {
        let current = status.borrow_and_update().clone();
        let Some(app) = shared.upgrade() else {
            return;
        };
        app.apply_roaming(&server, generation, &current, identity.version());
        drop(app);
        if status.changed().await.is_err() {
            return;
        }
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

/// Adds or updates the target's record for a signed-in user and makes the user active. Returns the saved record as it is now and
/// the version setup saw; `None` when the saved server it was for is gone (removed while signing in).
fn record(
    servers: &mut Vec<SavedServer>,
    target: &Target,
    user: &User,
    last_key: u64,
) -> Option<(SavedServer, Option<String>)> {
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
                        server: ServerRef::new(ServiceType::JELLYFIN, next_key(servers, last_key)),
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
    Some((saved.clone(), version))
}

fn upsert_user(saved: &mut SavedServer, id: UserId, name: String) {
    match saved.users.iter_mut().find(|user| user.id == id) {
        Some(user) => user.name = name,
        None => saved.users.push(SavedUser { id, name }),
    }
}

/// The next server key: one more than the highest number in use or ever given out (`last_key`), so a removed server's key is
/// never given to another server. Keys of servers added by a newer version that are not numbers are left alone.
fn next_key(servers: &[SavedServer], last_key: u64) -> ServerKey {
    let highest = highest_key(servers).max(last_key);
    ServerKey::new(highest.saturating_add(1).to_string())
}

/// The highest numeric key in `servers`, 0 if none.
fn highest_key(servers: &[SavedServer]) -> u64 {
    servers
        .iter()
        .filter_map(|saved| saved.server.key.as_str().parse::<u64>().ok())
        .max()
        .unwrap_or(0)
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
        assert_eq!(next_key(&[], 0), ServerKey::new("1"));
        let servers = [saved("1", "a"), saved("7", "b"), saved("x", "c")];
        assert_eq!(next_key(&servers, 0), ServerKey::new("8"));
        // A removed server's key is never given again.
        assert_eq!(next_key(&servers, 9), ServerKey::new("10"));
        assert_eq!(next_key(&[], 3), ServerKey::new("4"));
    }

    #[test]
    fn a_new_server_gets_a_key_and_its_user_becomes_active() {
        let mut servers = vec![saved("1", "a")];
        let (saved, version) = record(
            &mut servers,
            &new_target("b", "https://media.example.com"),
            &user("u1", "Example User"),
            0,
        )
        .unwrap();
        assert_eq!(saved.server.key, ServerKey::new("2"));
        assert_eq!(saved.addresses[0].url, "https://media.example.com");
        assert_eq!(saved, servers[1]);
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
        let (saved, _) = record(
            &mut servers,
            &new_target("a", "https://media.example.com"),
            &user("u2", "Another User"),
            0,
        )
        .unwrap();
        assert_eq!(saved.server.key, ServerKey::new("1"));
        // The session is built over both addresses.
        assert_eq!(saved.addresses.len(), 2);
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
            0,
        );
        assert_eq!(servers[0].addresses.len(), 2);
        assert_eq!(servers[0].users.len(), 2);
        assert_eq!(servers[0].users[0].name, "New Name");
    }

    #[test]
    fn a_saved_server_keeps_its_record() {
        let mut servers = vec![saved("3", "a")];
        let target = Target::Saved(servers[0].server.clone());
        let (saved, version) =
            record(&mut servers, &target, &user("u1", "Example User"), 0).unwrap();
        assert_eq!(saved.server.key, ServerKey::new("3"));
        assert_eq!(saved.addresses, servers[0].addresses);
        assert_eq!(version, None);
    }

    #[test]
    fn a_saved_server_that_vanished_is_not_saved_again() {
        let mut servers = vec![saved("3", "a")];
        let target = Target::Saved(ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("9")));
        assert_eq!(
            record(&mut servers, &target, &user("u1", "Example User"), 0),
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

    fn shared_with(servers: Vec<SavedServer>) -> Arc<Shared> {
        Arc::new(Shared::new(
            Arc::new(RequestLog::default()),
            None,
            ClientIdentity::current("Test", "device"),
            servers,
            0,
        ))
    }

    fn reachable_at(position: u8) -> RoamingStatus {
        RoamingStatus {
            reachability: Reachability::Reachable,
            position: Some(position),
            different_server: Vec::new(),
        }
    }

    #[tokio::test]
    async fn the_provider_is_built_from_the_saved_list_as_it_is_now() {
        let shared = shared_with(vec![saved("1", "a")]);
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"));
        let first = shared.provider(&server).await.unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &shared.provider(&server).await.unwrap()
        ));
        let old = lock(&shared.links)[&server].generation;

        // A sign-in adds an address: the next caller gets a provider over both, whatever list it read earlier.
        shared
            .commit(|servers, _| {
                servers[0].addresses.push(ServerAddress {
                    url: "https://media.example.com".into(),
                    plain_http_confirmed: false,
                });
            })
            .unwrap();
        let second = shared.provider(&server).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        let links = lock(&shared.links);
        assert_eq!(links[&server].addresses.len(), 2);
        let new = links[&server].generation;
        drop(links);

        // The replaced provider's status no longer reaches the capabilities state; the current one's does.
        shared.apply_roaming(&server, old, &reachable_at(1), None);
        let status = shared.capabilities.borrow().servers[0].clone();
        assert_eq!(status.reachability, Reachability::Unknown);
        shared.apply_roaming(&server, new, &reachable_at(2), Some("12.2.0".into()));
        let status = shared.capabilities.borrow().servers[0].clone();
        assert_eq!(status.reachability, Reachability::Reachable);
        assert_eq!(status.address, Some(2));
        assert_eq!(status.version.as_deref(), Some("12.2.0"));
    }

    #[tokio::test]
    async fn a_removed_server_gets_no_provider() {
        let shared = shared_with(Vec::new());
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"));
        assert!(matches!(
            shared.provider(&server).await,
            Err(LinkError::Removed)
        ));
    }

    #[tokio::test]
    async fn a_removed_server_gets_no_status_back_and_its_key_is_not_reused() {
        let shared = shared_with(vec![saved("1", "a"), saved("2", "b")]);
        let second = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("2"));
        assert_eq!(
            shared.remove_server(&second).await,
            Ok(Removal::Removed {
                credentials_left: false
            })
        );
        assert_eq!(shared.servers().len(), 1);
        assert_eq!(shared.capabilities.borrow().servers.len(), 1);
        // Late work for the removed server (a restore's status, say) writes nothing.
        shared.update_status(&second, |status| {
            status.reachability = Reachability::Unreachable;
        });
        assert!(shared.capabilities.borrow().server(&second).is_none());
        // Removing it again does nothing.
        assert_eq!(shared.remove_server(&second).await, Ok(Removal::NotSaved));
        // The next new server gets key 3, not 2.
        let (saved, _) = shared
            .commit(|servers, last_key| {
                record(
                    servers,
                    &new_target("c", "https://media.example.com"),
                    &user("u1", "Example User"),
                    last_key,
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(saved.server.key, ServerKey::new("3"));
    }

    #[test]
    fn a_provider_needs_the_runtime() {
        let shared = shared_with(vec![saved("1", "a")]);
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"));
        // Polled outside tokio: an error, not a panic.
        let result = poll_once(shared.provider(&server));
        assert!(matches!(result, Err(LinkError::NoRuntime)));
    }

    /// Polls a future that completes without waiting, on no runtime.
    fn poll_once<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => panic!("the future waited"),
        }
    }
}
