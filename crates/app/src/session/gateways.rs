//! The session side of authentication gateways (ADR 0017): each saved address's gateway credentials, loaded from the keyring and
//! shared by every client of that address, the cookies a gateway rotates saved back, the 401 rule's confirming probe (ADR 0015 as
//! amended), and the "Sign in to the gateway again" status.
//!
//! Every saved address gets its own [`Gateway`], empty when it has none: a later sign-in to the gateway then replaces the
//! credentials in place ([`Gateway::replace`]) and every client of the address carries them at once. No address ever gets another
//! address's gateway.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use current_jellyfin::{
    AccessToken, HeaderPlacement, JellyfinClient, ProbeError, Prober, SessionEvent,
};
use current_model::{AddressId, Reachability, SavedServer, ServerAddress, ServerRef};
use current_net::{
    AddressProvider, ClientConfig, Gateway, GatewayCredentials, RuntimeHandle, ServerClient, Url,
};
use current_store::{
    GatewayKey, GatewaySecret, StoreError, delete_gateway_secret, get_gateway_secret,
    set_gateway_secret,
};

use super::{Shared, UNCHECKED, blocking, lock};
use crate::gateway;

/// How often a link checks its addresses' gateways for rotated cookies to save.
pub(super) const ROTATION_CHECK: Duration = Duration::from_secs(60);

/// The confirming probe's timeouts: those of setup's probe (5 s to connect, 10 s per request).
fn probe_config() -> ClientConfig {
    ClientConfig {
        timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(5),
    }
}

/// One saved address's gateway in use.
pub(super) struct GatewayEntry {
    pub(super) gateway: Arc<Gateway>,
    /// The [`Gateway::changes`] count of the credentials last stored in the keyring.
    saved: u64,
    /// Whether the credentials are in the keyring: rotated cookies are saved only then. Credentials the keyring refused last for
    /// this run only, as a token does (ADR 0013).
    remembered: bool,
}

/// A new server's gateway credentials, stored in the keyring (when `remembered`) but not yet in use.
pub(super) struct NewGateway {
    key: GatewayKey,
    credentials: GatewayCredentials,
    count: u64,
    remembered: bool,
}

/// Whether a probe's answer is a gateway's: a gateway shape, or a redirect off the address (a portal, ADR 0017).
pub(crate) fn gateway_answered(error: &ProbeError) -> bool {
    error.is_gateway() || error.offers_sign_in()
}

impl Shared {
    /// The address's gateway, loaded from the keyring the first time (only for an address whose settings list a kind). `None` for
    /// an address without an id, which only setup's addresses lack. Blocking.
    pub(super) fn load_gateway(
        &self,
        server: &ServerRef,
        address: &ServerAddress,
    ) -> Option<Arc<Gateway>> {
        let id = address.id?;
        let key = (server.clone(), id);
        if let Some(entry) = lock(&self.gateways).get(&key) {
            return Some(entry.gateway.clone());
        }
        let stored = match &self.storage {
            Some(storage) if !address.gateway.is_empty() => get_gateway_secret(
                storage.secrets.as_ref(),
                &GatewayKey::new(server.clone(), id),
            )
            .ok()
            .flatten()
            .and_then(|secret| gateway::parse(secret.expose())),
            _ => None,
        };
        let remembered = stored.is_some();
        let gateway = Arc::new(Gateway::new(stored.unwrap_or_default()));
        // Another caller may have loaded it meanwhile: theirs wins, so every client shares one.
        let mut gateways = lock(&self.gateways);
        let entry = gateways.entry(key).or_insert(GatewayEntry {
            saved: gateway.changes(),
            gateway,
            remembered,
        });
        Some(entry.gateway.clone())
    }

    /// The saved server's addresses, each with its own gateway. Reads the keyring on the blocking pool.
    pub(super) async fn address_gateways(
        self: &Arc<Self>,
        saved: &SavedServer,
    ) -> Vec<(ServerAddress, Option<Arc<Gateway>>)> {
        let shared = self.clone();
        let saved = saved.clone();
        let fallback: Vec<_> = saved.addresses.iter().map(|a| (a.clone(), None)).collect();
        blocking("read the keyring for", move || {
            saved
                .addresses
                .iter()
                .map(|address| (address.clone(), shared.load_gateway(&saved.server, address)))
                .collect()
        })
        .await
        .unwrap_or(fallback)
    }

    /// Stores `credentials` as the address's gateway secret. Returns whether the keyring took them. Blocking.
    pub(super) fn store_gateway(&self, key: &GatewayKey, credentials: &GatewayCredentials) -> bool {
        match &self.storage {
            Some(storage) => set_gateway_secret(
                storage.secrets.as_ref(),
                key,
                &GatewaySecret::new(gateway::serialise(credentials)),
            )
            .is_ok(),
            None => false,
        }
    }

    /// Stores the gateway a new server's sign-in went through for its address at `url` in `record`: the credentials in the
    /// keyring first, then (in the settings write that follows) the kinds on the address. Credentials the keyring refuses are used
    /// for this run only and the kinds stay empty. Nothing is cached here: the caller adopts the result with
    /// [`Shared::adopt_new_gateway`] once the settings write succeeded, and deletes the keyring entry if it failed, so a key and
    /// address id that a failed write leaves free never lead to these credentials. Blocking: called inside [`Shared::commit`].
    pub(super) fn store_new_gateway(
        &self,
        record: &mut SavedServer,
        url: &str,
        in_use: &Arc<Gateway>,
    ) -> Option<NewGateway> {
        let server = record.server.clone();
        let address = record.addresses.iter_mut().find(|a| a.url == url)?;
        let id = address.id?;
        let (credentials, count) = in_use.snapshot();
        if credentials.is_empty() {
            return None;
        }
        let key = GatewayKey::new(server, id);
        let remembered = self.store_gateway(&key, &credentials);
        if remembered {
            address.gateway = gateway::kinds(&credentials);
        }
        Some(NewGateway {
            key,
            credentials,
            count,
            remembered,
        })
    }

    /// Uses a new server's gateway, stored by [`Shared::store_new_gateway`] and saved: an address already in use (the same
    /// address set up again) gets the new credentials in place, so every client of it carries them; otherwise `in_use` becomes
    /// the address's.
    pub(super) fn adopt_new_gateway(&self, stored: NewGateway, in_use: &Arc<Gateway>) {
        let NewGateway {
            key,
            credentials,
            count,
            remembered,
        } = stored;
        match self.gateway_of(&key.server, key.address) {
            Some(existing) if !Arc::ptr_eq(&existing, in_use) => {
                existing.replace(credentials);
                self.gateway_stored(&key.server, key.address, remembered, existing.changes());
            }
            _ => self.adopt_gateway(&key.server, key.address, in_use.clone(), remembered, count),
        }
    }

    /// Deletes a gateway secret [`Shared::store_new_gateway`] wrote for a settings write that then failed. Blocking.
    pub(super) fn unstore_new_gateway(&self, stored: &NewGateway) {
        if let (Some(storage), true) = (&self.storage, stored.remembered) {
            let _ = delete_gateway_secret(storage.secrets.as_ref(), &stored.key);
        }
    }

    /// Makes `gateway` the address's, as stored at `count` (when `remembered`).
    pub(super) fn adopt_gateway(
        &self,
        server: &ServerRef,
        address: AddressId,
        gateway: Arc<Gateway>,
        remembered: bool,
        count: u64,
    ) {
        lock(&self.gateways).insert(
            (server.clone(), address),
            GatewayEntry {
                gateway,
                saved: count,
                remembered,
            },
        );
    }

    /// The address's gateway in use, if loaded.
    pub(crate) fn gateway_of(
        &self,
        server: &ServerRef,
        address: AddressId,
    ) -> Option<Arc<Gateway>> {
        lock(&self.gateways)
            .get(&(server.clone(), address))
            .map(|entry| entry.gateway.clone())
    }

    /// Records that new credentials for the address are in use and, when `remembered`, stored at `count`.
    pub(crate) fn gateway_stored(
        &self,
        server: &ServerRef,
        address: AddressId,
        remembered: bool,
        count: u64,
    ) {
        if let Some(entry) = lock(&self.gateways).get_mut(&(server.clone(), address)) {
            entry.remembered = remembered;
            entry.saved = count;
        }
    }

    /// Drops the server's gateways (the server was removed).
    pub(super) fn forget_gateways(&self, server: &ServerRef) {
        lock(&self.gateways).retain(|(owner, _), _| owner != server);
        lock(&self.gateway_address).remove(server);
    }

    /// Saves the server's gateway credentials that changed since they were last stored (a gateway's rotated cookies, ADR 0017),
    /// only for addresses whose credentials are in the keyring. Nothing is written when no count moved.
    ///
    /// Runs under the session lock, as removing the server and a sign-in to the gateway do: a rotation can never land after the
    /// removal deleted the secret, nor over the credentials of a newer sign-in to the gateway. Under it, the server and each
    /// address must still be saved with gateway kinds, and the count must still differ.
    pub(super) async fn save_rotations(self: &Arc<Self>, server: &ServerRef) {
        if self.storage.is_none() {
            return;
        }
        let _changes = self.changes.lock().await;
        let Some(saved) = self.saved(server) else {
            return;
        };
        let moved: Vec<(AddressId, Arc<Gateway>)> = lock(&self.gateways)
            .iter()
            .filter(|((owner, _), entry)| {
                owner == server && entry.remembered && entry.gateway.changes() != entry.saved
            })
            .map(|((_, address), entry)| (*address, entry.gateway.clone()))
            .filter(|(address, _)| {
                saved
                    .addresses
                    .iter()
                    .any(|a| a.id == Some(*address) && !a.gateway.is_empty())
            })
            .collect();
        if moved.is_empty() {
            return;
        }
        let shared = self.clone();
        let server = server.clone();
        let _ = blocking("save", move || {
            for (address, gateway) in moved {
                let (credentials, count) = gateway.snapshot();
                let still_moved = lock(&shared.gateways)
                    .get(&(server.clone(), address))
                    .is_some_and(|entry| {
                        Arc::ptr_eq(&entry.gateway, &gateway) && entry.saved != count
                    });
                if still_moved
                    && shared.store_gateway(&GatewayKey::new(server.clone(), address), &credentials)
                {
                    shared.gateway_stored(&server, address, true, count);
                }
            }
        })
        .await;
    }

    /// What the session's client reports (the 401 rule, ADR 0015 as amended by ADR 0017). An unconfirmed answer is checked with
    /// the identity probe; a gateway's challenge shows "Sign in to the gateway again" at once.
    pub(super) fn session_event(
        self: &Arc<Self>,
        server: &ServerRef,
        id: u64,
        client: &Weak<JellyfinClient>,
        event: SessionEvent,
    ) {
        match event {
            SessionEvent::Gateway(_) => self.needs_gateway_sign_in(server, id, None),
            SessionEvent::Unconfirmed {
                token,
                unauthorized,
            } => {
                let Some(client) = client.upgrade() else {
                    return;
                };
                let Ok(runtime) = RuntimeHandle::try_current() else {
                    // Nothing can check without a runtime: let requests go out again.
                    client.resume();
                    return;
                };
                self.set_confirming(server, id, true);
                let shared = self.clone();
                let server = server.clone();
                runtime.spawn(async move {
                    shared
                        .confirm(&server, id, token, unauthorized, &client)
                        .await;
                    shared.set_confirming(&server, id, false);
                });
            }
        }
    }

    /// The 401 rule's second part: re-runs the identity probe through the server's provider, with the address's gateway
    /// credentials and never the token. Jellyfin's answer with the server's id after a 401 (`unauthorized`): the 401 was
    /// Jellyfin's, the session ends (ADR 0015); after a page or a redirect, which Jellyfin never sends, nothing changes (a proxy
    /// rule blocking one path, say). A gateway's answer: "Sign in to the gateway again", session and token kept. No answer: nothing changes. The client
    /// holds its requests back meanwhile and lets them go once this decides, so at most one probe runs per session.
    async fn confirm(
        self: &Arc<Self>,
        server: &ServerRef,
        id: u64,
        token: AccessToken,
        unauthorized: bool,
        client: &JellyfinClient,
    ) {
        let link = lock(&self.links)
            .get(server)
            .map(|link| (link.provider.clone(), link.identity.clone()));
        let Some((provider, identity)) = link else {
            client.resume();
            return;
        };
        let answer = match ServerClient::new(
            server.clone(),
            provider.clone(),
            None,
            self.request_log.clone(),
            &probe_config(),
        ) {
            Ok(probe) => Some(identity.probe_client(&probe).await),
            Err(_) => None,
        };
        match answer {
            Some(Ok(_)) if unauthorized => {
                if client.revoke(&token) {
                    self.revoked(server, id);
                }
            }
            Some(Err(error)) if gateway_answered(&error) => {
                let address = provider
                    .current()
                    .and_then(|selected| selected.position())
                    .and_then(|position| self.address_at(server, position));
                self.needs_gateway_sign_in(server, id, address);
                client.resume();
            }
            _ => client.resume(),
        }
    }

    fn set_confirming(&self, server: &ServerRef, id: u64, running: bool) {
        if let Some(session) = lock(&self.sessions).get(server).filter(|s| s.id == id) {
            session.confirming.send_replace(running);
        }
    }

    /// Waits until no confirming probe runs for session `id` (or it is gone).
    pub(super) async fn confirmed(&self, server: &ServerRef, id: u64) {
        let receiver = lock(&self.sessions)
            .get(server)
            .filter(|s| s.id == id)
            .map(|s| s.confirming.subscribe());
        if let Some(mut receiver) = receiver {
            // A dropped session closes the channel: nothing to wait for then.
            let _ = receiver.wait_for(|running| !*running).await;
        }
    }

    /// The saved address at 1-based `position`.
    fn address_at(&self, server: &ServerRef, position: u8) -> Option<AddressId> {
        let saved = self.saved(server)?;
        saved
            .addresses
            .get(usize::from(position).checked_sub(1)?)
            .and_then(|address| address.id)
    }

    /// Shows "Sign in to the gateway again" while session `id` is the server's current one, and remembers which address asked
    /// (when known) for [`App::sign_in_to_gateway`](crate::App::sign_in_to_gateway). The session's user is checked again once
    /// the server is reached, which clears the status when the gateway lets it through.
    pub(super) fn needs_gateway_sign_in(
        self: &Arc<Self>,
        server: &ServerRef,
        id: u64,
        address: Option<AddressId>,
    ) {
        if self.session_id(server) != Some(id) {
            return;
        }
        if let Some(address) = address {
            lock(&self.gateway_address).insert(server.clone(), address);
        }
        if let Some(session) = lock(&self.sessions).get(server) {
            let _ = session.check.compare_exchange(
                super::CHECKED,
                UNCHECKED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        self.update_session_status(server, id, |status| status.gateway_sign_in_needed = true);
    }

    /// The address a gateway sign-in is for: the one that last asked, else the address in use, else the first with gateway
    /// credentials, else the first.
    pub(crate) fn gateway_address(&self, server: &ServerRef) -> Option<(AddressId, Url, bool)> {
        let saved = self.saved(server)?;
        let asked = lock(&self.gateway_address).get(server).copied();
        let in_use = self
            .capabilities
            .borrow()
            .server(server)
            .and_then(|status| status.address)
            .and_then(|position| self.address_at(server, position));
        let address = asked
            .or(in_use)
            .and_then(|id| saved.addresses.iter().find(|a| a.id == Some(id)))
            .or_else(|| saved.addresses.iter().find(|a| !a.gateway.is_empty()))
            .or_else(|| saved.addresses.first())?;
        let url = Url::parse(&address.url).ok()?;
        Some((address.id?, url, address.plain_http_confirmed))
    }

    /// Saves new gateway credentials for a saved address after a sign-in to the gateway again: in the keyring, then the kinds and
    /// (for a Basic gateway, ADR 0017) Jellyfin's header placement in the settings; then every client of the address carries them
    /// and the session resumes. Returns whether the keyring kept them: without, they last for this run only.
    ///
    /// # Errors
    /// The server is gone ([`StoreError::Io`] is never returned for that: `Ok(None)`), or the settings could not be saved.
    pub(crate) async fn save_gateway_sign_in(
        self: &Arc<Self>,
        server: &ServerRef,
        address: AddressId,
        credentials: GatewayCredentials,
    ) -> Result<Option<bool>, StoreError> {
        let _changes = self.changes.lock().await;
        if self.saved(server).is_none() {
            return Ok(None);
        }
        let shared = self.clone();
        let owner = server.clone();
        let stored = credentials.clone();
        let remembered = blocking("save", move || {
            let remembered =
                shared.store_gateway(&GatewayKey::new(owner.clone(), address), &stored);
            shared.commit(|servers, _| {
                let Some(saved) = servers.iter_mut().find(|s| s.server == owner) else {
                    return;
                };
                let current = gateway::placement(&saved.header_placement);
                saved.header_placement =
                    gateway::stored_placement(HeaderPlacement::for_gateway(&stored, current));
                if remembered
                    && let Some(entry) = saved.addresses.iter_mut().find(|a| a.id == Some(address))
                {
                    entry.gateway = gateway::kinds(&stored);
                }
            })?;
            Ok::<_, StoreError>(remembered)
        })
        .await??;
        let in_use = self
            .gateway_of(server, address)
            .unwrap_or_else(|| Arc::new(Gateway::new(GatewayCredentials::default())));
        in_use.replace(credentials);
        self.adopt_gateway(
            server,
            address,
            in_use.clone(),
            remembered,
            in_use.changes(),
        );
        self.gateway_signed_in(server);
        Ok(Some(remembered))
    }

    /// The server's provider could reach none of its addresses: when it has a session and an address with gateway credentials,
    /// probes those addresses once (until the next network change) with their gateway credentials. A gateway's answer shows
    /// "Sign in to the gateway again": an expired gateway session must not look like a server that is down.
    pub(super) fn probe_gateways_when_unreachable(self: &Arc<Self>, server: &ServerRef) {
        let Some(session) = self.session_id(server) else {
            return;
        };
        let Some(saved) = self.saved(server) else {
            return;
        };
        if saved
            .addresses
            .iter()
            .all(|address| address.gateway.is_empty())
        {
            return;
        }
        let first = lock(&self.links)
            .get(server)
            .is_some_and(|link| !link.gateways_probed.swap(true, Ordering::AcqRel));
        let Ok(runtime) = RuntimeHandle::try_current() else {
            return;
        };
        if !first {
            return;
        }
        let shared = self.clone();
        let server = server.clone();
        runtime.spawn(async move {
            let prober = Prober::new(server.clone(), shared.request_log.clone());
            for address in saved.addresses.iter().filter(|a| !a.gateway.is_empty()) {
                let (Some(id), Ok(url)) = (address.id, Url::parse(&address.url)) else {
                    continue;
                };
                let gateway = shared.gateway_of(&server, id);
                let result = prober
                    .probe_expecting_with(&url, &saved.id, gateway, address.plain_http_confirmed)
                    .await;
                if let Err(error) = result
                    && gateway_answered(&error)
                {
                    shared.needs_gateway_sign_in(&server, session, Some(id));
                    return;
                }
            }
        });
    }

    /// New gateway credentials for a saved address are in use and saved (a sign-in to the gateway again): the status clears, the
    /// session's client takes the saved header placement, and the session's user is checked again.
    pub(crate) fn gateway_signed_in(self: &Arc<Self>, server: &ServerRef) {
        lock(&self.gateway_address).remove(server);
        if let (Some(client), Some(saved)) = (self.session(server), self.saved(server)) {
            client.set_header_placement(gateway::placement(&saved.header_placement));
        }
        if let Some(link) = lock(&self.links).get(server) {
            link.gateways_probed.store(false, Ordering::Release);
            link.provider.retry_now();
        }
        self.update_status(server, |status| status.gateway_sign_in_needed = false);
        if let Some(session) = lock(&self.sessions).get(server) {
            let _ = session.check.compare_exchange(
                super::CHECKED,
                UNCHECKED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        if self
            .capabilities
            .borrow()
            .server(server)
            .is_some_and(|status| status.reachability == Reachability::Reachable)
        {
            self.recheck(server);
        }
    }
}
