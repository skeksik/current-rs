//! The UniFFI surface over `current-app`: what the C# and Swift app shells call (ADR 0002, ADR 0004).
//!
//! - One runtime, owned here and started by [`open_core`] or [`Core::new`].
//! - The M1 flows (adding a server, discovery, sign-in) are objects with a state subscription and actions (`flows`); the saved
//!   servers are a list of diffs (`saved`).
//! - Observable values: `subscribe_x(listener) -> TaskHandle`. The listener gets the current value first, then every change, on a
//!   core thread (the runtime's blocking pool, never a worker or the UI thread). Lists use the same shape with batches of diffs (`diff::list_diff!`).
//! - Cancellable calls are `async` and take a [`CancelToken`]; they end with [`CoreError::Cancelled`].
//! - Listener callbacks return nothing and must never throw: each UI's listener catches everything.

mod browse;
mod cancel;
mod diff;
mod error;
mod flows;
mod player;
mod player_records;
mod problems;
mod records;
mod runtime;
mod saved;
mod subscribe;
mod task_handle;

use std::path::Path;
use std::sync::Arc;

use current_app::{App, ObservableList, Removal};
use current_store::{KeyringSecrets, MemorySecrets, Secrets};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

pub use browse::{
    Artwork, AspectRatio, Browse, BrowseNotice, BrowseNoticeListener, BrowseProblem, CalendarDate,
    Chapter, DetailsView, EpisodeLinks, HomeFeed, HomeRow, HomeRowDiff, HomeRowKind,
    HomeRowsListener, HomeState, HomeStateListener, ImageProblem, ImageRef, ImageRequest,
    ImageSource, ItemAction, ItemDetails, ItemDetailsListener, ItemQuery, ItemRef, ItemSummary,
    ItemSummaryDiff, ItemsListener, Library, LoadState, LoadStateListener, MediaSummary,
    NoticeKind, PagedItems, PagedState, PagedStateListener, Person, SearchGroup, SearchGroupDiff,
    SearchGroupsListener, SearchResults, Slot, SlotDiff, SlotsListener, SortOrder, StreamSummary,
    UserState,
};
pub use cancel::CancelToken;
pub use error::CoreError;
pub use flows::{
    AddServerFlow, AddServerState, AddServerStateListener, DiscoveredServerInfo, DiscoveryFlow,
    DiscoveryState, DiscoveryStateListener, GatewayCookie, GatewayHeader, GatewaySignInFlow,
    GatewaySignInState, GatewaySignInStateListener, LocalInterface, NewServerInfo, SignInFlow,
    SignInState, SignInStateListener,
};
pub use player::{
    NowPlayingListener, Player, PlayerStateListener, SwapChainHandle, SwapChainListener,
    player_engine,
};
pub use player_records::{
    CompositionSize, DisplayFacts, EngineOutputInfo, MediaStatusKind, NextEpisodeInfo,
    NowPlayingSnapshot, PlanNoteInfo, PlaybackInfoSnapshot, PlaybackProblem,
    PlaybackSettingsSnapshot, PlayerEngineKind, PlayerProblem, PlayerStateKind,
    PlayerStateSnapshot, QualityChoice, RouteInfo, RouteKind, SegmentInfo, SourceInfo, StreamInfo,
    StreamProtocolKind, SubtitleSelection, TrackInfo, TranscodingInfo, quality_steps,
};
pub use problems::{
    AddressProblem, DiscoveryProblem, GatewayInputProblem, GatewayProblem, GatewaySignInProblem,
    NetProblem, ProbeProblem, SignInProblem, StoreProblem,
};
pub use records::{
    AppInfo, AppTheme, CapabilitiesSnapshot, Connectivity, InterfaceKind, LoadReport, NetworkFacts,
    Reachability, RemoveServerResult, SavedServerSnapshot, SavedUserSnapshot, SecretStore,
    ServerRef, ServerStatusSnapshot, SignOutResult, UserSnapshot,
};
pub use runtime::WORKER_THREAD_NAME;
pub use saved::SavedServerDiff;
pub use task_handle::TaskHandle;

use subscribe::LiveListeners;

uniffi::setup_scaffolding!();

/// Receives the capabilities state. Called with the current value first, then on every change, on a core thread. It must only
/// hand the value to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait CapabilitiesListener: Send + Sync {
    fn on_value(&self, value: CapabilitiesSnapshot);
}

/// Receives the app theme. Called with the current value first, then on every change, on a core thread. It must only hand
/// the value to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait AppThemeListener: Send + Sync {
    fn on_value(&self, value: AppTheme);
}

/// Receives the saved servers: `[Reset]` first, then batches of diffs, on a core thread. Apply each batch in order. It must
/// only hand the batch to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait SavedServersListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<SavedServerDiff>);
}

/// Opens the core on the app's data and cache folders (ADR 0013): loads the settings, creating them and the device id on first
/// start, and the saved servers, and reads the image cache's index from the cache folder (ADR 0018). The file and keyring work runs on the core's own blocking pool, so the UI may await this from its UI thread. Then
/// call [`Core::restore_sessions`] in the background.
///
/// Open one core per data folder per process: two would each keep their own copy of the saved servers and overwrite each other's
/// settings. Dropping a core does not stop a restore check it started: that check runs until its requests end (their timeouts
/// bound it).
///
/// `device_name` is what Jellyfin shows for this device: the platform's name for the computer; an empty one becomes the platform's
/// name. `secrets` is [`SecretStore::Keyring`] in the apps.
///
/// # Errors
/// [`CoreError::Store`] if the data folder cannot be read or written; [`CoreError::Runtime`] if the runtime cannot start.
#[uniffi::export]
pub async fn open_core(
    data_dir: String,
    cache_dir: String,
    device_name: String,
    secrets: SecretStore,
) -> Result<Arc<Core>, CoreError> {
    let runtime = runtime::handle()?;
    let (app, report) = runtime
        .spawn_blocking(move || {
            let secrets: Arc<dyn Secrets> = match secrets {
                SecretStore::Keyring => Arc::new(KeyringSecrets::new()),
                SecretStore::Memory => Arc::new(MemorySecrets::new()),
            };
            App::open(
                Path::new(&data_dir),
                Path::new(&cache_dir),
                secrets,
                &device_name,
            )
        })
        .await
        .map_err(|error| CoreError::Runtime {
            message: error.to_string(),
        })?
        .map_err(|error| CoreError::Store {
            problem: (&error).into(),
        })?;
    Ok(Arc::new(Core::with_app(app, Some(report.into()), runtime)))
}

/// The core, created once per process by the app shell.
#[derive(uniffi::Object)]
pub struct Core {
    app: Arc<App>,
    runtime: Handle,
    listeners: LiveListeners,
    load_report: Option<LoadReport>,
    servers: Arc<ObservableList<SavedServerSnapshot>>,
    /// Keeps `servers` in step with the app's saved servers; stops with the core.
    servers_pump: JoinHandle<()>,
}

impl Core {
    fn with_app(app: App, load_report: Option<LoadReport>, runtime: Handle) -> Self {
        let app = Arc::new(app);
        let servers = ObservableList::new();
        let servers_pump = saved::pump(&runtime, app.saved_servers(), servers.clone());
        Self {
            app,
            runtime,
            listeners: LiveListeners::default(),
            load_report,
            servers,
            servers_pump,
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.servers_pump.abort();
    }
}

#[uniffi::export]
impl Core {
    /// A core that stores nothing: no saved servers, sign-ins last for the run. For tests and the smoke run; the apps use
    /// [`open_core`]. Starts the core's runtime on first use.
    ///
    /// # Errors
    /// [`CoreError::Runtime`] if the runtime cannot start.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self::with_app(
            App::new(),
            None,
            runtime::handle()?,
        )))
    }

    pub fn app_info(&self) -> AppInfo {
        self.app.info().into()
    }

    /// What the settings store found when [`open_core`] opened it. `None` for a core that stores nothing.
    pub fn load_report(&self) -> Option<LoadReport> {
        self.load_report.clone()
    }

    /// Subscribes to the capabilities state. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_capabilities(
        &self,
        listener: Box<dyn CapabilitiesListener>,
    ) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(
            &self.runtime,
            self.app.capabilities(),
            move |capabilities| {
                listener
                    .listener
                    .on_value(CapabilitiesSnapshot::from(&capabilities));
            },
        )
    }

    /// The theme the app shows now: the saved choice, `System` until one is made. Apply it before the window first shows.
    pub fn app_theme(&self) -> AppTheme {
        (*self.app.app_theme().borrow()).into()
    }

    /// Subscribes to the app theme. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_app_theme(&self, listener: Box<dyn AppThemeListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.app.app_theme(), move |theme| {
            listener.listener.on_value(theme.into());
        })
    }

    /// Changes the app theme and saves it at once, on the core's blocking pool; subscribers see it once it is saved. A core that
    /// stores nothing ([`Core::new`]) only publishes it. Not cancellable: it is one small write.
    ///
    /// # Errors
    /// [`CoreError::Store`] if the settings could not be saved (a newer version's file, a failed write): the theme stays as it was.
    pub async fn set_app_theme(&self, theme: AppTheme) -> Result<(), CoreError> {
        let app = self.app.clone();
        self.runtime
            .spawn_blocking(move || app.set_app_theme(theme.into()))
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| CoreError::Store {
                problem: (&error).into(),
            })
    }

    /// The saved servers now, in the order they were added: read from the app, so a change shows here as soon as the call that made
    /// it returns. A subscription's mirror follows a moment later.
    pub fn servers(&self) -> Vec<SavedServerSnapshot> {
        self.app.servers().iter().map(Into::into).collect()
    }

    /// Subscribes to the saved servers. Keep the handle while listening; cancel it when the view goes away.
    pub fn subscribe_servers(&self, listener: Box<dyn SavedServersListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_list(&self.runtime, self.servers.subscribe(), move |batch| {
            listener
                .listener
                .on_diffs(batch.into_iter().map(SavedServerDiff::from_core).collect());
        })
    }

    /// A new "add a server" flow.
    pub fn add_server(&self) -> Arc<AddServerFlow> {
        Arc::new(AddServerFlow::new(
            self.app.clone(),
            self.runtime.clone(),
            self.listeners.clone(),
        ))
    }

    /// A new search for servers on the local network.
    pub fn discover_servers(&self) -> Arc<DiscoveryFlow> {
        Arc::new(DiscoveryFlow::new(
            &self.app,
            self.runtime.clone(),
            self.listeners.clone(),
        ))
    }

    /// A sign-in to a saved server, for "sign in again" after a session ended. Each attempt first checks that the server's address
    /// still answers as that server.
    ///
    /// # Errors
    /// [`CoreError::UnknownServer`] if no such server is saved; [`CoreError::NoAddress`] if it has no address left;
    /// [`CoreError::InvalidAddress`] if an address cannot take requests.
    pub fn sign_in_again(&self, server: ServerRef) -> Result<Arc<SignInFlow>, CoreError> {
        let server: current_model::ServerRef = (&server).into();
        let Some(saved) = self
            .app
            .servers()
            .into_iter()
            .find(|saved| saved.server == server)
        else {
            return Err(CoreError::UnknownServer);
        };
        if saved.addresses.is_empty() {
            return Err(CoreError::NoAddress);
        }
        let flow = self
            .app
            .sign_in_again(&server)
            // Removed since the check above.
            .ok_or(CoreError::UnknownServer)?
            .map_err(|_| CoreError::InvalidAddress)?;
        Ok(Arc::new(SignInFlow::new(
            flow,
            self.runtime.clone(),
            self.listeners.clone(),
        )))
    }

    /// A sign-in to a saved server's gateway again (ADR 0017), when its status says `gateway_sign_in_needed`: for the address
    /// whose gateway asked. Call [`GatewaySignInFlow::start`] first. The Jellyfin session and its token were kept and go on once
    /// it is done.
    ///
    /// # Errors
    /// [`CoreError::UnknownServer`] if no such server is saved; [`CoreError::NoAddress`] if it has no usable address.
    pub fn sign_in_to_gateway(
        &self,
        server: ServerRef,
    ) -> Result<Arc<GatewaySignInFlow>, CoreError> {
        let server: current_model::ServerRef = (&server).into();
        if !self
            .app
            .servers()
            .iter()
            .any(|saved| saved.server == server)
        {
            return Err(CoreError::UnknownServer);
        }
        let flow = self
            .app
            .sign_in_to_gateway(&server)
            .ok_or(CoreError::NoAddress)?;
        Ok(Arc::new(GatewaySignInFlow::new(
            flow,
            self.runtime.clone(),
            self.listeners.clone(),
        )))
    }

    /// Checks every saved session, all servers at once: run it in the background after [`open_core`], and again when the network
    /// returns. The capabilities state shows the outcome per server.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first. Cancelling stops only the waiting: each server's check runs to its
    /// end, so none is left halfway.
    pub async fn restore_sessions(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        let app = self.app.clone();
        cancel::run_cancellable(&self.runtime, token, async move {
            app.restore_sessions().await;
            Ok(())
        })
        .await
    }

    /// Signs out of a server: this device forgets the session and its token at once, then the server is told (at most 10 s). Not
    /// cancellable: half a sign-out would leave the token behind.
    ///
    /// # Errors
    /// [`CoreError::UnknownServer`] if no such server is saved.
    pub async fn sign_out(&self, server: ServerRef) -> Result<SignOutResult, CoreError> {
        let server: current_model::ServerRef = (&server).into();
        if !self
            .app
            .servers()
            .iter()
            .any(|saved| saved.server == server)
        {
            return Err(CoreError::UnknownServer);
        }
        let app = self.app.clone();
        self.runtime
            .spawn(async move { app.sign_out(&server).await })
            .await
            .map(|told| SignOutResult {
                server_told: told.is_ok(),
            })
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })
    }

    /// Removes a saved server (ADR 0015): this device signs out every user there, deletes their tokens and forgets the server and
    /// its addresses at once; the server is told in the background. Removing a server that is not saved does nothing (`removed`
    /// false). Not cancellable, as sign-out.
    ///
    /// # Errors
    /// [`CoreError::Store`] if the settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub async fn remove_server(&self, server: ServerRef) -> Result<RemoveServerResult, CoreError> {
        let server: current_model::ServerRef = (&server).into();
        let app = self.app.clone();
        let removal = self
            .runtime
            .spawn(async move { app.remove_server(&server).await })
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| CoreError::Store {
                problem: (&error).into(),
            })?;
        Ok(match removal {
            Removal::NotSaved => RemoveServerResult {
                removed: false,
                credentials_left: false,
            },
            Removal::Removed { credentials_left } => RemoveServerResult {
                removed: true,
                credentials_left,
            },
        })
    }

    /// The platform's report of the network, on start and on every change (ADR 0014). Call it from one thread.
    pub fn network_changed(&self, facts: NetworkFacts) {
        self.app.network_changed(facts.into());
    }

    /// The app came back to the foreground: unreachable servers try again at once.
    pub fn returned_to_foreground(&self) {
        self.app.returned_to_foreground();
    }

    /// The request log as text, oldest first, redacted (ADR 0006).
    pub fn diagnostics_text(&self) -> String {
        self.app.diagnostics_text()
    }

    /// What "copy diagnostics" puts on the clipboard: header lines (the core's version, the platform and architecture, what the
    /// settings store found), then the redacted request log. Nothing else: no path, no device name.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] if `token` is cancelled first.
    pub async fn export_diagnostics(&self, token: Arc<CancelToken>) -> Result<String, CoreError> {
        let version = self.app.info().version;
        let settings = self
            .load_report
            .as_ref()
            .map_or("not stored", LoadReport::kind);
        let log = self.app.diagnostics_text();
        cancel::run_cancellable(&self.runtime, token, async move {
            Ok(format!(
                "Current {version}\nPlatform: {} {}\nSettings: {settings}\n\nRequests, oldest first:\n{log}",
                std::env::consts::OS,
                std::env::consts::ARCH,
            ))
        })
        .await
    }

    /// How many listeners the core still holds. A diagnostic for leak checks: it must fall when a view cancels its handles.
    pub fn live_listeners(&self) -> u64 {
        self.listeners.count()
    }
}

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod gateway_tests;
#[cfg(test)]
mod player_tests;
#[cfg(test)]
mod tests;
