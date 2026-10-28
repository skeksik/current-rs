//! The core's composition root and the state objects the app shells render (ADR 0002, ADR 0004).
//!
//! [`App`] is created once per process. It owns what every feature shares: the request log, the capabilities state and, when opened
//! on a data folder, the settings, the secrets and the device id (ADR 0013). Server setup ([`AddServer`]) finds a server; signing in
//! ([`SignIn`]) saves it with its user, and the sessions are restored at the next start (ADR 0015). The crate holds no async runtime:
//! `current-ffi` and the Linux app bring their own, and its async methods expect tokio's.

mod add_server;
mod browse;
mod discovery;
mod formats;
mod gateway;
mod gateway_sign_in;
mod images;
mod library_choices;
mod list;
pub mod navigation;
mod numbers;
mod player;
mod problems;
mod session;
mod sign_in;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use current_jellyfin::{
    AuthError, ClientIdentity, JellyfinClient, JellyfinIdentity, platform_name,
};
use current_model::{
    AppTheme, Capabilities, ItemQuery, ItemRef, Library, MediaKind, NetworkFacts, SavedServer,
    ServerAddress, ServerId, ServerRef,
};
use current_net::{
    Gateway, NetError, RequestLog, RoamingAddresses, RoamingConfig, RoamingError, RuntimeHandle,
    Url,
};
use current_store::{
    DeviceId, KindPage, LibraryChoices, LoadReport, Secrets, SettingsStore, StoreError,
};
use tokio::sync::watch;

pub use add_server::{AddServer, AddServerState, NewServer};
pub use browse::{
    BACKLOG_PAGES, Browse, BrowseNotice, DEPENDENT_RELOAD_DELAY, DetailsView, FAVOURITE_ROW_KINDS,
    FAVOURITE_ROW_LIMIT, FILTERS_OFF_WHILE_SEARCHING, GridTools, HomeFeed, HomeRow, HomeRowKind,
    HomeState, ItemAction, LATEST_LIMIT, LIBRARY_SEARCH_WINDOW, LIVE_PAUSE_AFTER, LoadState,
    MAX_PAGE_REQUESTS, NEXT_UP_LIMIT, NoticeKind, PAGE_SIZE, PagedItems, PagedState,
    REFRESH_WHEN_SHOWN_AFTER, RESUME_LIMIT, SIMILAR_LIMIT, SORT_OFF_WHILE_SEARCHING, SearchResults,
    SeriesRun, Slot, SortChoice, TileShape, favourites_query, grid_tools, kind_query,
    library_filters, library_query, library_search, library_search_note, library_sorts,
    library_toggle_filters, recently_added_everywhere_query, recently_added_query,
    seen_recently_query, series_run, tile_artwork, view_kinds, with_filter_choice,
    with_filter_toggle,
};
pub use browse::{
    BrowseView, CATALOG_NEEDS_SEERR, CATALOG_NEEDS_SEERR_LABEL, FAVOURITES_EMPTY,
    FAVOURITES_FAILED, FilterMenu, GridOpening, ItemOpens, KIND_TABS, NARROWED_SEARCH_LIMIT,
    RowDisplay, RowLink, RowNote, RowTarget, SearchChoice, SearchFilter, SearchSource,
    SearchSourceChoice, SearchTools, SearchTypeChoice, ViewLayout, catalog_available,
    favourites_status, filter_menu, grid_opening, home_row_display, kind_choice_name, kind_group,
    kind_tabs, library_filter_name, library_sort_name, person_query, runtime_label,
    search_filter_in_effect, search_group_display, search_note, search_result_display,
    search_tools, search_type_label, search_types, time_left_label, time_left_short, view_layout,
    view_link_tooltip, view_opening, view_query, view_remembers, view_title,
};
pub use browse::{
    DetailsFacts, FAILED_TILE_NAME, LIBRARY_GONE_MESSAGE, LIBRARY_GONE_TITLE, LOADING_TILE_NAME,
    action_failed_title, details_facts, facts_line, item_count_label, person_kind_name,
    search_status, size_on_disk, tile_name, tile_subtitle, tile_title,
};
pub use browse::{HomeRowSetting, NavGroup, PaneEntry, PaneLayout, PaneLibrary};
pub use discovery::{DiscoveredServer, Discovery, DiscoveryState, PROBES_AT_ONCE};
pub use formats::{
    DecoderProfile, FormatRow, FormatSupport, FormatTab, FormatTabKind, HardwareDecoding,
    SupportedFormats,
};
pub use gateway::{CookieInput, GatewaySubmitError, HeaderInput};
pub use gateway_sign_in::{GatewaySignIn, GatewaySignInError, GatewaySignInState};
pub use images::{ImageError, ImageLoad, ImageSource, WIDTH_BUCKETS, bucket};
pub use library_choices::{LibraryMemory, SearchMemory};
pub use list::{ListSubscription, ObservableList, VectorDiff};
pub use numbers::NumberFormat;
#[cfg(any(test, feature = "test-engine"))]
pub use player::test_engine;
pub use player::{
    CompositionSize, DisplayFacts, EngineOutput, EngineReport, MediaStatus, NowPlaying,
    PlaybackInfo, Player, PlayerAction, PlayerEngine, PlayerError, QualityEntry, QualityMenuItem,
    SwapChain, TIMELINE_INTERVAL, VideoOutput, bitrate_label, fallback_quality_entries,
    player_engine, quality_label, quality_menu,
};
// The player's words and its problems (ADR 0019), worded once for every UI.
pub use player::{
    NO_AUDIO_TRACKS, PanelSizes, PlaybackProblem, PlayerProblem, SUBTITLES_OFF, countdown_text,
    elapsed_label, episode_line, info_panel_lines, plan_note_text, playback_problem_text,
    player_problem_text, position_label, reason_text, remaining_label, route_name, skip_label,
    spoken_time, time_label, track_name,
};
/// What the server would convert the version `details` plays to, for naming its quality menu's steps ([`App::quality_options`]): the
/// audio track playing, and the video codec the server reports it encodes to. Only a transcode's codec says what the server picks; a
/// copied video's is the source's, so H.264, the server's default, stands then.
fn quality_target<'a>(
    details: Option<&'a playback::PlaybackDetails>,
    audio_codecs: &'a [current_model::Codec],
) -> playback::QualityTarget<'a> {
    playback::QualityTarget {
        audio: details
            .and_then(|details| details.plan.as_ref())
            .and_then(|plan| plan.tracks.audio),
        video_codec: details
            .and_then(|details| details.transcoding.as_ref())
            .filter(|facts| !facts.video_direct)
            .and_then(|facts| facts.video_codec.as_ref()),
        audio_codecs,
    }
}

/// `current-playback`'s values that the player publishes.
pub mod playback {
    pub use current_playback::{
        AutoSkip, NextEpisode, PlanNote, PlaybackDetails, PlayerStatus, Quality, QualityOption,
        QualityPreference, QualityTarget, Track, preferred_quality_options, quality_options,
        snap_quality,
    };
}
pub use problems::{
    AddressProblem, BrowseProblem, DiscoveryProblem, EMBY_HEADER_IGNORED, GATEWAY_REFUSED,
    GATEWAY_UNSUPPORTED, GATEWAY_WEB_PAGE, GOOGLE_REFUSES_WEB_VIEWS, GatewayInputProblem,
    GatewayProblem, GatewaySignInProblem, NetProblem, Platform, ProbeProblem, SignInProblem,
    StoreProblem, UNEXPECTED, browse_problem_text, discovery_problem_text,
    gateway_input_problem_text, gateway_problem_text, gateway_sign_in_problem_text,
    net_problem_text, probe_problem_text, sign_in_problem_text, store_problem_text,
};
pub use sign_in::{SignIn, SignInError, SignInState};

use images::ImageCache;
use session::{Roaming, Shared, Storage, Target};

/// What the about and diagnostics screens show about the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// The core's version, from the workspace manifest.
    pub version: String,
}

/// How removing a saved server went ([`App::remove_server`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// No such server was saved (unknown, or removed already): nothing changed.
    NotSaved,
    /// The server is gone from the list, the settings file and the capabilities state, and signed out on this device.
    /// `credentials_left` is true when a saved sign-in could not be deleted from the keyring: the UI says so, since it stays in
    /// the OS's credential store until deleted there (its key is never given to another server).
    Removed { credentials_left: bool },
}

/// The core: one per process.
pub struct App {
    shared: Arc<Shared>,
    device_id: Option<DeviceId>,
    /// The app theme, published after each successful save.
    app_theme: watch::Sender<AppTheme>,
    /// Serializes [`App::set_app_theme`], so the published theme is the one saved last.
    theme_write: Mutex<()>,
    images: ImageCache,
    browse: Arc<browse::Hub>,
    playback: Arc<player::PlaybackPrefs>,
    /// Settings' "Remember sort", "Remember filter" and "Remember view", and the choices remembered per library.
    libraries: library_choices::LibraryPrefs,
    /// The loopback relay for streams on addresses with a gateway (ADR 0020): one per app.
    relay: Arc<player::RelaySlot>,
    /// What the engine plays, kept from its last start or read once from a headless one (ADR 0021).
    engine_capabilities: Arc<player::KnownCapabilities>,
    /// How the UI's platform writes numbers, for the labels with decimals ([`App::set_decimal_separator`]).
    number_format: Mutex<NumberFormat>,
    /// The UI's platform, for the texts that name it ([`App::set_platform`]).
    platform: Mutex<Platform>,
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
                0,
            )),
            device_id: None,
            app_theme: watch::Sender::new(AppTheme::System),
            theme_write: Mutex::new(()),
            images: ImageCache::in_memory(),
            browse: Arc::default(),
            playback: Arc::new(player::PlaybackPrefs::load(None)),
            libraries: library_choices::LibraryPrefs::new(),
            relay: Arc::default(),
            engine_capabilities: Arc::default(),
            number_format: Mutex::default(),
            platform: Mutex::default(),
        }
    }

    /// A core on the data and cache folders the UI passes in (ADR 0013): loads the settings, creating them and the device id on first
    /// start, and the saved servers, each signed in if it has an active user until [`App::restore_sessions`] checks. The image
    /// cache's index is read from `cache_dir`, and the images of servers no longer saved or signed out are deleted there (ADR
    /// 0018); a cache folder that cannot be used leaves images in memory only. `secrets` is the OS
    /// keyring in the apps and memory in tests. `device_name` is what Jellyfin shows for this device: the UI passes the platform's
    /// name for the computer (ADR 0015); an empty one becomes the platform's name. Blocking file I/O: call it at start, off the
    /// async runtime. One `App` per data folder per process is assumed: two would each keep their own copy of the saved servers
    /// and overwrite each other's settings.
    pub fn open(
        data_dir: &Path,
        cache_dir: &Path,
        secrets: Arc<dyn Secrets>,
        device_name: &str,
    ) -> Result<(Self, LoadReport), StoreError> {
        let (settings, report) = SettingsStore::open(data_dir)?;
        let stored = settings.get();
        let identity = ClientIdentity::current(device_name, stored.device_id.as_str());
        let images = ImageCache::open(cache_dir, &stored.servers);
        let playback = Arc::new(player::PlaybackPrefs::load(Some(&settings)));
        let app = Self {
            shared: Arc::new(Shared::new(
                Arc::new(RequestLog::default()),
                Some(Storage { settings, secrets }),
                identity,
                stored.servers,
                stored.last_server_key,
            )),
            device_id: Some(stored.device_id),
            app_theme: watch::Sender::new(stored.app_theme),
            theme_write: Mutex::new(()),
            images,
            browse: Arc::default(),
            playback,
            libraries: library_choices::LibraryPrefs::new(),
            relay: Arc::default(),
            engine_capabilities: Arc::default(),
            number_format: Mutex::default(),
            platform: Mutex::default(),
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

    /// The theme the app shows: the saved choice now, and every change after. An unchanged theme notifies no one.
    pub fn app_theme(&self) -> watch::Receiver<AppTheme> {
        self.app_theme.subscribe()
    }

    /// Changes the app theme and saves it at once (an atomic save, ADR 0013); subscribers see it once it is saved. A core that
    /// stores nothing ([`App::new`], as in the smoke run) only publishes it. Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved (a newer version's file, a failed write): the theme stays as it was.
    pub fn set_app_theme(&self, theme: AppTheme) -> Result<(), StoreError> {
        // Held only for the save and the publish; a panic in between leaves nothing half-changed.
        let _order = self
            .theme_write
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(settings) = self.settings() {
            settings.update(|settings| settings.app_theme = theme)?;
        }
        self.app_theme.send_if_modified(|current| {
            let changed = *current != theme;
            *current = theme;
            changed
        });
        Ok(())
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

    /// The signed-in client of a server, for the features that browse and play. A restored session has a client even while the
    /// server cannot be reached (the capabilities state says `Unreachable`): its requests go out once the server's address
    /// provider reaches it again, and fail fast meanwhile. `None` when the server is signed out.
    pub fn jellyfin(&self, server: &ServerRef) -> Option<Arc<JellyfinClient>> {
        self.shared.session(server)
    }

    /// A new "add a server" flow. Its probes go to the shared request log.
    pub fn add_server(&self) -> AddServer {
        AddServer::new(self.shared.request_log.clone())
    }

    /// A sign-in to the server [`AddServer`] found (its `Ready` state). Success saves the server, or adds the address and user to
    /// the saved server with the same id, with the address's gateway credentials (ADR 0017) and the header Jellyfin's credentials
    /// went in. Its requests are logged as `jellyfin/setup`.
    pub fn sign_in(&self, server: NewServer) -> Result<SignIn, NetError> {
        let setup = ServerRef::new(
            current_model::ServiceType::JELLYFIN,
            current_model::ServerKey::new("setup"),
        );
        // The gateway credentials setup was given for the address, if any: the sign-in goes through them (ADR 0017).
        let gateway = (!server.gateway.is_empty()).then(|| Arc::new(Gateway::new(server.gateway)));
        let client = self
            .shared
            .client(setup, &server.address, gateway.clone())?;
        let expected = server.info.id.clone();
        let target = Target::New {
            id: server.info.id,
            name: server.info.name,
            address: server.address,
            version: server.info.version,
            gateway,
        };
        Ok(SignIn::new(self.shared.clone(), target, client, expected))
    }

    /// A sign-in to a saved server, over all its addresses through the server's address provider (ADR 0014). Each attempt first
    /// waits, in `SigningIn`, until the provider has selected an address that passed the identity check (the saved server's id),
    /// so no credential goes anywhere else. `None` if no such server is saved or it has no address; `InvalidRequest` if an
    /// address is not an http(s) URL.
    pub fn sign_in_again(&self, server: &ServerRef) -> Option<Result<SignIn, NetError>> {
        let saved = self.shared.saved(server)?;
        if saved.addresses.is_empty() {
            return None;
        }
        if saved
            .addresses
            .iter()
            .any(|address| Url::parse(&address.url).is_err())
        {
            return Some(Err(NetError::InvalidRequest));
        }
        Some(Ok(SignIn::saved(self.shared.clone(), saved)))
    }

    /// A sign-in to a saved server's gateway again (ADR 0017), when its status says `gateway_sign_in_needed`: for the address
    /// whose gateway asked (else the address in use, else the first with gateway credentials). Call [`GatewaySignIn::start`]
    /// first. Success saves the credentials for that address only; the Jellyfin session and its token were kept and go on. `None`
    /// if no such server is saved or its address is not an http(s) URL.
    pub fn sign_in_to_gateway(&self, server: &ServerRef) -> Option<GatewaySignIn> {
        let saved = self.shared.saved(server)?;
        let address = self.shared.gateway_address(server)?;
        Some(GatewaySignIn::new(
            self.shared.clone(),
            server.clone(),
            saved.id,
            address,
        ))
    }

    /// Checks every saved session, all servers at once; the shell runs it in the background after [`App::open`]. Each saved Jellyfin
    /// server gets its address provider here (it needs the runtime), and from then on the capabilities state follows the provider:
    /// reachability, the position of the address in use, and the version its identity check read. For each server with an active
    /// user: the token from the keyring, then the session over the provider, then the current user. The token goes only to an
    /// address that passed the identity check (ADR 0014). Signed in with the user when the server answers, signed out when the
    /// token is gone or the server answers 401, still signed in and unreachable when no address answers as the server; such a
    /// session checks its user once the provider reaches the server. Running it again checks sessions whose user is not yet
    /// confirmed. Each server's check runs to its end even if this future is dropped: dropping it only stops the waiting. Such a
    /// check holds the app's shared state, so it can outlive the `App` until its requests end (their timeouts bound it); it changes
    /// only that state and the settings file, never another `App`'s.
    pub async fn restore_sessions(&self) {
        self.shared.restore_all().await;
    }

    /// Signs out of a server: tells the server, then deletes the token and the active user whatever it answered. Returns the
    /// server's answer, for the UI to mention that the server could not be told; the app is signed out either way. The server's
    /// cached images are deleted then (ADR 0018); if this future is dropped before, the next start deletes them.
    pub async fn sign_out(&self, server: &ServerRef) -> Result<(), AuthError> {
        let result = self.shared.sign_out(server).await;
        self.images.forget_server(server).await;
        result
    }

    /// Removes a saved server (ADR 0015, "Remove a server"), local first, as sign-out: under the session lock the list without it
    /// is saved and its session, address provider and status are dropped; then every saved user's token for it is deleted from
    /// the keyring. Only then, in the background and without waiting, is the server told (`POST /Sessions/Logout` for each token,
    /// through its identity-checked provider); its answer changes nothing. A restore or sign-in to it that ends later finds it gone
    /// and saves nothing. Removing a server that is not saved does nothing. Its cached images are deleted after (ADR 0018); if this
    /// future is dropped before, the next start deletes them.
    ///
    /// # Errors
    /// The settings could not be saved (a newer version's file, a failed write): nothing changed, the server is still saved and
    /// signed in as before.
    pub async fn remove_server(&self, server: &ServerRef) -> Result<Removal, StoreError> {
        let removal = self.shared.remove_server(server).await?;
        self.libraries.server_removed(self.settings(), server);
        self.images.forget_server(server).await;
        Ok(removal)
    }

    /// An image's encoded bytes (JPEG or PNG from the server, BMP for a BlurHash placeholder), for a box of `load.width` by
    /// `load.height` physical pixels (ADR 0018, section 4). The width is rounded up to a bucket ([`bucket`]) and the height follows.
    /// From memory, the disk cache, or the server's signed-in session, through its address provider and gateway credentials.
    /// Identical requests share one fetch. Dropping the future cancels the request: the fetch stops once no caller waits for it.
    ///
    /// # Errors
    /// [`ImageError`]: not signed in, the server's or the network's failure, an answer that is not an image, an invalid BlurHash
    /// or an empty box.
    pub async fn load_image(&self, load: &ImageLoad) -> Result<Arc<[u8]>, ImageError> {
        self.images.load(&self.shared, load).await
    }

    /// The image cache's size on disk, in bytes, every server's images together (Settings → Storage). 0 for an app that stores
    /// nothing.
    pub async fn image_cache_size(&self) -> u64 {
        self.images.disk_size().await
    }

    /// Deletes every cached image of every server, in memory and on disk (Settings → Storage, "Clear"); they load again from the
    /// servers when next shown. Safe while images load: the cache's index stays in step with its folder.
    pub async fn clear_image_cache(&self) {
        self.images.clear().await;
    }

    /// Opens a player for a window (ADR 0019) with the panel's size in physical pixels (needed before the engine starts, ADR 0007) and
    /// the display's facts. One engine per process for now: while another player is open this waits for it to close (a few seconds),
    /// then fails with [`PlayerError::Busy`]. Call it on the tokio runtime.
    ///
    /// # Errors
    /// [`PlayerError::EngineUnavailable`] in a build without an engine, [`PlayerError::Busy`], [`PlayerError::EngineFailed`],
    /// [`PlayerError::NoRuntime`].
    pub async fn open_player(
        &self,
        size: CompositionSize,
        display: DisplayFacts,
    ) -> Result<Player, PlayerError> {
        let context = player::Context {
            shared: self.shared.clone(),
            images: self.images.clone(),
            browse: self.browse.clone(),
            prefs: self.playback.clone(),
            relay: self.relay.clone(),
            capabilities: self.engine_capabilities.clone(),
        };
        player::open(context, size, display).await
    }

    /// What this build's engine plays, without playing anything (ADR 0021): the capabilities the last engine reported at its start,
    /// else those of a headless engine started now only to read them (no window, no audio device). Call it on the tokio runtime.
    pub async fn engine_capabilities(&self) -> EngineReport {
        self.engine_capabilities.get().await
    }

    /// Settings' "Supported formats" page (ADR 0021) from the engine's capabilities ([`App::engine_capabilities`]), the graphics
    /// card's decoder profiles (`None` when they could not be read) and the facts of the display the window is on (`None` when
    /// unknown). Call it on the tokio runtime.
    pub async fn supported_formats(
        &self,
        hardware: Option<HardwareDecoding>,
        display: Option<DisplayFacts>,
    ) -> SupportedFormats {
        let engine = self.engine_capabilities().await;
        formats::supported_formats(&engine, hardware.as_ref(), display.as_ref())
    }

    /// The segment kinds skipped automatically (Settings → Playback, ADR 0019 choice 8), by their names (`intro`, `outro`, and kinds
    /// a later version adds); empty when off, which is the default. The current value first, then every change.
    pub fn auto_skip(&self) -> watch::Receiver<Vec<String>> {
        self.playback.auto_skip()
    }

    /// Sets the segment kinds skipped automatically and saves them; open players follow at once. Blocking file I/O: call it off the
    /// async runtime.
    ///
    /// # Errors
    /// The settings could not be saved; the setting stands for this run.
    pub fn set_auto_skip(&self, kinds: Vec<String>) -> Result<(), StoreError> {
        self.playback.set_auto_skip(&self.shared, kinds)
    }

    /// Settings → Playback's "Preferred quality": the most each item plays at, on local and internet addresses alike; "Maximum" by
    /// default. Each item plays at the highest entry of its quality menu that is not above it (`current_playback::snap_quality`); a
    /// quality picked in the player changes only the item playing (owner, 2026-10-27).
    pub fn preferred_quality(&self) -> playback::Quality {
        self.playback.preferred_quality()
    }

    /// Sets the preferred quality and saves it; it applies from the next item played. Blocking file I/O: call it off the async
    /// runtime.
    ///
    /// # Errors
    /// The settings could not be saved; the setting stands for this run.
    pub fn set_preferred_quality(&self, preferred: playback::Quality) -> Result<(), StoreError> {
        self.playback.set_preferred_quality(&self.shared, preferred)
    }

    /// Settings → Playback's "Show resolution in quality menu": whether the quality menu names the resolution each choice gives
    /// (`current_playback::quality_options`); on by default.
    pub fn quality_resolution(&self) -> bool {
        self.playback.quality_resolution()
    }

    /// Turns "Show resolution in quality menu" on or off and saves it. Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved; the setting stands for this run.
    pub fn set_quality_resolution(&self, on: bool) -> Result<(), StoreError> {
        self.playback.set_quality_resolution(&self.shared, on)
    }

    /// The quality menu for the version `info` plays (`current_playback::quality_options`, `all` as there), each step named by the
    /// picture the server would send this file at (ADR 0019, choice 10, "Names"): with the audio track playing, the audio codecs the
    /// device profile lists for the engine, and the video codec the server reports it encodes to, else H.264, its default. So the
    /// names can change once playback has started. Before a version is planned, every step by its width cap.
    pub fn quality_options(&self, info: &PlaybackInfo, all: bool) -> Vec<playback::QualityOption> {
        let audio_codecs = self
            .engine_capabilities
            .known()
            .map(|capabilities| current_jellyfin::transcode_targets(&capabilities).audio)
            .unwrap_or_default();
        let details = info.details.as_ref();
        let source = details
            .and_then(|details| details.plan.as_ref())
            .map(|plan| &plan.source);
        playback::quality_options(source, &quality_target(details, &audio_codecs), all)
    }

    /// Settings → Playback's "Show all quality options": whether the quality menu lists every step below the original's bitrate, not
    /// only the common ones (`current_playback::quality_options`); off by default.
    pub fn quality_all(&self) -> bool {
        self.playback.quality_all()
    }

    /// Turns "Show all quality options" on or off and saves it. Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved; the setting stands for this run.
    pub fn set_quality_all(&self, all: bool) -> Result<(), StoreError> {
        self.playback.set_quality_all(&self.shared, all)
    }

    /// Settings' "Remember sort", "Remember filter" and "Remember view" now: all off by default.
    pub fn library_memory(&self) -> LibraryMemory {
        self.libraries.switches(self.settings())
    }

    /// Turns "Remember sort" on or off and saves it; off forgets every library's sort in the same save, so on again starts afresh.
    /// Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub fn set_remember_sort(&self, on: bool) -> Result<(), StoreError> {
        self.libraries
            .set(self.settings(), library_choices::Part::Sort, on)
    }

    /// Turns "Remember filter" on or off, as [`App::set_remember_sort`].
    ///
    /// # Errors
    /// As [`App::set_remember_sort`].
    pub fn set_remember_filter(&self, on: bool) -> Result<(), StoreError> {
        self.libraries
            .set(self.settings(), library_choices::Part::Filter, on)
    }

    /// Turns "Remember view" on or off, as [`App::set_remember_sort`].
    ///
    /// # Errors
    /// As [`App::set_remember_sort`].
    pub fn set_remember_view(&self, on: bool) -> Result<(), StoreError> {
        self.libraries
            .set(self.settings(), library_choices::Part::View, on)
    }

    /// What `library` opens with: the last sort, filter and view chosen there, each only while its switch is on; `None` for a part
    /// with nothing remembered, which opens with the library's first choice.
    pub fn library_choices(&self, library: &ItemRef) -> LibraryChoices {
        self.libraries.choices(self.settings(), library)
    }

    /// What `library`'s grid opens with ([`grid_opening`]): `home_query` as it is when a Home row's header opened it, else the
    /// library's first query with what [`App::library_choices`] remembers.
    pub fn library_opening(&self, library: &Library, home_query: Option<ItemQuery>) -> GridOpening {
        let remembered = match home_query {
            Some(_) => LibraryChoices::default(),
            None => self.library_choices(&library.item),
        };
        grid_opening(library, home_query, &remembered)
    }

    /// What `view`'s grid of `server` opens with ([`view_opening`]): a kind tab's page ([`view_remembers`]) opened from its tab with
    /// what [`App::view_choices`] remembers, as a library; opened from a Favourites row's header (`favourite`), or any other view,
    /// its own query, posters with titles.
    pub fn view_opening(
        &self,
        server: &ServerRef,
        view: BrowseView,
        kind: Option<&MediaKind>,
        favourite: bool,
    ) -> GridOpening {
        let remembered = if favourite {
            LibraryChoices::default()
        } else {
            self.view_choices(server, view, kind)
        };
        view_opening(view, kind, favourite, &remembered)
    }

    /// What `view` of `server` remembers, as [`App::library_choices`] for a library: nothing for a view that does not remember
    /// ([`view_remembers`]).
    pub fn view_choices(
        &self,
        server: &ServerRef,
        view: BrowseView,
        kind: Option<&MediaKind>,
    ) -> LibraryChoices {
        match kind.filter(|_| view_remembers(view, kind)) {
            Some(kind) => self.libraries.kind_choices(
                self.settings(),
                &KindPage::new(server.clone(), kind.clone()),
            ),
            None => LibraryChoices::default(),
        }
    }

    /// Remembers the user's choices in `view` of `server`, as [`App::remember_library_choices`] does a library's; nothing for a view
    /// that does not remember ([`view_remembers`]). Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// As [`App::remember_library_choices`].
    pub fn remember_view_choices(
        &self,
        server: &ServerRef,
        view: BrowseView,
        kind: Option<&MediaKind>,
        choices: LibraryChoices,
    ) -> Result<(), StoreError> {
        match kind.filter(|_| view_remembers(view, kind)) {
            Some(kind) => self.libraries.remember_kind(
                self.settings(),
                &KindPage::new(server.clone(), kind.clone()),
                &choices,
            ),
            None => Ok(()),
        }
    }

    /// The UI's decimal separator ("." or ",", from the user's region), for the labels with decimals ([`App::number_format`]): a
    /// platform fact the UI passes once at start. An empty one keeps ".".
    pub fn set_decimal_separator(&self, separator: &str) {
        if !separator.is_empty() {
            self.lock_number_format().decimal_separator = separator.to_owned();
        }
    }

    /// How the labels with decimals write numbers: "." until the UI passes its own ([`App::set_decimal_separator`]).
    pub fn number_format(&self) -> NumberFormat {
        self.lock_number_format().clone()
    }

    /// The UI's platform, for the few texts that name it ("this PC", "Windows"): a platform fact the UI passes once at start.
    /// Only Windows's words exist so far, so every platform gets them; macOS's and Linux's come with their apps (M3, Linux).
    pub fn set_platform(&self, platform: Platform) {
        *self.platform.lock().unwrap_or_else(PoisonError::into_inner) = platform;
    }

    /// The platform the texts are worded for: Windows until the UI passes its own ([`App::set_platform`]).
    pub fn platform(&self) -> Platform {
        // One assignment at a time: a poisoned lock still holds a whole value.
        *self.platform.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The UI's digit grouping, for the counts with grouped digits ("12,345 items"): its group separator and its group sizes, as
    /// [`NumberFormat::group_sizes`] reads them. A platform fact the UI passes once at start. An empty list of sizes keeps groups of 3.
    pub fn set_digit_grouping(&self, separator: &str, sizes: &[u32]) {
        let mut format = self.lock_number_format();
        format.group_separator = separator.to_owned();
        if !sizes.is_empty() {
            format.group_sizes = sizes.to_vec();
        }
    }

    fn lock_number_format(&self) -> std::sync::MutexGuard<'_, NumberFormat> {
        // One assignment at a time: a poisoned lock still holds a whole value.
        self.number_format
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Remembers the user's choices in `library`: each part given (`Some`) whose switch is on; the others stay as they were. Saved at
    /// once; nothing is written when nothing changed. Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub fn remember_library_choices(
        &self,
        library: &ItemRef,
        choices: LibraryChoices,
    ) -> Result<(), StoreError> {
        self.libraries.remember(self.settings(), library, &choices)
    }

    /// The global search page's Filter choices for `filter`, as the capabilities state stands now: the catalog is listed, unavailable
    /// until a Seerr server is signed in ([`browse::search_tools`]).
    pub fn search_tools(&self, filter: &SearchFilter) -> SearchTools {
        browse::search_tools(&self.shared.capabilities.borrow(), filter)
    }

    /// What the global search page opens with: its last filter and view, each only while its switch ("Remember filter", "Remember
    /// view") is on.
    pub fn search_choices(&self) -> SearchMemory {
        self.libraries.search(self.settings())
    }

    /// Remembers the search page's choices: each part given whose switch is on; the other stays as it was. Saved at once; nothing is
    /// written when nothing changed. Blocking file I/O: call it off the async runtime.
    ///
    /// # Errors
    /// As [`App::remember_library_choices`].
    pub fn remember_search_choices(&self, choices: SearchMemory) -> Result<(), StoreError> {
        self.libraries.remember_search(self.settings(), &choices)
    }

    /// The platform's report of the network, on start and on every change (ADR 0014). Every server's address provider hears it, and
    /// providers made later start from it. Call it from one thread (the UI's network observer): the providers hear the facts after
    /// the lock is released, so two concurrent reports could reach a provider in either order.
    pub fn network_changed(&self, facts: NetworkFacts) {
        self.shared.network_changed();
        let providers = {
            let mut roaming = self.lock_roaming();
            roaming
                .providers
                .retain(|provider| provider.strong_count() > 0);
            roaming.facts = Some(facts.clone());
            roaming.providers.clone()
        };
        // Signed-in servers' providers are among them, so their sessions race again too.
        for provider in providers.iter().filter_map(Weak::upgrade) {
            provider.network_changed(&facts);
        }
    }

    /// The app came back to the foreground: servers that are unreachable try again at once (ADR 0014), and every live browse list
    /// refreshes without resetting (ADR 0018).
    pub fn returned_to_foreground(&self) {
        let providers = self.lock_roaming().providers.clone();
        for provider in providers.iter().filter_map(Weak::upgrade) {
            provider.retry_now();
        }
        self.browse.refresh_all();
    }

    /// The platform's report that the app's window is minimized (`true`) or no longer is (`false`), on every change. Every
    /// session's live updates close their socket once the window has been minimized for [`LIVE_PAUSE_AFTER`], and open it again
    /// when it is restored, reading what the lists hold again (ADR 0023). A window that is merely behind others or not focused is
    /// not minimized. Playback is not affected. Restoring is also a return to the foreground: the platform reports that too
    /// ([`App::returned_to_foreground`]).
    pub fn window_minimized(&self, minimized: bool) {
        self.browse.window_minimized(minimized);
    }

    /// Browsing `server` (ADR 0018): `None` unless it is a Jellyfin server that is signed in (its status is available, as
    /// [`Capabilities::is_available`] reads it) with a live session, and the call is made on the tokio runtime. The `Browse`
    /// belongs to that session: after a sign-out, a removal or a new sign-in its lists stop loading and its actions fail with
    /// [`AuthError::NotSignedIn`], and a new `Browse` is needed. Every `Browse` of one session shares its played and favourite
    /// state. Lists load when opened: that is the refresh "on start".
    pub fn browse(&self, server: &ServerRef) -> Option<Browse> {
        self.browse.browse(&self.shared, server)
    }

    /// The latest browse notice (a played or favourite change the server did not take), for a message bar. Only the latest is
    /// kept; its `seq` tells two identical notices apart.
    pub fn browse_notices(&self) -> watch::Receiver<Option<BrowseNotice>> {
        self.browse.notices()
    }

    /// An address provider of a Jellyfin server with id `id` reached at `addresses` (the user's order), for a `ServerClient` of its
    /// own. Saved servers already have one each, made by [`App::restore_sessions`] or a sign-in; this one is separate. Its probes
    /// check the id and go to the shared request log; its races run on `runtime`. It hears network changes from now on.
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
        self.shared.register(&provider);
        Ok(provider)
    }

    fn lock_roaming(&self) -> std::sync::MutexGuard<'_, Roaming> {
        // Each change is one assignment or one `retain`, so a poisoned lock leaves a usable value.
        self.shared
            .roaming
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
    fn the_quality_names_follow_the_codec_the_server_transcodes_to() {
        use current_model::{Codec, TranscodingFacts};
        let details = |video_direct| playback::PlaybackDetails {
            plan: None,
            notes: Vec::new(),
            transcoding: Some(TranscodingFacts {
                video_direct,
                video_codec: Some(if video_direct {
                    Codec::AV1
                } else {
                    Codec::HEVC
                }),
                ..TranscodingFacts::default()
            }),
        };
        let audio = [Codec::AAC];
        let transcoded = details(false);
        let target = quality_target(Some(&transcoded), &audio);
        assert_eq!(target.video_codec, Some(&Codec::HEVC));
        assert_eq!(target.audio_codecs, &audio);
        // A copied video's codec is the source's, not one the server picked: H.264 (`None`) stands.
        let copied = details(true);
        assert_eq!(quality_target(Some(&copied), &audio).video_codec, None);
        let before = quality_target(None, &audio);
        assert_eq!((before.audio, before.video_codec), (None, None));
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
        let (app, report) = App::open(
            temp.path(),
            &temp.path().join("cache"),
            secrets.clone(),
            "Office PC",
        )
        .unwrap();
        assert_eq!(report, LoadReport::Created);
        let id = app.device_id().unwrap().clone();
        assert_eq!(app.settings().unwrap().get().device_id, id);
        assert!(app.secrets().is_some());
        drop(app);

        let (app, report) = App::open(
            temp.path(),
            &temp.path().join("cache"),
            secrets,
            "Office PC",
        )
        .unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(app.device_id(), Some(&id));
    }

    #[test]
    fn the_app_theme_is_saved_at_once_and_survives_reopening() {
        let temp = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn Secrets> = Arc::new(MemorySecrets::new());
        let (app, _) =
            App::open(temp.path(), &temp.path().join("cache"), secrets.clone(), "").unwrap();
        let rx = app.app_theme();
        assert_eq!(*rx.borrow(), AppTheme::System);
        app.set_app_theme(AppTheme::Dark).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow(), AppTheme::Dark);
        assert_eq!(app.settings().unwrap().get().app_theme, AppTheme::Dark);
        drop((rx, app));

        let (app, _) = App::open(temp.path(), &temp.path().join("cache"), secrets, "").unwrap();
        assert_eq!(*app.app_theme().borrow(), AppTheme::Dark);
    }

    #[test]
    fn an_unchanged_app_theme_notifies_no_one() {
        let app = App::new();
        let mut rx = app.app_theme();
        app.set_app_theme(AppTheme::System).unwrap();
        assert!(!rx.has_changed().unwrap());
        // A core that stores nothing still publishes a change.
        app.set_app_theme(AppTheme::Light).unwrap();
        assert_eq!(*rx.borrow_and_update(), AppTheme::Light);
    }

    #[test]
    fn an_app_theme_that_cannot_be_saved_is_not_published() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(current_store::SETTINGS_FILE),
            r#"{"version": 99, "device_id": "abc", "app_theme": "light"}"#,
        )
        .unwrap();
        let (app, report) = App::open(
            temp.path(),
            &temp.path().join("cache"),
            Arc::new(MemorySecrets::new()),
            "",
        )
        .unwrap();
        assert_eq!(report, LoadReport::NewerVersion { found: 99 });
        let rx = app.app_theme();
        assert_eq!(*rx.borrow(), AppTheme::Light);
        assert_eq!(
            app.set_app_theme(AppTheme::Dark),
            Err(StoreError::NewerVersion { found: 99 })
        );
        assert!(!rx.has_changed().unwrap());
        assert_eq!(*rx.borrow(), AppTheme::Light);
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
        ServerAddress::new(format!("http://127.0.0.1:{port}"), false)
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
        let address = ServerAddress::new("not an address", false);
        let result = app.jellyfin_addresses(
            jellyfin(),
            ServerId::new(ID),
            &[address],
            &RuntimeHandle::current(),
        );
        assert_eq!(result.map(|_| ()), Err(RoamingError::InvalidAddress(1)));
    }
}
