//! The player across the bridge (ADR 0019, "What crosses `current-ffi`"): [`Core::open_player`] gives a [`Player`] per window, with
//! subscriptions to its state, the media controls' value and the swap chain, actions, playback information, and the playback
//! settings. No URL, cookie or header value crosses: every record is in `player_records`.
//!
//! The long actions (play, a track or quality change, the next episode: each may ask the server for a new plan) are `async` with a
//! [`CancelToken`]; the quick ones (pause, seek, volume, ...) are queued without waiting and run in the order they were called.

use std::sync::Arc;
use std::time::Duration;

use current_app::{PlayerAction, SwapChain};
use tokio::runtime::Handle;

use crate::cancel::{self, CancelToken};
use crate::player_records::{
    CompositionSize, DisplayFacts, ItemRef, NowPlayingSnapshot, PlaybackInfoSnapshot,
    PlaybackSettingsSnapshot, PlayerEngineKind, PlayerStateSnapshot, QualityChoice,
    SubtitleSelection,
};
use crate::subscribe::{self, LiveListeners};
use crate::{Core, CoreError, TaskHandle};

/// Receives the player's state: the current value first, then every change (at most 4 a second for the position), on a core thread.
/// It must only hand the value to the UI thread and return, and must never throw.
#[uniffi::export(callback_interface)]
pub trait PlayerStateListener: Send + Sync {
    fn on_value(&self, value: PlayerStateSnapshot);
}

/// Receives what the system media controls show, as [`PlayerStateListener`] does.
#[uniffi::export(callback_interface)]
pub trait NowPlayingListener: Send + Sync {
    fn on_value(&self, value: NowPlayingSnapshot);
}

/// Receives the engine's swap chain (Windows), `None` until there is one and after the player closed, as [`PlayerStateListener`]
/// does. Hand it to the UI thread, `SetSwapChain` the handle, then call [`SwapChainHandle::release`].
#[uniffi::export(callback_interface)]
pub trait SwapChainListener: Send + Sync {
    fn on_value(&self, value: Option<Arc<SwapChainHandle>>);
}

/// A reference to the engine's swap chain: `handle` is the `IDXGISwapChain*` as an integer for `ISwapChainPanelNative::SetSwapChain`.
/// Release it once `SetSwapChain` took its own reference; releasing twice does nothing, and dropping the last handle releases too.
#[derive(Debug, uniffi::Object)]
pub struct SwapChainHandle {
    swap_chain: Arc<SwapChain>,
}

#[uniffi::export]
impl SwapChainHandle {
    pub fn handle(&self) -> u64 {
        self.swap_chain.handle()
    }

    /// Grows with every new swap chain, so the UI can tell a new one from a repeat.
    pub fn generation(&self) -> u64 {
        self.swap_chain.generation()
    }

    pub fn release(&self) {
        self.swap_chain.release();
    }

    pub fn is_released(&self) -> bool {
        self.swap_chain.is_released()
    }
}

/// The engine of this build: libmpv in the Windows app, none in a build without it, the test engine only in test builds.
#[uniffi::export]
pub fn player_engine() -> PlayerEngineKind {
    current_app::player_engine().into()
}

fn player_error(error: &current_app::PlayerError) -> CoreError {
    CoreError::Player {
        problem: error.into(),
    }
}

/// A player for one window. Close it with [`Player::close`] after detaching the panel and releasing the swap chain; dropping it
/// closes it in the background.
#[derive(uniffi::Object)]
pub struct Player {
    player: Arc<current_app::Player>,
    runtime: Handle,
    listeners: LiveListeners,
}

impl Player {
    pub(crate) fn new(
        player: current_app::Player,
        runtime: Handle,
        listeners: LiveListeners,
    ) -> Self {
        Self {
            player: Arc::new(player),
            runtime,
            listeners,
        }
    }

    fn quick(&self, action: PlayerAction) -> Result<(), CoreError> {
        self.player
            .send(action)
            .map_err(|error| player_error(&error))
    }
}

/// Runs a long action on the core's runtime until it ends or `token` is cancelled; cancelling drops it, which cancels it in the
/// player too.
macro_rules! long {
    ($self:ident, $token:ident, |$player:ident| $action:expr) => {{
        let $player = $self.player.clone();
        cancel::run_cancellable(&$self.runtime, $token, async move {
            $action.await.map_err(|error| player_error(&error))
        })
        .await
    }};
}

#[uniffi::export]
impl Player {
    /// Plays `item` from `start_ms` (a resume position or zero: the details screen decides). Returns once the item is loading or
    /// failed: the state says which and why. A play while another item plays stops that one first.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`]; [`CoreError::Player`] with `NotSignedIn` or `Closed`.
    pub async fn play(
        &self,
        item: ItemRef,
        start_ms: u64,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        let item: current_model::ItemRef = (&item).into();
        long!(self, token, |player| player
            .play(item, Duration::from_millis(start_ms)))
    }

    /// The audio stream with this server index. On a server stream the server makes a new stream at the position.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`]; [`CoreError::Player`] with `Closed`.
    pub async fn select_audio(&self, index: u32, token: Arc<CancelToken>) -> Result<(), CoreError> {
        long!(self, token, |player| player.select_audio(index))
    }

    /// A subtitle choice; a burned-in one on a server stream makes a new stream at the position.
    ///
    /// # Errors
    /// As [`Player::select_audio`].
    pub async fn select_subtitle(
        &self,
        subtitle: SubtitleSelection,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        long!(self, token, |player| player
            .select_subtitle(subtitle.into()))
    }

    /// The quality for the kind of address in use, remembered for it (local or internet); plays on at the position with it.
    ///
    /// # Errors
    /// As [`Player::select_audio`].
    pub async fn set_quality(
        &self,
        quality: QualityChoice,
        token: Arc<CancelToken>,
    ) -> Result<(), CoreError> {
        long!(self, token, |player| player.set_quality(quality.into()))
    }

    /// Plays the next episode now, from its start.
    ///
    /// # Errors
    /// As [`Player::select_audio`].
    pub async fn play_next(&self, token: Arc<CancelToken>) -> Result<(), CoreError> {
        long!(self, token, |player| player.play_next())
    }

    /// # Errors
    /// [`CoreError::Player`] with `Closed`, for this and the other quick actions.
    pub fn pause(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::Pause)
    }

    pub fn resume(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::Resume)
    }

    pub fn toggle(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::Toggle)
    }

    pub fn seek_to(&self, position_ms: u64) -> Result<(), CoreError> {
        self.quick(PlayerAction::SeekTo(Duration::from_millis(position_ms)))
    }

    /// Back when negative.
    pub fn seek_by(&self, offset_ms: i64) -> Result<(), CoreError> {
        self.quick(PlayerAction::SeekBy(offset_ms))
    }

    /// Stops the item (reported to the server) and keeps the engine for the next one.
    pub fn stop(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::Stop)
    }

    /// Seeks past the segment under the position.
    pub fn skip_segment(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::SkipSegment)
    }

    /// Hides the "Next episode" card and stops its countdown.
    pub fn dismiss_next(&self) -> Result<(), CoreError> {
        self.quick(PlayerAction::DismissNext)
    }

    /// 0 to 100.
    pub fn set_volume(&self, volume: u8) -> Result<(), CoreError> {
        self.quick(PlayerAction::SetVolume(volume.min(100)))
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), CoreError> {
        self.quick(PlayerAction::SetMuted(muted))
    }

    /// The window moved to another monitor, or the monitor's mode changed (ADR 0007).
    pub fn set_display_facts(&self, facts: DisplayFacts) -> Result<(), CoreError> {
        self.quick(PlayerAction::DisplayFacts(facts.into()))
    }

    /// The panel's new size in physical pixels.
    pub fn set_composition_size(&self, size: CompositionSize) -> Result<(), CoreError> {
        self.quick(PlayerAction::CompositionSize(size.into()))
    }

    /// The state now.
    pub fn state(&self) -> PlayerStateSnapshot {
        (&*self.player.state().borrow()).into()
    }

    pub fn subscribe_state(&self, listener: Box<dyn PlayerStateListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.player.state(), move |status| {
            listener.listener.on_value((&status).into());
        })
    }

    pub fn subscribe_now_playing(&self, listener: Box<dyn NowPlayingListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.player.now_playing(), move |now| {
            listener.listener.on_value((&now).into());
        })
    }

    pub fn subscribe_swap_chain(&self, listener: Box<dyn SwapChainListener>) -> Arc<TaskHandle> {
        let listener = self.listeners.track(listener);
        subscribe::watch_value(&self.runtime, self.player.swap_chain(), move |swap_chain| {
            listener
                .listener
                .on_value(swap_chain.map(|swap_chain| Arc::new(SwapChainHandle { swap_chain })));
        })
    }

    /// The playback information view's content.
    ///
    /// # Errors
    /// [`CoreError::Player`] with `Closed`.
    pub async fn playback_info(&self) -> Result<PlaybackInfoSnapshot, CoreError> {
        let player = self.player.clone();
        self.runtime
            .spawn(async move { player.playback_info().await })
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map(|info| (&info).into())
            .map_err(|error| player_error(&error))
    }

    /// Stops the item (reported to the server, best effort with a short timeout), gives the swap chain back and shuts the engine down;
    /// returns once it is gone, so the next player can open. Detach the panel and release the swap chain first. Not cancellable.
    pub async fn close(&self) {
        let player = self.player.clone();
        let _ = self
            .runtime
            .spawn(async move { player.close().await })
            .await;
    }
}

#[uniffi::export]
impl Core {
    /// Opens a player for a window with the panel's size in physical pixels (needed before the engine starts, ADR 0007) and the
    /// display's facts. One engine per process for now: while another player is open this waits a few seconds for it to close.
    ///
    /// # Errors
    /// [`CoreError::Player`] with `EngineUnavailable` in a build without an engine, `Busy` while another player stays open, or
    /// `EngineFailed`.
    pub async fn open_player(
        &self,
        size: CompositionSize,
        display: DisplayFacts,
    ) -> Result<Arc<Player>, CoreError> {
        let app = self.app.clone();
        let player = self
            .runtime
            .spawn(async move { app.open_player(size.into(), display.into()).await })
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| player_error(&error))?;
        Ok(Arc::new(Player::new(
            player,
            self.runtime.clone(),
            self.listeners.clone(),
        )))
    }

    /// Settings → Playback now.
    pub fn playback_settings(&self) -> PlaybackSettingsSnapshot {
        PlaybackSettingsSnapshot::new(self.app.auto_skip().borrow().clone())
    }

    /// The Settings switch: skip intros and credits (`intro` and `outro` segments) automatically, or not. Saved at once on the core's
    /// blocking pool; open players follow at once.
    ///
    /// # Errors
    /// [`CoreError::Store`] if the settings could not be saved: the setting stands for this run.
    pub async fn set_skip_intros_and_credits(&self, skip: bool) -> Result<(), CoreError> {
        let kinds = if skip {
            vec!["intro".to_owned(), "outro".to_owned()]
        } else {
            Vec::new()
        };
        self.set_auto_skip(kinds).await
    }

    /// The segment kinds skipped automatically, by open name; empty turns it off. Saved as [`Core::set_skip_intros_and_credits`].
    ///
    /// # Errors
    /// As [`Core::set_skip_intros_and_credits`].
    pub async fn set_auto_skip(&self, kinds: Vec<String>) -> Result<(), CoreError> {
        let app = self.app.clone();
        self.runtime
            .spawn_blocking(move || app.set_auto_skip(kinds))
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| CoreError::Store {
                problem: (&error).into(),
            })
    }
}
