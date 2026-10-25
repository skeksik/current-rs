//! The player's driver task (module doc of [`player`](super)): owns the controller, runs the actions, feeds the engine's events and
//! the clock to it, and publishes what it reports.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use current_model::{EngineErrorKind, ImageType, ItemRef, ItemSummary, MediaKind, PlayerState};
use current_playback::{Clock, Controller, EndReason, EngineEvent, PlayerEvent, PlayerStatus};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::backend::{Backend, CompositionSize, DisplayFacts, Signal, SwapChain};
use super::now_playing::{NowPlaying, TIMELINE_INTERVAL, now_playing};
use super::prefs::PlaybackPrefs;
use super::server::JellyfinMedia;
use super::{Action, Command, PlaybackInfo, Player, PlayerError, auto_skip};
use crate::browse::Hub;
use crate::images::{ImageCache, ImageLoad, ImageSource};
use crate::session::Shared;

/// The width the media controls' artwork is loaded at (an ADR 0018 bucket).
pub(crate) const ARTWORK_WIDTH: u32 = 320;

/// What a player needs from the app.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) shared: Arc<Shared>,
    pub(crate) images: ImageCache,
    pub(crate) browse: Arc<Hub>,
    pub(crate) prefs: Arc<PlaybackPrefs>,
}

/// The controller's clock: tokio's, so the driver's sleeps and the controller's deadlines agree.
#[derive(Clone, Copy, Debug)]
struct TokioClock {
    start: Instant,
}

impl Clock for TokioClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Opens a player on this build's engine (see [`super::player_engine`]).
pub(crate) async fn open(
    context: Context,
    size: CompositionSize,
    display: DisplayFacts,
) -> Result<Player, PlayerError> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err(PlayerError::NoRuntime);
    }
    start(context, size, display).await
}

#[cfg(feature = "libmpv")]
async fn start(
    context: Context,
    size: CompositionSize,
    display: DisplayFacts,
) -> Result<Player, PlayerError> {
    use super::mpv::MpvBackend;
    let (events, raw) = mpsc::unbounded_channel();
    // Blocking: libmpv starts its threads and VO, and waits for an earlier engine of the process to go.
    let backend = tokio::task::spawn_blocking(move || MpvBackend::start(size, display, events))
        .await
        .map_err(|_| PlayerError::EngineFailed {
            kind: EngineErrorKind::LOADING_FAILED,
        })?
        .map_err(|error| match error {
            current_mpv::EngineError::Busy => PlayerError::Busy,
            _ => PlayerError::EngineFailed {
                kind: EngineErrorKind::LOADING_FAILED,
            },
        })?;
    Ok(spawn(context, backend, raw))
}

#[cfg(all(not(feature = "libmpv"), any(test, feature = "test-engine")))]
async fn start(
    context: Context,
    size: CompositionSize,
    display: DisplayFacts,
) -> Result<Player, PlayerError> {
    use super::test_engine::TestBackend;
    let (events, raw) = mpsc::unbounded_channel();
    let mut backend = tokio::task::spawn_blocking(move || TestBackend::start(events))
        .await
        .map_err(|_| PlayerError::Busy)?
        .map_err(|_| PlayerError::Busy)?;
    // libmpv takes both as options at start; the test engine records them.
    backend.set_composition_size(size);
    backend.set_display_facts(&display);
    Ok(spawn(context, backend, raw))
}

#[cfg(not(any(feature = "libmpv", test, feature = "test-engine")))]
async fn start(
    _context: Context,
    _size: CompositionSize,
    _display: DisplayFacts,
) -> Result<Player, PlayerError> {
    Err(PlayerError::EngineUnavailable)
}

fn spawn<B: Backend>(context: Context, backend: B, raw: mpsc::UnboundedReceiver<B::Raw>) -> Player {
    let (commands, receiver) = mpsc::unbounded_channel();
    let (state, state_rx) = watch::channel(PlayerStatus::default());
    let (now, now_rx) = watch::channel(NowPlaying::default());
    let (swap_chain, swap_chain_rx) = watch::channel(None);
    let clock = TokioClock {
        start: Instant::now(),
    };
    let auto_skip_rx = context.prefs.auto_skip();
    let initial_skip = auto_skip(&auto_skip_rx.borrow());
    let controller = Controller::new(
        backend,
        JellyfinMedia::default(),
        clock,
        context.prefs.quality(),
        initial_skip,
    );
    let driver = Driver {
        controller,
        context,
        clock,
        raw: Some(raw),
        state,
        now_playing: now,
        swap_chain,
        commands: commands.downgrade(),
        summary: None,
        artwork: None,
        artwork_task: None,
        revision: 0,
        last_revision: Instant::now(),
        auto_skip: Some(auto_skip_rx),
    };
    tokio::spawn(driver.run(receiver));
    Player {
        commands,
        state: state_rx,
        now_playing: now_rx,
        swap_chain: swap_chain_rx,
    }
}

struct Driver<B: Backend> {
    controller: Controller<B, JellyfinMedia, TokioClock>,
    context: Context,
    clock: TokioClock,
    /// `None` once the engine's thread ended.
    raw: Option<mpsc::UnboundedReceiver<B::Raw>>,
    state: watch::Sender<PlayerStatus>,
    now_playing: watch::Sender<NowPlaying>,
    swap_chain: watch::Sender<Option<Arc<SwapChain>>>,
    /// For the artwork task: weak, so the handle's drop still closes the channel.
    commands: mpsc::WeakUnboundedSender<Command>,
    summary: Option<ItemSummary>,
    artwork: Option<Arc<[u8]>>,
    artwork_task: Option<AbortHandle>,
    revision: u64,
    last_revision: Instant,
    auto_skip: Option<watch::Receiver<Vec<String>>>,
}

/// What woke the driver.
enum Wake<R> {
    Command(Command),
    HandleGone,
    Raw(R),
    EngineThreadEnded,
    AutoSkip,
    AutoSkipGone,
    Deadline,
}

/// Runs `action` until it ends, its caller stops waiting (`caller_gone`), or a command arrives that replaces it. Other commands that
/// arrive meanwhile wait in `deferred`. `None` when it did not end.
async fn interruptible<T>(
    action: impl Future<Output = T>,
    caller_gone: impl Future<Output = ()>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    deferred: &mut VecDeque<Command>,
) -> Option<T> {
    tokio::pin!(action, caller_gone);
    loop {
        tokio::select! {
            out = &mut action => return Some(out),
            () = &mut caller_gone => return None,
            command = commands.recv() => match command {
                Some(command) if command.replaces() => {
                    deferred.push_back(command);
                    return None;
                }
                Some(command) => deferred.push_back(command),
                None => {
                    // The handle is gone: close after this.
                    let (done, _) = oneshot::channel();
                    deferred.push_back(Command::Close(done));
                    return None;
                }
            },
        }
    }
}

impl<B: Backend> Driver<B> {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        let mut deferred = VecDeque::new();
        loop {
            let wake = match deferred.pop_front() {
                Some(command) => Wake::Command(command),
                None => self.wait(&mut commands).await,
            };
            match wake {
                Wake::Command(Command::Close(done)) => {
                    self.close().await;
                    let _ = done.send(());
                    return;
                }
                Wake::HandleGone => {
                    self.close().await;
                    return;
                }
                Wake::Command(command) => self.command(command, &mut commands, &mut deferred).await,
                Wake::Raw(raw) => self.engine(raw).await,
                Wake::EngineThreadEnded => self.raw = None,
                Wake::AutoSkip => {
                    if let Some(kinds) = &self.auto_skip {
                        let skip = auto_skip(&kinds.borrow());
                        self.controller.set_auto_skip(skip);
                    }
                }
                Wake::AutoSkipGone => self.auto_skip = None,
                Wake::Deadline => {
                    self.controller.tick().await;
                    if self.revision_due() {
                        self.bump_revision();
                        self.refresh_now_playing();
                    }
                }
            }
            self.drain();
        }
    }

    async fn wait(&mut self, commands: &mut mpsc::UnboundedReceiver<Command>) -> Wake<B::Raw> {
        let deadline = self.deadline();
        let raw = &mut self.raw;
        let skip = &mut self.auto_skip;
        tokio::select! {
            command = commands.recv() => command.map_or(Wake::HandleGone, Wake::Command),
            raw = async { raw.as_mut()?.recv().await }, if raw.is_some() => {
                raw.map_or(Wake::EngineThreadEnded, Wake::Raw)
            }
            changed = async { skip.as_mut()?.changed().await.ok() }, if skip.is_some() => {
                changed.map_or(Wake::AutoSkipGone, |()| Wake::AutoSkip)
            }
            () = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => Wake::Deadline,
        }
    }

    /// The controller's next deadline, or the media controls' next timeline renewal while playing.
    fn deadline(&self) -> Option<Instant> {
        let controller = self
            .controller
            .next_deadline()
            .map(|deadline| self.clock.start + deadline);
        let timeline = (self.state.borrow().state == PlayerState::Playing)
            .then(|| self.last_revision + TIMELINE_INTERVAL);
        controller.into_iter().chain(timeline).min()
    }

    fn revision_due(&self) -> bool {
        self.state.borrow().state == PlayerState::Playing
            && Instant::now() >= self.last_revision + TIMELINE_INTERVAL
    }

    async fn command(
        &mut self,
        command: Command,
        commands: &mut mpsc::UnboundedReceiver<Command>,
        deferred: &mut VecDeque<Command>,
    ) {
        match command {
            Command::Play {
                item,
                start,
                mut done,
            } => {
                // Shown at once: the controller's own states come when the plan is made.
                self.publish_status(PlayerStatus {
                    state: PlayerState::Preparing,
                    item: Some(item.clone()),
                    ..self.controller.status()
                });
                let outcome = {
                    let caller_gone = done.closed();
                    interruptible(self.play(item, start), caller_gone, commands, deferred).await
                };
                match outcome {
                    Some(result) => {
                        let _ = done.send(result);
                        self.start_artwork();
                    }
                    // Given up or replaced while preparing: nothing half-started stays (the controller leaves it with a stop).
                    None => self.controller.stop().await,
                }
            }
            Command::Action { action, mut done } => {
                let caller_gone = done.closed();
                let ran = interruptible(self.action(action), caller_gone, commands, deferred)
                    .await
                    .is_some();
                if ran {
                    let _ = done.send(());
                }
            }
            Command::Info(done) => {
                let _ = done.send(PlaybackInfo {
                    details: self.controller.details(),
                    engine: self.controller.engine().output(),
                });
            }
            Command::Summary(summary) => {
                if self.state.borrow().item.as_ref() == Some(&summary.item) {
                    self.summary = Some(*summary);
                    self.refresh_now_playing();
                }
            }
            Command::Artwork { item, bytes } => {
                if self
                    .summary
                    .as_ref()
                    .is_some_and(|summary| summary.item == item)
                {
                    self.artwork = Some(bytes);
                    self.refresh_now_playing();
                }
            }
            // Handled by `run`.
            Command::Close(_) => {}
        }
    }

    async fn play(&mut self, item: ItemRef, start: Duration) -> Result<(), PlayerError> {
        let (_, client) = self
            .context
            .shared
            .current_session(&item.server)
            .ok_or(PlayerError::NotSignedIn)?;
        let same = self
            .controller
            .server()
            .client()
            .is_some_and(|current| Arc::ptr_eq(&current, &client));
        if !same {
            // The item playing is reported stopped to its own server first.
            self.controller.stop().await;
            self.controller.server().set_client(client.clone());
        }
        if let Some(task) = self.artwork_task.take() {
            task.abort();
        }
        self.artwork = None;
        self.summary = client
            .details(&item)
            .await
            .ok()
            .map(|details| details.summary);
        let kind = self
            .summary
            .as_ref()
            .map_or_else(|| MediaKind::new(""), |summary| summary.kind.clone());
        self.controller.play(item, kind, start).await;
        Ok(())
    }

    async fn action(&mut self, action: Action) {
        let controller = &mut self.controller;
        match action {
            Action::Pause => controller.pause().await,
            Action::Resume => controller.resume().await,
            Action::Toggle => controller.toggle().await,
            Action::SeekTo(position) => controller.seek_to(position).await,
            Action::SeekBy(millis) => controller.seek_by(millis).await,
            Action::Stop => controller.stop().await,
            Action::SelectAudio(index) => controller.select_audio(index).await,
            Action::SelectSubtitle(choice) => controller.select_subtitle(choice).await,
            Action::SetQuality(quality) => controller.set_quality(quality).await,
            Action::SkipSegment => controller.skip_segment().await,
            Action::PlayNext => {
                controller.play_next().await;
                // The next episode's details and artwork load in the background (`drain`).
                self.summary = None;
                self.artwork = None;
                if let Some(task) = self.artwork_task.take() {
                    task.abort();
                }
            }
            Action::DismissNext => controller.dismiss_next(),
            Action::SetVolume(volume) => controller.set_volume(volume),
            Action::SetMuted(muted) => controller.set_muted(muted),
            Action::DisplayFacts(facts) => controller.engine_mut().set_display_facts(&facts),
            Action::CompositionSize(size) => controller.engine_mut().set_composition_size(size),
        }
    }

    async fn engine(&mut self, raw: B::Raw) {
        let mut signals = Vec::new();
        self.controller.engine_mut().translate(raw, &mut signals);
        for signal in signals {
            match signal {
                Signal::Event(event) => self.controller.on_engine_event(event).await,
                // The one replaced is released when the UI lets go of it too.
                Signal::SwapChain(swap_chain) => {
                    self.swap_chain.send_replace(Some(Arc::new(swap_chain)));
                }
                Signal::Gone => {
                    self.controller
                        .on_engine_event(EngineEvent::Ended(EndReason::Error(
                            EngineErrorKind::ENGINE_GONE,
                        )))
                        .await;
                }
            }
        }
    }

    /// Delivers what the controller reported.
    fn drain(&mut self) {
        for event in self.controller.take_events() {
            match event {
                PlayerEvent::State(status) => self.publish_status(*status),
                PlayerEvent::Stopped { item, .. } => self.context.browse.playback_stopped(&item),
                PlayerEvent::QualityChanged { local, quality } => {
                    self.context
                        .prefs
                        .save_quality(&self.context.shared, local, quality);
                }
            }
        }
        // A play of the next episode (by the user or the countdown) replaced the item: its details load in the background.
        let item = self.state.borrow().item.clone();
        if item.is_some()
            && self
                .summary
                .as_ref()
                .is_none_or(|summary| Some(&summary.item) != item.as_ref())
            && self.artwork_task.is_none()
        {
            self.start_artwork();
        }
    }

    fn publish_status(&mut self, status: PlayerStatus) {
        let previous = self.state.borrow().clone();
        let changed_state =
            std::mem::discriminant(&previous.state) != std::mem::discriminant(&status.state);
        let seek_done =
            previous.state == PlayerState::Seeking && status.state != PlayerState::Seeking;
        if changed_state || seek_done || previous.item != status.item {
            self.bump_revision();
        }
        self.state.send_replace(status);
        self.refresh_now_playing();
    }

    fn bump_revision(&mut self) {
        self.revision += 1;
        self.last_revision = Instant::now();
    }

    fn refresh_now_playing(&mut self) {
        let status = self.state.borrow().clone();
        let summary = self
            .summary
            .as_ref()
            .filter(|summary| Some(&summary.item) == status.item.as_ref());
        let value = now_playing(
            &status,
            summary,
            self.artwork.clone(),
            status.next.is_some(),
            self.revision,
        );
        self.now_playing.send_if_modified(|current| {
            if *current == value {
                return false;
            }
            *current = value;
            true
        });
    }

    /// Loads the item's details when missing (after a next-episode play) and its artwork for the media controls: the BlurHash first,
    /// then the image, each sent back as a command.
    fn start_artwork(&mut self) {
        let Some(item) = self.state.borrow().item.clone() else {
            return;
        };
        let Some(commands) = self.commands.upgrade() else {
            return;
        };
        if let Some(task) = self.artwork_task.take() {
            task.abort();
        }
        let summary = self.summary.clone().filter(|summary| summary.item == item);
        let client = self.controller.server().client();
        let shared = self.context.shared.clone();
        let images = self.context.images.clone();
        let task = tokio::spawn(async move {
            let summary = match summary {
                Some(summary) => summary,
                None => {
                    let Some(client) = client else { return };
                    let Ok(details) = client.details(&item).await else {
                        return;
                    };
                    let _ = commands.send(Command::Summary(Box::new(details.summary.clone())));
                    details.summary
                }
            };
            let Some(artwork) = summary
                .artwork
                .iter()
                .find(|artwork| artwork.kind == ImageType::PRIMARY)
                .or_else(|| summary.artwork.first())
            else {
                return;
            };
            let height = artwork.aspect_ratio.map_or(ARTWORK_WIDTH, |ratio| {
                (f64::from(ARTWORK_WIDTH) / ratio.as_f64()).round().max(1.0) as u32
            });
            if let Some(hash) = &artwork.blurhash
                && let Ok(bytes) = images
                    .load(
                        &shared,
                        &ImageLoad {
                            source: ImageSource::BlurHash(hash.clone()),
                            width: ARTWORK_WIDTH,
                            height,
                        },
                    )
                    .await
            {
                let _ = commands.send(Command::Artwork {
                    item: item.clone(),
                    bytes,
                });
            }
            let load = ImageLoad {
                source: ImageSource::Server(artwork.image.clone()),
                width: ARTWORK_WIDTH,
                height,
            };
            if let Ok(bytes) = images.load(&shared, &load).await {
                let _ = commands.send(Command::Artwork { item, bytes });
            }
        });
        self.artwork_task = Some(task.abort_handle());
    }

    /// Stops the item (reporting it), gives the swap chain back and shuts the engine down on the blocking pool.
    async fn close(mut self) {
        self.controller.stop().await;
        self.drain();
        if let Some(task) = self.artwork_task.take() {
            task.abort();
        }
        self.swap_chain.send_replace(None);
        let shutdown = self.controller.engine_mut().take_shutdown();
        let _ = tokio::task::spawn_blocking(shutdown).await;
        self.publish_status(PlayerStatus::default());
    }
}
