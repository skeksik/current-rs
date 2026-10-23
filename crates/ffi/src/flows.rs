//! The M1 flows as bridge objects: adding a server, finding servers on the local network and signing in (ADR 0012, ADR 0015,
//! ADR 0016). Each has a state the UI subscribes to (a value, ADR 0004) and actions. The states carry what a screen shows and
//! nothing more: no address, token, password or Quick Connect secret ever crosses the bridge in a state. The password goes in only
//! as an argument of [`SignInFlow::with_password`].
//!
//! Cancelling an action's token returns its flow to where it was before the action (`Idle`, or `Ready` for a sign-in), unless a
//! newer action on the same flow started meanwhile: that one is left alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use current_app::{App, NewServer};
use tokio::runtime::Handle;

use crate::problems::{DiscoveryProblem, ProbeProblem, SignInProblem};
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
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
        }
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

/// Receives a flow's state: the current value first, then every change, on a core worker thread. It must only hand the value to
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

    /// Back to `Idle`, dropping a running probe.
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
