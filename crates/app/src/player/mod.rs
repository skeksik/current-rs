//! The player (ADR 0019, task 4): one per window, over the process's one engine for now (a second waits for the first to close, then
//! fails with [`PlayerError::Busy`]). A [`Player`] is a handle; its driver task owns `current-playback`'s controller with the engine
//! and the item's Jellyfin session as its server, and is the only place the controller is called from:
//!
//! - it runs the actions the handle sends, one at a time; a play or a stop that arrives while another action runs replaces it, and an
//!   action whose caller gave up (a cancelled call) is dropped where it stands, which the controller allows;
//! - it hands the controller every engine event (the engine's own thread only sends them into a channel) and every event of the
//!   stream's relay route (ADR 0020; a refusal after the 401 rule has run in the background), and sleeps until the controller's next
//!   deadline otherwise;
//! - it publishes the state (at most 4 a second for the position, at once on any other change: the controller's rate), the media
//!   controls' value and the swap chain to watch channels, tells browse when an item stopped, and saves a picked quality.
//!
//! Closing (or dropping the handle) stops the item, which reports it stopped (best effort, with `current-jellyfin`'s short timeout),
//! gives the swap chain back and shuts the engine down on the blocking pool (ADR 0007's order).
//!
//! No URL, cookie or header value leaves this module: the state, the media controls' value and playback information hold none.

// Without an engine (no `libmpv`, no `test-engine`) no player opens, so the driver and the mapping are unused there; they are still
// compiled, and tested with the test engine.
#![cfg_attr(
    not(any(feature = "libmpv", test, feature = "test-engine")),
    allow(dead_code)
)]

mod backend;
mod driver;
mod known_caps;
#[cfg(test)]
mod live_tests;
mod mpv;
mod now_playing;
mod prefs;
mod relay;
mod server;
#[cfg(any(test, feature = "test-engine"))]
pub mod test_engine;
// With libmpv built in, players use it: these tests need the test engine.
#[cfg(all(test, not(feature = "libmpv")))]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use current_model::{EngineErrorKind, ItemRef, SegmentKind, SubtitleChoice};
use current_playback::{AutoSkip, PlaybackDetails, PlayerStatus, Quality};
use tokio::sync::{mpsc, oneshot, watch};

pub use backend::{CompositionSize, DisplayFacts, EngineOutput, SwapChain, VideoOutput};
pub use known_caps::EngineReport;
pub use now_playing::{MediaStatus, NowPlaying, TIMELINE_INTERVAL};

pub(crate) use driver::{Context, open};
pub(crate) use known_caps::KnownCapabilities;
pub(crate) use prefs::PlaybackPrefs;
pub(crate) use relay::RelaySlot;

/// Which engine [`App::open_player`](crate::App::open_player) uses in this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayerEngine {
    /// None: this build has no engine, and no player opens.
    Unavailable,
    /// libmpv (`current-mpv`), with the `libmpv` feature.
    Libmpv,
    /// The test engine, with the `test-engine` feature: opens nothing (`test_engine`).
    Test,
}

/// The engine of this build: libmpv when built in, else the test engine when built in, else none.
pub const fn player_engine() -> PlayerEngine {
    if cfg!(feature = "libmpv") {
        PlayerEngine::Libmpv
    } else if cfg!(any(test, feature = "test-engine")) {
        PlayerEngine::Test
    } else {
        PlayerEngine::Unavailable
    }
}

/// Why a player could not open or an action could not run. No variant carries a URL or a server's text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlayerError {
    /// This build has no player engine (libmpv is built in only where the app ships with it).
    #[error("this build has no player engine")]
    EngineUnavailable,
    /// Another player still holds the process's engine (one per process for now, ADR 0019).
    #[error("another player is still open")]
    Busy,
    /// The engine could not start.
    #[error("the player engine could not start ({kind})")]
    EngineFailed { kind: EngineErrorKind },
    /// The item's server is not signed in.
    #[error("not signed in to the item's server")]
    NotSignedIn,
    /// The call was outside the core's runtime.
    #[error("the player needs the core's runtime")]
    NoRuntime,
    /// A later play or stop replaced this action before it was done: not a failure (the state shows what plays now).
    #[error("a later play or stop replaced this action")]
    Replaced,
    /// The player is closed.
    #[error("the player is closed")]
    Closed,
}

/// The playback information view (ADR 0019): the plan (route, reasons, source), what explains it, the server's transcoding facts and
/// what the engine reports about its decoding and output. No URL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackInfo {
    pub details: Option<PlaybackDetails>,
    pub engine: EngineOutput,
}

/// The automatic skipping the playback settings name (ADR 0019, choice 8).
pub(crate) fn auto_skip(kinds: &[String]) -> AutoSkip {
    AutoSkip::new(kinds.iter().map(|kind| SegmentKind::new(kind.clone())))
}

/// One user action, for [`Player::send`].
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerAction {
    Pause,
    Resume,
    Toggle,
    SeekTo(Duration),
    SeekBy(i64),
    Stop,
    SelectAudio(u32),
    SelectSubtitle(SubtitleChoice),
    SetQuality(Quality),
    SkipSegment,
    PlayNext,
    DismissNext,
    SetVolume(u8),
    SetMuted(bool),
    DisplayFacts(DisplayFacts),
    CompositionSize(CompositionSize),
}

impl PlayerAction {
    /// Whether the action ends the one running (a stop does; a new play does too, see [`Command::Play`]).
    fn replaces(&self) -> bool {
        matches!(self, PlayerAction::Stop | PlayerAction::PlayNext)
    }
}

pub(crate) enum Command {
    Play {
        item: ItemRef,
        start: Duration,
        done: oneshot::Sender<Result<(), PlayerError>>,
    },
    Action {
        action: PlayerAction,
        /// `None` for an action sent without waiting: it is never given up. Answered `Replaced` when a later play or stop replaced it.
        done: Option<oneshot::Sender<Result<(), PlayerError>>>,
    },
    Info(oneshot::Sender<PlaybackInfo>),
    /// The details of the item playing loaded (after a next-episode play).
    Summary(Box<current_model::ItemSummary>),
    /// The item's artwork loaded: its placeholder, then the server's image.
    Artwork {
        item: ItemRef,
        bytes: Arc<[u8]>,
    },
    /// The 401 rule decided what a refusal of relay route `route`'s stream means (ADR 0020).
    StreamProblem {
        route: u64,
        problem: current_model::PlaybackProblem,
    },
    Close(oneshot::Sender<()>),
}

impl Command {
    fn replaces(&self) -> bool {
        match self {
            Command::Play { .. } | Command::Close(_) => true,
            Command::Action { action, .. } => action.replaces(),
            Command::Info(_)
            | Command::Summary(_)
            | Command::Artwork { .. }
            | Command::StreamProblem { .. } => false,
        }
    }
}

/// A player: one per window. Actions return once the driver ran them (a play once the item is loading or failed: its state tells);
/// dropping an action's future cancels it. Dropping the player closes it in the background; [`Player::close`] waits.
pub struct Player {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<PlayerStatus>,
    now_playing: watch::Receiver<NowPlaying>,
    swap_chain: watch::Receiver<Option<Arc<SwapChain>>>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("state", &self.state.borrow().state)
            .finish_non_exhaustive()
    }
}

impl Player {
    /// The state, published at most 4 times a second for the position and at once on any other change.
    pub fn state(&self) -> watch::Receiver<PlayerStatus> {
        self.state.clone()
    }

    /// What the system media controls need.
    pub fn now_playing(&self) -> watch::Receiver<NowPlaying> {
        self.now_playing.clone()
    }

    /// The engine's swap chain for the UI's panel (Windows), `None` until the engine has one and after closing.
    pub fn swap_chain(&self) -> watch::Receiver<Option<Arc<SwapChain>>> {
        self.swap_chain.clone()
    }

    /// Plays `item` from `start` (a resume position or zero: the details screen decides, ADR 0018). Returns once the item is loading,
    /// or failed (the state says why).
    pub async fn play(&self, item: ItemRef, start: Duration) -> Result<(), PlayerError> {
        let (done, answer) = oneshot::channel();
        self.command(Command::Play { item, start, done })?;
        answer.await.map_err(|_| PlayerError::Closed)?
    }

    async fn act(&self, action: PlayerAction) -> Result<(), PlayerError> {
        let (done, answer) = oneshot::channel();
        self.command(Command::Action {
            action,
            done: Some(done),
        })?;
        answer.await.map_err(|_| PlayerError::Closed)?
    }

    /// Queues `action` without waiting for it: actions run in the order they are sent. For quick actions from the UI thread (a key, a
    /// button); a cancellable one (a track or quality change re-plans) is better awaited.
    pub fn send(&self, action: PlayerAction) -> Result<(), PlayerError> {
        self.command(Command::Action { action, done: None })
    }

    pub async fn pause(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::Pause).await
    }

    pub async fn resume(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::Resume).await
    }

    pub async fn toggle(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::Toggle).await
    }

    pub async fn seek_to(&self, position: Duration) -> Result<(), PlayerError> {
        self.act(PlayerAction::SeekTo(position)).await
    }

    /// Seeks by `millis`, back when negative.
    pub async fn seek_by(&self, millis: i64) -> Result<(), PlayerError> {
        self.act(PlayerAction::SeekBy(millis)).await
    }

    pub async fn stop(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::Stop).await
    }

    /// The audio stream with this server index.
    pub async fn select_audio(&self, index: u32) -> Result<(), PlayerError> {
        self.act(PlayerAction::SelectAudio(index)).await
    }

    pub async fn select_subtitle(&self, choice: SubtitleChoice) -> Result<(), PlayerError> {
        self.act(PlayerAction::SelectSubtitle(choice)).await
    }

    /// The quality for the kind of address in use; remembered for that kind (local or internet) in the settings.
    pub async fn set_quality(&self, quality: Quality) -> Result<(), PlayerError> {
        self.act(PlayerAction::SetQuality(quality)).await
    }

    /// Seeks past the segment under the position.
    pub async fn skip_segment(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::SkipSegment).await
    }

    /// Plays the next episode now, from its start.
    pub async fn play_next(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::PlayNext).await
    }

    /// Hides the "Next episode" card and stops its countdown.
    pub async fn dismiss_next(&self) -> Result<(), PlayerError> {
        self.act(PlayerAction::DismissNext).await
    }

    /// 0 to 100.
    pub async fn set_volume(&self, volume: u8) -> Result<(), PlayerError> {
        self.act(PlayerAction::SetVolume(volume)).await
    }

    pub async fn set_muted(&self, muted: bool) -> Result<(), PlayerError> {
        self.act(PlayerAction::SetMuted(muted)).await
    }

    /// What the monitor the window is on reports (ADR 0007).
    pub async fn set_display_facts(&self, facts: DisplayFacts) -> Result<(), PlayerError> {
        self.act(PlayerAction::DisplayFacts(facts)).await
    }

    /// The panel's size in physical pixels.
    pub async fn set_composition_size(&self, size: CompositionSize) -> Result<(), PlayerError> {
        self.act(PlayerAction::CompositionSize(size)).await
    }

    /// The playback information view's content.
    pub async fn playback_info(&self) -> Result<PlaybackInfo, PlayerError> {
        let (done, answer) = oneshot::channel();
        self.command(Command::Info(done))?;
        answer.await.map_err(|_| PlayerError::Closed)
    }

    /// Stops the item, gives the swap chain back and shuts the engine down; returns when it is gone. Release the swap chain in the UI
    /// (after detaching the panel) first.
    pub async fn close(&self) {
        let (done, answer) = oneshot::channel();
        if self.command(Command::Close(done)).is_ok() {
            let _ = answer.await;
        }
    }

    fn command(&self, command: Command) -> Result<(), PlayerError> {
        self.commands.send(command).map_err(|_| PlayerError::Closed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let (done, _) = oneshot::channel();
        let _ = self.commands.send(Command::Close(done));
    }
}
