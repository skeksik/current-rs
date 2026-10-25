//! The M1 flows as bridge objects: adding a server, finding servers on the local network, signing in, and signing in to a
//! gateway again (ADR 0012, ADR 0015, ADR 0016, ADR 0017). Each has a state the UI subscribes to (a value, ADR 0004) and actions.
//! The states carry what a screen shows and nothing more: no token, password, Quick Connect secret, cookie or header value ever
//! crosses the bridge in a state; of an address, only the origin a prompt names and the URL the sign-in window opens. Passwords,
//! cookies and headers go in only as arguments of the actions that send them.
//!
//! Cancelling an action's token returns its flow to where it was before the action (`Idle`, or `Ready` for a sign-in), unless a
//! newer action on the same flow started meanwhile: that one is left alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use std::time::{Duration, SystemTime};

use current_app::{App, CookieInput, GatewaySubmitError, HeaderInput, NewServer};
use current_net::Url;
use tokio::runtime::Handle;

use crate::problems::{
    DiscoveryProblem, GatewayInputProblem, GatewaySignInProblem, ProbeProblem, SignInProblem,
};
use crate::records::{ServerRef, UserSnapshot};
use crate::subscribe::{self, LiveListeners};
use crate::{CancelToken, CoreError, TaskHandle, cancel};

/// A server that setup found, as the confirmation and sign-in screens show it. Its address stays in the core.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NewServerInfo {
    /// The id the server reports for itself.
    pub id: String,
    pub name: String,
    pub version: String,
    /// The product name the server reports, e.g. `Jellyfin Server`.
    pub product: String,
    /// https was tried first and failed on its certificate; the server was reached over plain http instead.
    pub https_failed_tls: bool,
}

impl From<&NewServer> for NewServerInfo {
    fn from(server: &NewServer) -> Self {
        Self {
            id: server.info.id.as_str().to_owned(),
            name: server.info.name.clone(),
            version: server.info.version.clone(),
            product: server.info.product.clone(),
            https_failed_tls: server.https_failed_tls,
        }
    }
}

/// Where adding a server stands. The address the user typed is not repeated here: the UI has it.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AddServerState {
    Idle,
    Probing,
    Failed {
        problem: ProbeProblem,
    },
    /// A server answered over plain http outside the local network: the user confirms once (ADR 0012).
    ConfirmPlainHttp {
        server: NewServerInfo,
    },
    /// Ready to sign in.
    Ready {
        server: NewServerInfo,
    },
    /// A password gateway (HTTP Basic) in front of the address (ADR 0017): the UI asks for its user name and password
    /// ([`AddServerFlow::submit_password`]). `origin` (scheme, host and port) names the address in the prompt. `realm` is the
    /// site's own text: show it only as "The site says: ...", never as a title. `wrong`: the password just given was refused.
    /// `plain_http`: plain http outside the local network, so the prompt also asks to confirm that, and the action passes it.
    GatewayPassword {
        origin: String,
        realm: Option<String>,
        wrong: bool,
        plain_http: bool,
    },
    /// The address shows a web page where the server's answer was expected: a gateway's sign-in, or a plain web site. The UI says
    /// so ("This address shows a web page. If it is your server's sign-in, sign in; otherwise check the address.") and offers
    /// "Sign in", which opens the sign-in window on `url`, titled with `origin`. After each navigation that lands on `origin`, the
    /// UI hands the window's cookies over ([`AddServerFlow::submit_cookies`]); the state moves on once they let the server through.
    /// `redirected`: a redirect to another host or port, which may as well be a moved server: the UI also offers "Use the address
    /// it points to" ([`AddServerFlow::follow_redirect`]), naming `location_origin` when the server sent one.
    GatewaySignIn {
        url: String,
        origin: String,
        redirected: bool,
        location_origin: Option<String>,
        plain_http: bool,
    },
    /// The gateway refuses without a sign-in: it wants something else, such as a service token. The UI offers "Advanced: extra
    /// headers" ([`AddServerFlow::submit_headers`]) with an explanation.
    GatewayRefused {
        origin: String,
        plain_http: bool,
    },
    /// The address asks for a sign-in method Current does not support.
    GatewayUnsupported {
        origin: String,
    },
}

/// Shown in Debug in place of an address (ADR 0006) or a realm (ADR 0017).
const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for AddServerState {
    /// Like a derived Debug, with the gateway states' origin, URL and realm redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("Idle"),
            Self::Probing => f.write_str("Probing"),
            Self::Failed { problem } => f.debug_struct("Failed").field("problem", problem).finish(),
            Self::ConfirmPlainHttp { server } => f
                .debug_struct("ConfirmPlainHttp")
                .field("server", server)
                .finish(),
            Self::Ready { server } => f.debug_struct("Ready").field("server", server).finish(),
            Self::GatewayPassword {
                realm,
                wrong,
                plain_http,
                ..
            } => f
                .debug_struct("GatewayPassword")
                .field("origin", &REDACTED)
                .field("realm", &realm.as_ref().map(|_| REDACTED))
                .field("wrong", wrong)
                .field("plain_http", plain_http)
                .finish(),
            Self::GatewaySignIn {
                redirected,
                location_origin,
                plain_http,
                ..
            } => f
                .debug_struct("GatewaySignIn")
                .field("url", &REDACTED)
                .field("origin", &REDACTED)
                .field("redirected", redirected)
                .field(
                    "location_origin",
                    &location_origin.as_ref().map(|_| REDACTED),
                )
                .field("plain_http", plain_http)
                .finish(),
            Self::GatewayRefused { plain_http, .. } => f
                .debug_struct("GatewayRefused")
                .field("origin", &REDACTED)
                .field("plain_http", plain_http)
                .finish(),
            Self::GatewayUnsupported { .. } => f
                .debug_struct("GatewayUnsupported")
                .field("origin", &REDACTED)
                .finish(),
        }
    }
}

/// The origin (scheme, host and port when not the default) of `url`: what a prompt or a window title names.
fn origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

impl From<&current_app::AddServerState> for AddServerState {
    fn from(state: &current_app::AddServerState) -> Self {
        use current_app::AddServerState as App;
        match state {
            App::Idle => Self::Idle,
            App::Probing { .. } => Self::Probing,
            App::Failed { error, .. } => Self::Failed {
                problem: error.into(),
            },
            App::ConfirmPlainHttp { server } => Self::ConfirmPlainHttp {
                server: server.into(),
            },
            App::Ready { server } => Self::Ready {
                server: server.into(),
            },
            App::GatewayPassword {
                origin,
                realm,
                wrong,
                plain_http,
                ..
            } => Self::GatewayPassword {
                origin: origin.clone(),
                realm: realm.as_ref().map(|realm| realm.as_str().to_owned()),
                wrong: *wrong,
                plain_http: *plain_http,
            },
            App::GatewaySignIn {
                url,
                origin,
                redirected,
                location,
                plain_http,
                ..
            } => Self::GatewaySignIn {
                url: url.clone(),
                origin: origin.clone(),
                redirected: *redirected,
                location_origin: location
                    .as_ref()
                    .map(|location| self::origin(location.url())),
                plain_http: *plain_http,
            },
            App::GatewayRefused {
                origin, plain_http, ..
            } => Self::GatewayRefused {
                origin: origin.clone(),
                plain_http: *plain_http,
            },
            App::GatewayUnsupported { origin, .. } => Self::GatewayUnsupported {
                origin: origin.clone(),
            },
        }
    }
}

/// A cookie the sign-in window's web view holds, read after a navigation (ADR 0017). Values go into the core only; no Debug.
#[derive(uniffi::Record)]
pub struct GatewayCookie {
    pub name: String,
    pub value: String,
    /// As the web view reports it: with a leading dot for a domain cookie, without for a host-only one (RFC 6265 §5.3).
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// When it expires, in milliseconds since 1970 (UTC); `None` for a session cookie.
    pub expires_unix_ms: Option<i64>,
}

impl From<GatewayCookie> for CookieInput {
    fn from(cookie: GatewayCookie) -> Self {
        let expires = cookie.expires_unix_ms.map(|ms| match u64::try_from(ms) {
            Ok(ms) => SystemTime::UNIX_EPOCH + Duration::from_millis(ms),
            // Before 1970: long expired.
            Err(_) => SystemTime::UNIX_EPOCH,
        });
        Self {
            name: cookie.name,
            value: cookie.value,
            domain: cookie.domain,
            path: cookie.path,
            secure: cookie.secure,
            http_only: cookie.http_only,
            expires,
        }
    }
}

/// An extra header the user typed (advanced, ADR 0017). The value goes into the core only; no Debug.
#[derive(uniffi::Record)]
pub struct GatewayHeader {
    pub name: String,
    pub value: String,
}

impl From<GatewayHeader> for HeaderInput {
    fn from(header: GatewayHeader) -> Self {
        Self {
            name: header.name,
            value: header.value,
        }
    }
}

/// The error a gateway action returns for `error`.
fn submit_error(error: &GatewaySubmitError) -> CoreError {
    match GatewayInputProblem::of(error) {
        Some(problem) => CoreError::Gateway { problem },
        None => CoreError::NotReady,
    }
}

/// A server found on the local network.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DiscoveredServerInfo {
    /// The server's id: what [`AddServerFlow::pick`] takes.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Picking it asks to confirm plain http first.
    pub needs_confirmation: bool,
}

impl From<&current_app::DiscoveredServer> for DiscoveredServerInfo {
    fn from(server: &current_app::DiscoveredServer) -> Self {
        Self {
            id: server.info().id.as_str().to_owned(),
            name: server.info().name.clone(),
            version: server.info().version.clone(),
            needs_confirmation: server.needs_confirmation(),
        }
    }
}

/// Where the search for local servers stands. Servers are listed in the order they answered.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DiscoveryState {
    Idle,
    Searching { found: Vec<DiscoveredServerInfo> },
    Done { found: Vec<DiscoveredServerInfo> },
    Failed { problem: DiscoveryProblem },
}

impl From<&current_app::DiscoveryState> for DiscoveryState {
    fn from(state: &current_app::DiscoveryState) -> Self {
        use current_app::DiscoveryState as App;
        let list = |found: &[current_app::DiscoveredServer]| found.iter().map(Into::into).collect();
        match state {
            App::Idle => Self::Idle,
            App::Searching { found } => Self::Searching { found: list(found) },
            App::Done { found } => Self::Done { found: list(found) },
            App::Failed { reason } => Self::Failed {
                problem: reason.into(),
            },
        }
    }
}

/// Where a sign-in stands.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SignInState {
    /// Waiting for the user. `quick_connect` is whether the server offers Quick Connect, once known.
    Ready {
        quick_connect: Option<bool>,
    },
    SigningIn,
    /// The user enters `code` in another signed-in client to approve this device.
    QuickConnectWaiting {
        code: String,
    },
    Failed {
        problem: SignInProblem,
    },
    /// Signed in and saved. `remembered` is false when the token could not go into the keyring: the session lasts for this run.
    SignedIn {
        server: ServerRef,
        user: UserSnapshot,
        remembered: bool,
    },
}

impl From<&current_app::SignInState> for SignInState {
    fn from(state: &current_app::SignInState) -> Self {
        use current_app::SignInState as App;
        match state {
            App::Ready { quick_connect } => Self::Ready {
                quick_connect: *quick_connect,
            },
            App::SigningIn => Self::SigningIn,
            App::QuickConnectWaiting { code } => Self::QuickConnectWaiting { code: code.clone() },
            App::Failed { error } => Self::Failed {
                problem: error.into(),
            },
            App::SignedIn {
                server,
                user,
                remembered,
            } => Self::SignedIn {
                server: server.into(),
                user: user.into(),
                remembered: *remembered,
            },
        }
    }
}

/// Receives a flow's state: the current value first, then every change, on a core thread. It must only hand the value to
/// the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait AddServerStateListener: Send + Sync {
    fn on_value(&self, value: AddServerState);
}

/// See [`AddServerStateListener`].
#[uniffi::export(callback_interface)]
pub trait DiscoveryStateListener: Send + Sync {
    fn on_value(&self, value: DiscoveryState);
}

/// See [`AddServerStateListener`].
#[uniffi::export(callback_interface)]
pub trait SignInStateListener: Send + Sync {
    fn on_value(&self, value: SignInState);
}

/// Counts the actions started on one flow, so that a cancelled action undoes its state only while no newer one has started.
#[derive(Clone, Debug, Default)]
struct Attempts(Arc<AtomicU64>);

impl Attempts {
    fn begin(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_latest(&self, attempt: u64) -> bool {
        self.0.load(Ordering::SeqCst) == attempt
    }
}

/// Runs `work` as a cancellable call on the flow; if `token` cancels it while it is the flow's latest action, runs `undo`. The work is
/// dropped on cancel, so this is only for actions that hold no lock and write nothing (a probe, a search): their states are
/// generation-checked, and a dropped one changes nothing. Sign-ins use [`SignInFlow::attempt`].
async fn attempt<F>(
    runtime: &Handle,
    attempts: &Attempts,
    token: Arc<CancelToken>,
    work: F,
    undo: impl FnOnce(),
) -> Result<(), CoreError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let attempt = attempts.begin();
    let result = cancel::run_cancellable(runtime, token, async move {
        work.await;
        Ok(())
    })
    .await;
    if result == Err(CoreError::Cancelled) && attempts.is_latest(attempt) {
        undo();
    }
    result
}

/// Adding a server: probing an address the user typed, or picking a server discovery found, then confirming plain http if asked,
/// then signing in. One per "add a server" screen.
#[derive(uniffi::Object)]
pub struct AddServerFlow {
    app: Arc<App>,
    flow: Arc<current_app::AddServer>,
    runtime: Handle,
    listeners: LiveListeners,
    attempts: Attempts,
}

impl AddServerFlow {
    pub(crate) fn new(app: Arc<App>, runtime: Handle, listeners: LiveListeners) -> Self {
        let flow = Arc::new(app.add_server());
        Self {
            app,
            flow,
            runtime,
            listeners,
            attempts: Attempts::default(),
        }
    }
}

#[uniffi::export]
impl AddServerFlow {
    /// Subscribes to the state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_state(&self, listener: Box<dyn AddServerStateListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.flow.state(), move |state| {
            listener.listener.on_value(AddServerState::from(&state));
        })
    }

    /// The state now.
    pub fn state(&self) -> AddServerState {
        AddServerState::from(&*self.flow.state().borrow())
    }

    /// Probes the address the user typed. The outcome is the state; a newer probe, a pick or a reset wins over this one.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first: the flow is back to `Idle`.
    pub async fn probe(&self, input: String, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        attempt(
            &self.runtime,
            &self.attempts,
            token,
            async move { flow.probe(&input).await },
            || self.flow.reset(),
        )
        .await
    }

    /// Picks a server that `discovery` found, by its id. False if `discovery` does not list it (any more).
    pub fn pick(&self, discovery: Arc<DiscoveryFlow>, server_id: String) -> bool {
        let found = discovery.flow.state().borrow().clone();
        let (current_app::DiscoveryState::Searching { found }
        | current_app::DiscoveryState::Done { found }) = found
        else {
            return false;
        };
        let Some(server) = found
            .iter()
            .find(|server| server.info().id.as_str() == server_id)
        else {
            return false;
        };
        self.attempts.begin();
        self.flow.pick(server);
        true
    }

    /// Confirms plain http when the state asks for it. False in any other state.
    pub fn confirm_plain_http(&self) -> bool {
        self.flow.confirm_plain_http()
    }

    /// The gateway's user name and password, in `GatewayPassword`: the address is probed again with them, never with a Jellyfin
    /// token. The state moves on when they let the server through, or asks again (`wrong`). `confirm_plain_http` is the user's
    /// agreement where the state says `plain_http`. The password is used for these requests and kept with the server only once a
    /// sign-in saves it (in the keyring).
    ///
    /// # Errors
    /// [`CoreError::NotReady`] in any other state; [`CoreError::Gateway`] when they cannot be used or plain http is not
    /// confirmed (nothing is sent then); [`CoreError::Cancelled`] if `token` is cancelled first: the probe still ends and sets the
    /// state.
    pub async fn submit_password(
        &self,
        user: String,
        password: String,
        confirm_plain_http: bool,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        self.attempts.begin();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_password(&user, &password, confirm_plain_http)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// The sign-in window's cookies, in `GatewaySignIn`, after each navigation that lands on the state's `origin`. Hand over every
    /// cookie the web view holds: only those that apply to the server's address are kept (RFC 6265). The state moves on once they
    /// let the server through, and stays otherwise (keep the window open).
    ///
    /// # Errors
    /// As [`AddServerFlow::submit_password`].
    pub async fn submit_cookies(
        &self,
        cookies: Vec<GatewayCookie>,
        confirm_plain_http: bool,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        let cookies: Vec<CookieInput> = cookies.into_iter().map(Into::into).collect();
        self.attempts.begin();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_cookies(&cookies, confirm_plain_http)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// Extra headers (advanced), in `GatewayPassword`, `GatewaySignIn` or `GatewayRefused`: the address is probed again with them.
    ///
    /// # Errors
    /// As [`AddServerFlow::submit_password`]; [`CoreError::Gateway`] also for a name Current sets itself or a value outside
    /// visible ASCII.
    pub async fn submit_headers(
        &self,
        headers: Vec<GatewayHeader>,
        confirm_plain_http: bool,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        let headers: Vec<HeaderInput> = headers.into_iter().map(Into::into).collect();
        self.attempts.begin();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_headers(&headers, confirm_plain_http)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// "Use the address it points to", in `GatewaySignIn` after a redirect: probes the address the server pointed to, as if the
    /// user had typed it. The address itself stays in the core.
    ///
    /// # Errors
    /// [`CoreError::NotReady`] in any other state, or when the server sent no address; [`CoreError::Cancelled`] as
    /// [`AddServerFlow::probe`].
    pub async fn follow_redirect(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let state = self.flow.state().borrow().clone();
        let current_app::AddServerState::GatewaySignIn {
            redirected: true,
            location: Some(location),
            ..
        } = state
        else {
            return Err(CoreError::NotReady);
        };
        // A server that moved points its info request to the new address: the address is what comes before it.
        let target = location.url().as_str();
        let target = target
            .strip_suffix("/System/Info/Public")
            .unwrap_or(target)
            .to_owned();
        self.probe(target, token).await
    }

    /// Back to `Idle`, dropping a running probe and the gateway credentials given so far.
    pub fn reset(&self) {
        self.attempts.begin();
        self.flow.reset();
    }

    /// A sign-in to the server the state is `Ready` with.
    ///
    /// # Errors
    /// [`CoreError::NotReady`] in any other state; [`CoreError::InvalidAddress`] if the found address cannot take requests.
    pub fn sign_in(&self) -> Result<Arc<SignInFlow>, CoreError> {
        let state = self.flow.state().borrow().clone();
        let current_app::AddServerState::Ready { server } = state else {
            return Err(CoreError::NotReady);
        };
        let flow = self
            .app
            .sign_in(server)
            .map_err(|_| CoreError::InvalidAddress)?;
        Ok(Arc::new(SignInFlow::new(
            flow,
            self.runtime.clone(),
            self.listeners.clone(),
        )))
    }
}

#[cfg(test)]
impl AddServerFlow {
    /// [`AddServerFlow::sign_in`] with Quick Connect polling every `interval`, so tests need not wait 5 s.
    pub(crate) fn sign_in_polling_every(&self, interval: std::time::Duration) -> Arc<SignInFlow> {
        let current_app::AddServerState::Ready { server } = self.flow.state().borrow().clone()
        else {
            panic!("not ready")
        };
        let mut flow = self.app.sign_in(server).unwrap();
        flow.set_quick_connect_timing(interval, std::time::Duration::from_secs(60));
        Arc::new(SignInFlow::new(
            flow,
            self.runtime.clone(),
            self.listeners.clone(),
        ))
    }
}

/// One of the device's up IPv4 interfaces, as the platform lists it before a search (ADR 0016). It is the device's own address:
/// never show or log it. No Debug, for that reason.
#[derive(uniffi::Record)]
pub struct LocalInterface {
    /// The interface's IPv4 address as text, for example `192.168.1.10`.
    pub address: String,
    /// The prefix length of its subnet, 0 to 32 (24 for a netmask of 255.255.255.0).
    pub prefix_length: u8,
}

impl LocalInterface {
    pub(crate) fn parse(&self) -> Option<current_model::LocalInterface> {
        Some(current_model::LocalInterface {
            address: self.address.parse().ok()?,
            prefix_len: self.prefix_length,
        })
    }
}

/// Finding Jellyfin servers on the local network (ADR 0016). One per "add a server" screen.
#[derive(uniffi::Object)]
pub struct DiscoveryFlow {
    flow: Arc<current_app::Discovery>,
    runtime: Handle,
    listeners: LiveListeners,
    attempts: Attempts,
}

impl DiscoveryFlow {
    pub(crate) fn new(app: &App, runtime: Handle, listeners: LiveListeners) -> Self {
        Self {
            flow: Arc::new(app.discover_servers()),
            runtime,
            listeners,
            attempts: Attempts::default(),
        }
    }
}

#[uniffi::export]
impl DiscoveryFlow {
    /// Subscribes to the state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_state(&self, listener: Box<dyn DiscoveryStateListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.flow.state(), move |state| {
            listener.listener.on_value(DiscoveryState::from(&state));
        })
    }

    /// The state now.
    pub fn state(&self) -> DiscoveryState {
        DiscoveryState::from(&*self.flow.state().borrow())
    }

    /// Searches once: `Searching` lists servers as they answer, then `Done`. A few seconds. `interfaces` are the device's up IPv4
    /// interfaces, listed by the platform just before (ADR 0016); the request also goes out unbound, so an empty list still
    /// searches. Entries whose address is not IPv4 text are skipped.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first: the flow is back to `Idle`.
    pub async fn search(
        &self,
        interfaces: Vec<LocalInterface>,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        let interfaces: Vec<current_model::LocalInterface> = interfaces
            .iter()
            .filter_map(LocalInterface::parse)
            .collect();
        attempt(
            &self.runtime,
            &self.attempts,
            token,
            async move { flow.search(&interfaces).await },
            || self.flow.reset(),
        )
        .await
    }

    /// Back to `Idle`, dropping a running search.
    pub fn reset(&self) {
        self.attempts.begin();
        self.flow.reset();
    }
}

/// Signing in to one server, with a password or Quick Connect (ADR 0015). Success saves the server and its user.
#[derive(uniffi::Object)]
pub struct SignInFlow {
    flow: Arc<current_app::SignIn>,
    runtime: Handle,
    listeners: LiveListeners,
    attempts: Attempts,
}

impl SignInFlow {
    /// Runs one sign-in attempt on the core's runtime. A cancelled `token` goes to the app flow's own cancel, which the attempt
    /// notices at its next step; the attempt is never dropped halfway. A sign-in the server already accepted is saved in full
    /// and wins over the cancel: the call then succeeds, signed in.
    async fn attempt<F>(&self, token: Arc<CancelToken>, work: F) -> Result<(), CoreError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let attempt = self.attempts.begin();
        let flow = self.flow.clone();
        let attempts = self.attempts.clone();
        // Detached: dropping this call's future does not abort the attempt.
        let task = self.runtime.spawn(async move {
            let mut work = std::pin::pin!(work);
            tokio::select! {
                // The work first, so that its first step (which starts the attempt in the app flow) runs before a cancel can.
                biased;
                () = &mut work => return false,
                () = token.cancelled() => {}
            }
            if attempts.is_latest(attempt) {
                flow.cancel();
            }
            work.await;
            true
        });
        let cancelled = task.await.map_err(|error| CoreError::Runtime {
            message: error.to_string(),
        })?;
        let signed_in = matches!(
            *self.flow.state().borrow(),
            current_app::SignInState::SignedIn { .. }
        );
        if cancelled && !signed_in {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn new(
        flow: current_app::SignIn,
        runtime: Handle,
        listeners: LiveListeners,
    ) -> Self {
        Self {
            flow: Arc::new(flow),
            runtime,
            listeners,
            attempts: Attempts::default(),
        }
    }
}

#[uniffi::export]
impl SignInFlow {
    /// Subscribes to the state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_state(&self, listener: Box<dyn SignInStateListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.flow.state(), move |state| {
            listener.listener.on_value(SignInState::from(&state));
        })
    }

    /// The state now.
    pub fn state(&self) -> SignInState {
        SignInState::from(&*self.flow.state().borrow())
    }

    /// Asks the server whether it offers Quick Connect; the answer shows in `Ready`. A failure leaves it unknown.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn check_quick_connect(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        cancel::run_cancellable(&self.runtime, token, async move {
            flow.check_quick_connect().await;
            Ok(())
        })
        .await
    }

    /// Signs in with a user name and password. The password is used for this request only and is kept nowhere.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first: the flow is back to `Ready`. If the server had already accepted the
    /// sign-in, it is saved and the call succeeds instead.
    pub async fn with_password(
        &self,
        username: String,
        password: String,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        self.attempt(token, async move {
            flow.with_password(&username, &password).await;
        })
        .await
    }

    /// Signs in with Quick Connect: the state shows the code, then the sign-in once another client approves it (or the code
    /// expires after 10 minutes).
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first: polling stops and the flow is back to `Ready`. If the server had
    /// already accepted the sign-in, it is saved and the call succeeds instead.
    pub async fn with_quick_connect(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        self.attempt(token, async move {
            flow.with_quick_connect().await;
        })
        .await
    }

    /// Stops a running attempt and goes back to `Ready`. A finished sign-in stays signed in.
    pub fn cancel(&self) {
        self.attempts.begin();
        self.flow.cancel();
    }
}

/// Where signing in to a saved server's gateway again stands (ADR 0017). The same rules as [`AddServerState`]'s gateway states:
/// `realm` only as "The site says: ...", no credential ever.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum GatewaySignInState {
    /// Asking the address what its gateway wants, or trying what was just given.
    Checking,
    Password {
        origin: String,
        realm: Option<String>,
        wrong: bool,
    },
    /// Open the sign-in window on `url`, titled with `origin`, and hand its cookies over after each navigation that lands on
    /// `origin`. `redirected` and `location_origin` as in [`AddServerState::GatewaySignIn`].
    WebSignIn {
        url: String,
        origin: String,
        redirected: bool,
        location_origin: Option<String>,
    },
    /// Extra headers (advanced).
    Refused {
        origin: String,
    },
    Unsupported {
        origin: String,
    },
    /// Signed in to the gateway again and saved: the session goes on. `remembered` is false when the keyring refused the
    /// credentials: they last for this run.
    Done {
        remembered: bool,
    },
    Failed {
        problem: GatewaySignInProblem,
    },
}

impl std::fmt::Debug for GatewaySignInState {
    /// Like a derived Debug, with the origin, URL and realm redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checking => f.write_str("Checking"),
            Self::Password { realm, wrong, .. } => f
                .debug_struct("Password")
                .field("origin", &REDACTED)
                .field("realm", &realm.as_ref().map(|_| REDACTED))
                .field("wrong", wrong)
                .finish(),
            Self::WebSignIn {
                redirected,
                location_origin,
                ..
            } => f
                .debug_struct("WebSignIn")
                .field("url", &REDACTED)
                .field("origin", &REDACTED)
                .field("redirected", redirected)
                .field(
                    "location_origin",
                    &location_origin.as_ref().map(|_| REDACTED),
                )
                .finish(),
            Self::Refused { .. } => f
                .debug_struct("Refused")
                .field("origin", &REDACTED)
                .finish(),
            Self::Unsupported { .. } => f
                .debug_struct("Unsupported")
                .field("origin", &REDACTED)
                .finish(),
            Self::Done { remembered } => f
                .debug_struct("Done")
                .field("remembered", remembered)
                .finish(),
            Self::Failed { problem } => f.debug_struct("Failed").field("problem", problem).finish(),
        }
    }
}

impl From<&current_app::GatewaySignInState> for GatewaySignInState {
    fn from(state: &current_app::GatewaySignInState) -> Self {
        use current_app::GatewaySignInState as App;
        match state {
            App::Checking => Self::Checking,
            App::Password {
                origin,
                realm,
                wrong,
            } => Self::Password {
                origin: origin.clone(),
                realm: realm.as_ref().map(|realm| realm.as_str().to_owned()),
                wrong: *wrong,
            },
            App::WebSignIn {
                url,
                origin,
                redirected,
                location,
            } => Self::WebSignIn {
                url: url.clone(),
                origin: origin.clone(),
                redirected: *redirected,
                location_origin: location
                    .as_ref()
                    .map(|location| self::origin(location.url())),
            },
            App::Refused { origin } => Self::Refused {
                origin: origin.clone(),
            },
            App::Unsupported { origin } => Self::Unsupported {
                origin: origin.clone(),
            },
            App::Done { remembered } => Self::Done {
                remembered: *remembered,
            },
            App::Failed { error } => Self::Failed {
                problem: error.into(),
            },
        }
    }
}

/// See [`AddServerStateListener`].
#[uniffi::export(callback_interface)]
pub trait GatewaySignInStateListener: Send + Sync {
    fn on_value(&self, value: GatewaySignInState);
}

/// Signing in to a saved server's gateway again, when its status says `gateway_sign_in_needed` (ADR 0017). Call
/// [`GatewaySignInFlow::start`] first. One per sign-in window or prompt; success saves the credentials for that address only.
#[derive(uniffi::Object)]
pub struct GatewaySignInFlow {
    flow: Arc<current_app::GatewaySignIn>,
    runtime: Handle,
    listeners: LiveListeners,
}

impl GatewaySignInFlow {
    pub(crate) fn new(
        flow: current_app::GatewaySignIn,
        runtime: Handle,
        listeners: LiveListeners,
    ) -> Self {
        Self {
            flow: Arc::new(flow),
            runtime,
            listeners,
        }
    }
}

#[uniffi::export]
impl GatewaySignInFlow {
    /// Subscribes to the state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_state(
        &self,
        listener: Box<dyn GatewaySignInStateListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.flow.state(), move |state| {
            listener.listener.on_value(GatewaySignInState::from(&state));
        })
    }

    /// The state now.
    pub fn state(&self) -> GatewaySignInState {
        GatewaySignInState::from(&*self.flow.state().borrow())
    }

    /// Asks the address what its gateway wants, with the credentials it has: a gateway state, or `Done` when it lets the server
    /// through already.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first: the check still ends and sets the state.
    pub async fn start(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        cancel::run_detached(&self.runtime, token, async move {
            flow.start().await;
            Ok(())
        })
        .await
    }

    /// The gateway's user name and password, in `Password`.
    ///
    /// # Errors
    /// [`CoreError::NotReady`] in any other state; [`CoreError::Gateway`] when they cannot be used; [`CoreError::Cancelled`] as
    /// [`GatewaySignInFlow::start`].
    pub async fn submit_password(
        &self,
        user: String,
        password: String,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_password(&user, &password)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// The sign-in window's cookies, in `WebSignIn`, after each navigation that lands on the state's `origin`.
    ///
    /// # Errors
    /// As [`GatewaySignInFlow::submit_password`].
    pub async fn submit_cookies(
        &self,
        cookies: Vec<GatewayCookie>,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        let cookies: Vec<CookieInput> = cookies.into_iter().map(Into::into).collect();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_cookies(&cookies)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// Extra headers (advanced), in `Password`, `WebSignIn` or `Refused`.
    ///
    /// # Errors
    /// As [`GatewaySignInFlow::submit_password`].
    pub async fn submit_headers(
        &self,
        headers: Vec<GatewayHeader>,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let flow = self.flow.clone();
        let headers: Vec<HeaderInput> = headers.into_iter().map(Into::into).collect();
        cancel::run_detached(&self.runtime, token, async move {
            flow.submit_headers(&headers)
                .await
                .map_err(|error| submit_error(&error))
        })
        .await
    }

    /// Stops the flow (the window or prompt closed): a running attempt's result is dropped.
    pub fn cancel(&self) {
        self.flow.cancel();
    }
}
