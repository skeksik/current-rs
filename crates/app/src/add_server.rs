//! The "add a server" flow: the user types an address, the core probes it and either reports why it cannot be used, asks the user to
//! confirm plain HTTP, or hands back a server ready to add (ADR 0012). A server found on the local network can be picked instead of
//! typing its address (ADR 0016). Behind an authentication gateway (ADR 0017) the flow asks for the gateway's password, waits for
//! a web sign-in's cookies or takes extra headers, and probes the address again with them.
//!
//! In memory only: storing the server is `current-store`'s, signing in is a later step. The gateway credentials the user gave
//! stay in the flow and in the [`NewServer`] it hands back, never in a state the UI renders.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use current_jellyfin::{FoundServer, ProbeError, Prober, Realm};
use current_model::{PublicServerInfo, ServerAddress, ServerKey, ServerRef, ServiceType};
use current_net::{Gateway, GatewayCredentials, Location, RequestLog, Transport, Url, transport};
use tokio::sync::watch;

use crate::discovery::DiscoveredServer;
use crate::gateway::{self, CookieInput, GatewaySubmitError, HeaderInput};

/// A server the flow found, with the address it answered on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewServer {
    pub info: PublicServerInfo,
    pub address: ServerAddress,
    /// https was tried and failed its handshake before plain http answered: the server may offer https with a certificate this
    /// device does not trust. The UI says so.
    pub https_failed_tls: bool,
    /// The gateway credentials the address needed, empty without a gateway. Saved with the address when the sign-in succeeds; its
    /// Debug output shows no value.
    pub gateway: GatewayCredentials,
}

impl NewServer {
    pub(crate) fn from_found(found: &FoundServer) -> Self {
        Self {
            address: found.server_address(false),
            https_failed_tls: found.https_failed_tls,
            info: found.info.clone(),
            gateway: GatewayCredentials::default(),
        }
    }
}

/// Where the flow is. The UI renders one screen state per variant. No variant holds a password, a cookie or a header value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AddServerState {
    /// Waiting for an address.
    #[default]
    Idle,
    /// Probing what the user typed, or the address again with the gateway credentials the user gave.
    Probing { input: String },
    /// No usable server at that address.
    Failed { input: String, error: ProbeError },
    /// A server answered over plain HTTP outside the local network: it is used only if the user confirms (once per address).
    ConfirmPlainHttp { server: NewServer },
    /// A server ready to add.
    Ready { server: NewServer },
    /// A password gateway (HTTP Basic) in front of the address: the UI asks for its user name and password
    /// ([`AddServer::submit_password`]). `origin` is the address's scheme, host and port; `realm` is the site's own text, shown
    /// only as "The site says: ..." (Debug leaves it out). `wrong` is true when the password just given was refused.
    /// `plain_http`: plain http outside the local network, so the password goes only once the user confirms that.
    GatewayPassword {
        input: String,
        origin: String,
        realm: Option<Realm>,
        wrong: bool,
        plain_http: bool,
    },
    /// The address shows a web page where the server's answer was expected: a gateway's sign-in, or a plain web site. The UI
    /// says so ("This address shows a web page. If it is your server's sign-in, sign in; otherwise check the address.") and
    /// offers "Sign in", which opens the sign-in window on `url` (the address minus the API path). After each navigation that
    /// lands on `origin`, the UI hands the window's cookies over ([`AddServer::submit_cookies`]): the flow moves on when they let
    /// the server through, and stays here otherwise. `redirected` is true for a redirect to another host or port, which may as
    /// well be a moved server: the UI then also offers "Use the address it points to" (`location`, when the server sent one).
    GatewaySignIn {
        input: String,
        url: String,
        origin: String,
        redirected: bool,
        location: Option<Location>,
        plain_http: bool,
    },
    /// The gateway refused access and shows no sign-in: it wants something else, such as a service token. The UI offers
    /// "Advanced: extra headers" with an explanation ([`AddServer::submit_headers`]).
    GatewayRefused {
        input: String,
        origin: String,
        plain_http: bool,
    },
    /// The address asks for a sign-in method Current does not support (Digest, NTLM, Negotiate, Bearer, ...).
    GatewayUnsupported { input: String, origin: String },
}

/// The address a gateway answered on, with the credentials given so far. Never in a state.
#[derive(Debug)]
struct Pending {
    input: String,
    base: Url,
    credentials: GatewayCredentials,
    /// The user confirmed plain http to this address.
    confirmed: bool,
}

/// The flow's state and actions. A newer probe or a reset supersedes a probe still running: its result is dropped.
#[derive(Debug)]
pub struct AddServer {
    prober: Prober,
    state: watch::Sender<AddServerState>,
    generation: AtomicU64,
    pending: Mutex<Option<Pending>>,
}

impl AddServer {
    /// A flow whose probes are logged to `log` as `jellyfin/setup`.
    pub fn new(log: Arc<RequestLog>) -> Self {
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("setup"));
        Self::with_prober(Prober::new(server, log))
    }

    pub fn with_prober(prober: Prober) -> Self {
        Self {
            prober,
            state: watch::Sender::new(AddServerState::Idle),
            generation: AtomicU64::new(0),
            pending: Mutex::new(None),
        }
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        // Replaced whole under the lock, so a poisoned one still holds a usable value.
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The state now, and every change after.
    pub fn state(&self) -> watch::Receiver<AddServerState> {
        self.state.subscribe()
    }

    /// Probes `input` and moves to `Failed`, `ConfirmPlainHttp`, `Ready` or a gateway state, unless a newer probe or a reset came
    /// first. The first probe of an address carries no credentials (ADR 0017).
    pub async fn probe(&self, input: &str) {
        let generation = self.begin(input);
        *self.lock_pending() = None;
        let next = match self.prober.probe_reporting(input).await {
            Ok(found) => settle(found),
            Err((error, base)) => match base {
                Some(base) => {
                    let pending = Pending {
                        input: input.to_owned(),
                        base: *base,
                        credentials: GatewayCredentials::default(),
                        confirmed: false,
                    };
                    let state = gateway_state(&pending, error, false);
                    self.keep_if_current(generation, pending);
                    state
                }
                None => AddServerState::Failed {
                    input: input.to_owned(),
                    error,
                },
            },
        };
        self.set_if_current(generation, next);
    }

    /// Takes a server found on the local network: `Ready`, or `ConfirmPlainHttp` under the same rule as a typed address. It was
    /// probed by the search, so it is not probed again; a probe still running is dropped.
    pub fn pick(&self, server: &DiscoveredServer) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.lock_pending() = None;
        self.state.send_replace(settle(server.found.clone()));
    }

    /// The user agreed to plain HTTP: `ConfirmPlainHttp` becomes `Ready`, with the confirmation kept on the address. Returns whether
    /// there was anything to confirm.
    pub fn confirm_plain_http(&self) -> bool {
        self.state.send_if_modified(|state| match state {
            AddServerState::ConfirmPlainHttp { server } => {
                let mut server = server.clone();
                server.address.plain_http_confirmed = true;
                *state = AddServerState::Ready { server };
                true
            }
            _ => false,
        })
    }

    /// The gateway's user name and password, in `GatewayPassword`: the address is probed again with them. Moves on when they let
    /// the server through; a refused password asks again (`wrong`). `confirm_plain_http` is the user's agreement to plain http,
    /// needed where the state says `plain_http`.
    ///
    /// # Errors
    /// [`GatewaySubmitError`] when nothing asks for a password, plain http is not confirmed, or the user name or password cannot
    /// be sent (a `:` in the user name, a control character). Nothing is sent then.
    pub async fn submit_password(
        &self,
        user: &str,
        password: &str,
        confirm_plain_http: bool,
    ) -> Result<(), GatewaySubmitError> {
        let seen = self.generation.load(Ordering::SeqCst);
        if !matches!(*self.state.borrow(), AddServerState::GatewayPassword { .. }) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let basic = gateway::basic(user, password)?;
        self.submit(seen, confirm_plain_http, true, |current| {
            Ok(gateway::merge(current, Some(basic), None, None)?)
        })
        .await
    }

    /// The sign-in window's cookies, in `GatewaySignIn`, after a navigation that landed on the server's origin. Only cookies that
    /// apply to the server's address are kept (RFC 6265); the address is probed with them, and nothing else changes until they
    /// let the server through: the window stays open meanwhile. A later gateway answer of another kind (a password, a refusal)
    /// moves to its state.
    ///
    /// # Errors
    /// [`GatewaySubmitError::NotAsked`] outside `GatewaySignIn`, [`GatewaySubmitError::PlainHttpNotConfirmed`]. Nothing is sent
    /// then, nor when no cookie applies.
    pub async fn submit_cookies(
        &self,
        cookies: &[CookieInput],
        confirm_plain_http: bool,
    ) -> Result<(), GatewaySubmitError> {
        let seen = self.generation.load(Ordering::SeqCst);
        if !matches!(*self.state.borrow(), AddServerState::GatewaySignIn { .. }) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let base = match &*self.lock_pending() {
            Some(pending) => pending.base.clone(),
            None => return Err(GatewaySubmitError::NotAsked),
        };
        let kept = gateway::cookies(&base, cookies);
        if kept.is_empty() {
            // Nothing that could let the server through: no probe.
            return Ok(());
        }
        self.submit(seen, confirm_plain_http, false, |current| {
            Ok(gateway::merge(current, None, None, Some(kept))?)
        })
        .await
    }

    /// Extra headers (advanced), in any gateway state but `GatewayUnsupported`: the address is probed again with them.
    ///
    /// # Errors
    /// [`GatewaySubmitError`] when nothing asks for them, plain http is not confirmed, no header was given, or a name or value
    /// cannot be used (`Authorization`, the names Current sets itself, text outside visible ASCII).
    pub async fn submit_headers(
        &self,
        headers: &[HeaderInput],
        confirm_plain_http: bool,
    ) -> Result<(), GatewaySubmitError> {
        let seen = self.generation.load(Ordering::SeqCst);
        if !matches!(
            *self.state.borrow(),
            AddServerState::GatewayPassword { .. }
                | AddServerState::GatewaySignIn { .. }
                | AddServerState::GatewayRefused { .. }
        ) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let headers = gateway::headers(headers)?;
        self.submit(seen, confirm_plain_http, true, |current| {
            Ok(gateway::merge(current, None, Some(headers), None)?)
        })
        .await
    }

    /// Probes the pending address with the credentials `change` makes from those given so far. `seen` is the probe generation
    /// whose state asked for them: if a reset or a newer probe came since, nothing is sent, so the values never reach another
    /// address. `show_probing`: the UI shows `Probing` meanwhile (not for cookies, whose window stays open).
    async fn submit(
        &self,
        seen: u64,
        confirm_plain_http: bool,
        show_probing: bool,
        change: impl FnOnce(&GatewayCredentials) -> Result<GatewayCredentials, GatewaySubmitError>,
    ) -> Result<(), GatewaySubmitError> {
        let asked_password = matches!(*self.state.borrow(), AddServerState::GatewayPassword { .. });
        let (input, base, credentials, confirmed) = {
            let pending = self.lock_pending();
            // Every probe and reset changes the generation before it touches `pending`, so under this lock it tells whether
            // `pending` is still the address the state asked for.
            if self.generation.load(Ordering::SeqCst) != seen {
                return Err(GatewaySubmitError::NotAsked);
            }
            let Some(pending) = pending.as_ref() else {
                return Err(GatewaySubmitError::NotAsked);
            };
            let confirmed = pending.confirmed || confirm_plain_http;
            if transport(&pending.base) == Transport::PlainPublic && !confirmed {
                return Err(GatewaySubmitError::PlainHttpNotConfirmed);
            }
            (
                pending.input.clone(),
                pending.base.clone(),
                change(&pending.credentials)?,
                confirmed,
            )
        };
        let generation = if show_probing {
            self.begin(&input)
        } else {
            self.generation.load(Ordering::SeqCst)
        };
        let shared = Arc::new(Gateway::new(credentials));
        let result = self
            .prober
            .probe_url_with(&base, Some(shared.clone()), confirmed)
            .await;
        // Cookies the gateway rotated while answering are the ones to keep.
        let (credentials, _) = shared.snapshot();
        let pending = Pending {
            input: input.clone(),
            base,
            credentials: credentials.clone(),
            confirmed,
        };
        let next = match result {
            Ok(found) => {
                let mut server = NewServer::from_found(&found);
                server.address.plain_http_confirmed = confirmed;
                server.gateway = credentials;
                if found.needs_confirmation() && !confirmed {
                    AddServerState::ConfirmPlainHttp { server }
                } else {
                    AddServerState::Ready { server }
                }
            }
            Err(error) if is_gateway(&error) => {
                // A password asked for again right after one was given: it was wrong (or expired).
                let wrong = asked_password && matches!(error, ProbeError::GatewayPassword { .. });
                if !show_probing
                    && matches!(
                        error,
                        ProbeError::GatewaySignIn | ProbeError::Redirected { .. }
                    )
                {
                    // Still the sign-in page: keep waiting, the window stays open.
                    self.keep_if_current(generation, pending);
                    return Ok(());
                }
                let state = gateway_state(&pending, error, wrong);
                self.keep_if_current(generation, pending);
                state
            }
            // Not reached after a web sign-in: keep waiting, the next navigation tries again.
            Err(_) if !show_probing => return Ok(()),
            Err(error) => AddServerState::Failed { input, error },
        };
        self.set_if_current(generation, next);
        Ok(())
    }

    /// Back to `Idle`, dropping any probe still running and the gateway credentials given so far.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.lock_pending() = None;
        self.state.send_if_modified(|state| {
            let changed = *state != AddServerState::Idle;
            *state = AddServerState::Idle;
            changed
        });
    }

    /// Starts a probe: a new generation, shown as `Probing`.
    fn begin(&self, input: &str) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.send_replace(AddServerState::Probing {
            input: input.to_owned(),
        });
        generation
    }

    fn keep_if_current(&self, generation: u64, pending: Pending) {
        let mut slot = self.lock_pending();
        if self.generation.load(Ordering::SeqCst) == generation {
            *slot = Some(pending);
        }
    }

    fn set_if_current(&self, generation: u64, next: AddServerState) {
        // Checked and replaced under the channel's lock, so a reset or newer probe in between always wins.
        self.state.send_if_modified(|state| {
            if self.generation.load(Ordering::SeqCst) == generation {
                *state = next;
                true
            } else {
                false
            }
        });
    }
}

/// Whether setup handles `error` with a gateway state: a gateway's answer, or a redirect to another host or port (a portal or a
/// moved server).
fn is_gateway(error: &ProbeError) -> bool {
    error.is_gateway() || error.offers_sign_in()
}

/// The state for a probe of `pending`'s address that failed with `error`.
fn gateway_state(pending: &Pending, error: ProbeError, wrong: bool) -> AddServerState {
    let input = pending.input.clone();
    let origin = gateway::origin(&pending.base);
    let plain_http = transport(&pending.base) == Transport::PlainPublic && !pending.confirmed;
    match error {
        ProbeError::GatewayPassword { realm } => AddServerState::GatewayPassword {
            input,
            origin,
            realm,
            wrong,
            plain_http,
        },
        ProbeError::GatewaySignIn => AddServerState::GatewaySignIn {
            input,
            url: pending.base.as_str().trim_end_matches('/').to_owned(),
            origin,
            redirected: false,
            location: None,
            plain_http,
        },
        ProbeError::Redirected { location, .. } => AddServerState::GatewaySignIn {
            input,
            url: pending.base.as_str().trim_end_matches('/').to_owned(),
            origin,
            redirected: true,
            location,
            plain_http,
        },
        ProbeError::GatewayRefused => AddServerState::GatewayRefused {
            input,
            origin,
            plain_http,
        },
        ProbeError::UnsupportedChallenge => AddServerState::GatewayUnsupported { input, origin },
        error => AddServerState::Failed { input, error },
    }
}

/// The state a found server leads to.
fn settle(found: FoundServer) -> AddServerState {
    let server = NewServer::from_found(&found);
    if found.needs_confirmation() {
        AddServerState::ConfirmPlainHttp { server }
    } else {
        AddServerState::Ready { server }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use current_model::ServerId;
    use current_net::{Transport, Url};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    use super::*;

    const INFO: &str = r#"{"ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"0123456789abcdef0123456789abcdef","StartupWizardCompleted":true}"#;

    /// A Jellyfin stand-in on 127.0.0.1 that answers once `release` is notified (at once if `None`).
    async fn stand_in(release: Option<Arc<Notify>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let release = release.clone();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = socket.read(&mut buffer).await;
                    if let Some(release) = release {
                        release.notified().await;
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{INFO}",
                        INFO.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn flow() -> AddServer {
        AddServer::new(Arc::new(RequestLog::default()))
    }

    fn found(address: &str) -> FoundServer {
        let address = Url::parse(address).unwrap();
        FoundServer {
            info: PublicServerInfo {
                id: ServerId::new("1"),
                name: "Test Server".into(),
                version: "12.2.0".into(),
                product: "Jellyfin Server".into(),
                local_address: None,
                setup_completed: true,
            },
            transport: current_net::transport(&address),
            address,
            https_failed_tls: false,
        }
    }

    #[tokio::test]
    async fn a_local_server_is_ready_at_once() {
        let address = stand_in(None).await;
        let flow = flow();
        let rx = flow.state();
        flow.probe(&address).await;
        let AddServerState::Ready { server } = rx.borrow().clone() else {
            panic!("{:?}", *rx.borrow())
        };
        assert_eq!(server.address.url, address);
        assert!(!server.address.plain_http_confirmed);
        assert_eq!(server.info.name, "Test Server");
    }

    #[tokio::test]
    async fn a_failure_keeps_the_input() {
        let flow = flow();
        flow.probe("ftp://media.example.com").await;
        assert!(matches!(
            &*flow.state().borrow(),
            AddServerState::Failed { input, error: ProbeError::InvalidAddress(_) } if input == "ftp://media.example.com"
        ));
        flow.probe("http://127.0.0.1:1").await;
        assert!(matches!(
            &*flow.state().borrow(),
            AddServerState::Failed {
                error: ProbeError::Unreachable,
                ..
            }
        ));
    }

    #[test]
    fn plain_http_outside_the_local_network_is_confirmed_once() {
        let state = settle(found("http://media.example.com:8096/"));
        let AddServerState::ConfirmPlainHttp { server } = &state else {
            panic!("{state:?}")
        };
        assert_eq!(server.address.url, "http://media.example.com:8096");
        assert!(!server.address.plain_http_confirmed);

        let flow = flow();
        assert!(!flow.confirm_plain_http());
        flow.state.send_replace(state);
        assert!(flow.confirm_plain_http());
        let AddServerState::Ready { server } = flow.state().borrow().clone() else {
            panic!("not ready")
        };
        assert!(server.address.plain_http_confirmed);
        assert!(!flow.confirm_plain_http());
    }

    #[test]
    fn https_and_local_http_need_no_confirmation() {
        assert!(matches!(
            settle(found("https://media.example.com/jellyfin")),
            AddServerState::Ready { .. }
        ));
        let local = found("http://192.168.1.10:8096/");
        assert_eq!(local.transport, Transport::PlainLocal);
        assert!(matches!(settle(local), AddServerState::Ready { .. }));
    }

    #[tokio::test]
    async fn a_newer_probe_wins_over_a_slower_older_one() {
        let release = Arc::new(Notify::new());
        let slow = stand_in(Some(release.clone())).await;
        let fast = stand_in(None).await;
        let flow = Arc::new(flow());

        let older = tokio::spawn({
            let flow = flow.clone();
            async move { flow.probe(&slow).await }
        });
        // Let the older probe reach the server.
        tokio::time::sleep(Duration::from_millis(100)).await;
        flow.probe(&fast).await;
        release.notify_one();
        older.await.unwrap();

        let AddServerState::Ready { server } = flow.state().borrow().clone() else {
            panic!("not ready")
        };
        assert_eq!(server.address.url, fast);
    }

    #[tokio::test]
    async fn a_reset_drops_a_running_probe() {
        let release = Arc::new(Notify::new());
        let slow = stand_in(Some(release.clone())).await;
        let flow = Arc::new(flow());
        let probe = tokio::spawn({
            let flow = flow.clone();
            async move { flow.probe(&slow).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            *flow.state().borrow(),
            AddServerState::Probing { .. }
        ));
        flow.reset();
        release.notify_one();
        probe.await.unwrap();
        assert_eq!(*flow.state().borrow(), AddServerState::Idle);
    }

    #[tokio::test]
    async fn no_gateway_credential_goes_over_unconfirmed_public_http() {
        let flow = flow();
        *flow.lock_pending() = Some(Pending {
            input: "media.example.com".into(),
            base: Url::parse("http://media.example.com:8096/").unwrap(),
            credentials: GatewayCredentials::default(),
            confirmed: false,
        });
        flow.state.send_replace(AddServerState::GatewayPassword {
            input: "media.example.com".into(),
            origin: "http://media.example.com:8096".into(),
            realm: None,
            wrong: false,
            plain_http: true,
        });
        assert_eq!(
            flow.submit_password("gate-user", "gate-password", false)
                .await,
            Err(GatewaySubmitError::PlainHttpNotConfirmed)
        );
        // Nothing was probed: the state is as it was.
        assert!(matches!(
            *flow.state().borrow(),
            AddServerState::GatewayPassword { .. }
        ));
        flow.reset();
        assert!(flow.lock_pending().is_none());
        assert_eq!(
            flow.submit_password("gate-user", "gate-password", true)
                .await,
            Err(GatewaySubmitError::NotAsked)
        );
    }

    #[test]
    fn gateway_states_show_no_realm_location_or_credential() {
        let base = Url::parse("https://media.example.com/").unwrap();
        let pending = Pending {
            input: "media.example.com".into(),
            base: base.clone(),
            credentials: gateway::merge(
                &GatewayCredentials::default(),
                Some(gateway::basic("gate-user", "gate-password").unwrap()),
                None,
                None,
            )
            .unwrap(),
            confirmed: false,
        };
        let mut text = format!("{pending:?}");
        for error in [
            ProbeError::GatewayPassword {
                realm: Realm::new("Private Realm Text"),
            },
            ProbeError::GatewaySignIn,
            ProbeError::Redirected {
                status: 302,
                location: None,
            },
            ProbeError::GatewayRefused,
            ProbeError::UnsupportedChallenge,
        ] {
            text.push_str(&format!("{:?}", gateway_state(&pending, error, false)));
        }
        let mut server = NewServer::from_found(&found("https://media.example.com/"));
        server.gateway = pending.credentials.clone();
        text.push_str(&format!("{:?}", AddServerState::Ready { server }));
        for secret in ["Private Realm Text", "gate-password"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}
