//! The playback controller (ADR 0019, "Playback controller"): one per player, a state machine driven by user actions, engine events
//! and a clock.
//!
//! The composition root owns it and calls it from one task: actions as the user acts, [`Controller::on_engine_event`] for each
//! event of the app engine and [`Controller::on_native_engine_event`] for each of the native engine (ADR 0025), and [`Controller::tick`] at [`Controller::next_deadline`]. It reports to the server, drives the engine and queues
//! [`PlayerEvent`]s, which the root drains with [`Controller::take_events`] and delivers to listeners. A future dropped part way (a
//! cancelled action) leaves the controller in a state the next action can leave.

use std::time::Duration;

use current_model::{
    EngineRole, HowToPlay, ItemRef, ItemSummary, MediaKind, MediaStream, PlaybackProblem,
    PlayerState, Route, RouteReason, Segment, SegmentKind, ServerOffer, StreamKind, StreamProtocol,
    SubtitleChoice, SubtitleDelivery, SubtitleFormat, TrackChoice, TranscodingFacts,
};

use crate::clock::Clock;
use crate::engine::{EndReason, Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};
use crate::engines::{DeviceRemux, DisplayKind, EngineProfile, Engines, PlatformRules};
use crate::planner::{self, Planned, RouteInputs};
use crate::quality::{Quality, QualityPreference, snap_quality};
use crate::route::{self, RouteContext};
use crate::server::{MediaServer, ProgressReport, StopReport, TranscodePermissions};
use crate::status::{AutoSkip, NextEpisode, PlaybackDetails, PlayerEvent, PlayerStatus, Track};

/// Progress reports while a file plays, paused or not (ADR 0019, choice 7); well under the server's 60 s HLS kill timer.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
/// The shortest time between two published states that differ only in position or buffer: at most 4 a second.
pub const STATE_INTERVAL: Duration = Duration::from_millis(250);
/// Without an outro segment, the "Next episode" card shows in the last 30 s (choice 8).
pub const NEXT_EPISODE_LEAD: Duration = Duration::from_secs(30);
/// The next episode starts this long after the end when the user's autoplay is on (choice 8).
pub const AUTOPLAY_COUNTDOWN: Duration = Duration::from_secs(10);
/// Loading, seeking, or buffering while not paused, with no position or buffer change for this long, playback fails as
/// [`PlaybackProblem::Network`]: FFmpeg can wait on a dead connection for minutes without an error (docs/notes/windows-shell.md,
/// "Player: Windows run", case 7). Twice the 30 s timeouts: mpv's `network-timeout=30` (`current-mpv`'s options) runs out first, so an
/// error the engine does report takes the engine-failure path and its fallback; the other 30 s, `current-net`'s request timeout,
/// covers a server that is slow to make the first segment of a transcode.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// A player's native engine (ADR 0025): the platform's own player beside the app engine, with what the platform adds to it. Boxed: on
/// Windows and Apple it is a platform object behind the bridge (ADR 0025, "How a platform engine plugs into the core").
pub struct NativeEngine {
    pub engine: Box<dyn Engine + Send>,
    /// Whether it plays HLS.
    pub hls: bool,
    pub remux: Option<DeviceRemux>,
    pub rules: PlatformRules,
}

impl std::fmt::Debug for NativeEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEngine")
            .field("hls", &self.hls)
            .field("remux", &self.remux)
            .field("rules", &self.rules)
            .finish_non_exhaustive()
    }
}

/// Whether `candidate` is the route `current`: for a server stream, the same engine (a new answer may differ in what it copies).
fn same_route(candidate: &Route, current: &Route) -> bool {
    if candidate.is_local() || current.is_local() {
        candidate == current
    } else {
        candidate.engine() == current.engine()
    }
}

/// The engine of `role`: the native one when there is one, else the app engine.
fn pick<'a, E: Engine>(
    app: &'a mut E,
    native: &'a mut Option<NativeEngine>,
    role: EngineRole,
) -> &'a mut dyn Engine {
    match (role, native) {
        (EngineRole::Native, Some(native)) => native.engine.as_mut(),
        _ => app,
    }
}

/// Why a plan and load did not leave a file loading.
#[derive(Debug)]
enum LoadFailure {
    Problem(PlaybackProblem),
    /// With two engines, the engine of the planned route refused the file at once: a failed route.
    Refused(current_model::EngineErrorKind),
}

impl From<PlaybackProblem> for LoadFailure {
    fn from(problem: PlaybackProblem) -> Self {
        Self::Problem(problem)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Preparing,
    Loading,
    /// A file is open and playing, paused, buffering or seeking.
    Active,
    Ended,
    Failed(PlaybackProblem),
}

/// One item being played. A re-plan (a track or quality change, the fallback) keeps it and replaces the plan.
#[derive(Debug)]
struct Session {
    item: ItemRef,
    /// The quality limit in use: the preferred quality until the first plan, then the entry of the item's quality menu it snapped to
    /// ([`snap_quality`]), or the user's pick, which only this item keeps.
    quality: Quality,
    planned: Option<Planned>,
    /// Notes when no plan came out.
    notes: Vec<crate::planner::PlanNote>,
    position: Duration,
    duration: Option<Duration>,
    buffered: Option<Duration>,
    paused: bool,
    buffering: bool,
    seeking: bool,
    /// When the engine last moved (an event that changed something, or a command); the stall rule counts from here.
    last_progress: Duration,
    /// The stream's connection failed while the engine was playing from its buffer ([`Controller::stream_failed`]): an engine
    /// error, a stall or an end short of the duration that follows is that failure, not the file's. Cleared once data arrives again.
    stream_lost: bool,
    /// A refusal of the stream is with the 401 rule ([`Controller::stream_refused`]): what the engine reports about the refused
    /// stream (an error or an end) waits for its answer.
    refusal_pending: bool,
    /// Whether the server was told this play session started, and that it stopped.
    started: bool,
    stop_reported: bool,
    /// Whether the rest of the app was told the item stopped.
    item_stopped: bool,
    last_report: Duration,
    /// An engine failed in this play and the plan moved on (ADR 0019 choice 9, ADR 0025's chain).
    fallback_used: bool,
    /// The routes that failed in this play, oldest first (ADR 0025): none is tried again.
    failed_routes: Vec<Route>,
    /// The engine that has the file, once loaded.
    loaded_on: Option<EngineRole>,
    /// "How to play" for this playback: Settings' as the item started (a change there applies from the next item), or the one
    /// chosen in the player for this item (ADR 0025, choice 3 as amended; owner, 2026-10-31: "for the current playback").
    how: HowToPlay,
    /// `how` was chosen in the player.
    chosen_here: bool,
    /// The native engine ignored the speed with these tracks, so the app engine plays this playback (ADR 0025, "Track changes").
    speed_not_supported: bool,
    /// What the engine playing last said of rates other than 1 ([`EngineEvent::RatesSupported`]).
    rates_supported: bool,
    permissions: TranscodePermissions,
    facts: Option<TranscodingFacts>,
    /// External subtitles added to the file playing, by server index.
    added_subtitles: Vec<u32>,
    segments: Vec<Segment>,
    /// Per segment: skipped automatically once in this play.
    auto_skipped: Vec<bool>,
    segment: Option<usize>,
    next: Option<ItemSummary>,
    autoplay: bool,
    next_shown: bool,
    next_dismissed: bool,
    countdown_until: Option<Duration>,
}

impl Session {
    fn new(item: ItemRef, quality: Quality, start: Duration, now: Duration) -> Self {
        Self {
            item,
            quality,
            planned: None,
            notes: Vec::new(),
            position: start,
            duration: None,
            buffered: None,
            paused: false,
            buffering: false,
            seeking: false,
            last_progress: now,
            stream_lost: false,
            refusal_pending: false,
            started: false,
            stop_reported: false,
            item_stopped: false,
            last_report: now,
            fallback_used: false,
            failed_routes: Vec::new(),
            loaded_on: None,
            how: HowToPlay::Auto,
            chosen_here: false,
            speed_not_supported: false,
            rates_supported: true,
            permissions: TranscodePermissions::default(),
            facts: None,
            added_subtitles: Vec::new(),
            segments: Vec::new(),
            auto_skipped: Vec::new(),
            segment: None,
            next: None,
            autoplay: false,
            next_shown: false,
            next_dismissed: false,
            countdown_until: None,
        }
    }

    fn tracks(&self) -> TrackChoice {
        self.planned
            .as_ref()
            .map_or_else(TrackChoice::default, |planned| planned.plan.tracks)
    }

    /// The route plays the file on the device (native, remux or app engine), not a server stream.
    fn is_local(&self) -> bool {
        self.planned
            .as_ref()
            .is_some_and(|planned| planned.plan.route.is_local())
    }

    fn subtitle_stream(&self, choice: SubtitleChoice) -> Option<&MediaStream> {
        match choice {
            SubtitleChoice::Stream(index) => self.planned.as_ref()?.plan.source.stream(index),
            _ => None,
        }
    }

    /// Whether the core fetches this subtitle for the engine: an external file under direct play, or whatever the server delivers
    /// as a separate file on a server stream.
    fn fetches(&self, stream: &MediaStream) -> bool {
        if self.is_local() {
            stream.is_external
        } else {
            stream.subtitle_delivery.as_ref() == Some(&SubtitleDelivery::EXTERNAL)
        }
    }

    /// Whether the server burns this subtitle into a server stream's video.
    fn burned_in(&self, choice: SubtitleChoice) -> bool {
        !self.is_local()
            && self.subtitle_stream(choice).is_some_and(|stream| {
                stream.subtitle_delivery.as_ref() == Some(&SubtitleDelivery::ENCODE)
            })
    }

    /// Whether the position is at the end of the file (within a second), as a real end of the file leaves it.
    fn at_end(&self) -> bool {
        self.duration
            .is_some_and(|duration| self.position + Duration::from_secs(1) >= duration)
    }

    /// Where the "Next episode" card starts: the first outro segment, else the last 30 s.
    fn next_threshold(&self) -> Option<Duration> {
        self.segments
            .iter()
            .find(|segment| segment.kind == SegmentKind::OUTRO)
            .map(|segment| segment.start)
            .or_else(|| {
                self.duration
                    .map(|duration| duration.saturating_sub(NEXT_EPISODE_LEAD))
            })
    }
}

/// The playback controller. `E` is the player's app engine, kept for every file it plays; a native engine may join it
/// ([`Controller::with_native_engine`]).
pub struct Controller<E, S, C> {
    engine: E,
    native: Option<NativeEngine>,
    display: DisplayKind,
    /// Settings' "How to play".
    how: HowToPlay,
    /// The playback rate, 1 for normal.
    speed: f64,
    server: S,
    clock: C,
    quality: QualityPreference,
    auto_skip: AutoSkip,
    phase: Phase,
    session: Option<Session>,
    volume: u8,
    muted: bool,
    events: Vec<PlayerEvent>,
    last_published: Option<Duration>,
    publish_pending: bool,
    published_countdown: Option<u32>,
}

impl<E: Engine, S: MediaServer, C: Clock> Controller<E, S, C> {
    /// A controller with Settings' quality choices and the auto-skip setting, both loaded by the composition root.
    pub fn new(
        engine: E,
        server: S,
        clock: C,
        quality: QualityPreference,
        auto_skip: AutoSkip,
    ) -> Self {
        Self {
            engine,
            native: None,
            display: DisplayKind::Unknown,
            how: HowToPlay::Auto,
            speed: 1.0,
            server,
            clock,
            quality,
            auto_skip,
            phase: Phase::Idle,
            session: None,
            volume: 100,
            muted: false,
            events: Vec::new(),
            last_published: None,
            publish_pending: false,
            published_countdown: None,
        }
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// The controller with a native engine beside the app engine (ADR 0025), as the player opens; volume and mute are the app
    /// engine's.
    pub fn with_native_engine(mut self, mut native: NativeEngine) -> Self {
        native.engine.set_volume(self.volume);
        native.engine.set_muted(self.muted);
        self.native = Some(native);
        self
    }

    /// The controller with the display the player's window opens on (departure 8).
    pub fn with_display(mut self, display: DisplayKind) -> Self {
        self.display = display;
        self
    }

    pub fn native_engine_mut(&mut self) -> Option<&mut NativeEngine> {
        self.native.as_mut()
    }

    /// Settings → Playback → "How to play" (ADR 0025, choice 3 as amended), from the next item played; the item playing keeps the
    /// one it started with.
    pub fn set_how_to_play(&mut self, how: HowToPlay) {
        self.how = how;
    }

    /// The engine that has the file: the app engine when nothing is loaded.
    fn role(&self) -> EngineRole {
        self.session
            .as_ref()
            .and_then(|session| session.loaded_on)
            .unwrap_or(EngineRole::App)
    }

    /// The engine that has the file.
    fn playing(&mut self) -> &mut dyn Engine {
        let role = self.role();
        pick(&mut self.engine, &mut self.native, role)
    }

    /// The player's engines, for a plan.
    fn engines(&self) -> Engines {
        let app = self.engine.capabilities();
        match &self.native {
            None => Engines::app_only(app),
            Some(native) => Engines {
                native: Some(EngineProfile::new(native.engine.capabilities(), native.hls)),
                app: EngineProfile::new(app, true),
                remux: native.remux.clone(),
                rules: native.rules,
            },
        }
    }

    fn route_context(&self) -> RouteContext {
        let session = self.session.as_ref();
        RouteContext {
            display: self.display,
            how: session.map_or(self.how, |session| session.how),
            chosen_here: session.is_some_and(|session| session.chosen_here),
            speed_not_supported: session.is_some_and(|session| session.speed_not_supported),
        }
    }

    /// Whether the route playing still plays the item with `tracks` (ADR 0025, "Track changes"). Always with the app engine only.
    fn route_fits(&self, tracks: TrackChoice) -> bool {
        if self.native.is_none() {
            return true;
        }
        let Some(current) = self.current_route() else {
            return true;
        };
        self.candidates(tracks, self.route_context())
            .iter()
            .any(|candidate| same_route(candidate, &current))
    }

    fn current_route(&self) -> Option<Route> {
        self.session
            .as_ref()?
            .planned
            .as_ref()
            .map(|planned| planned.plan.route.clone())
    }

    /// The routes that would play the item playing with `tracks` in `context`, in order, leaving out those that failed: local ones
    /// for a local route, the engines that take its stream for a server stream.
    fn candidates(&self, tracks: TrackChoice, context: RouteContext) -> Vec<Route> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let Some(planned) = &session.planned else {
            return Vec::new();
        };
        let engines = self.engines();
        let source = &planned.plan.source;
        match &planned.plan.route {
            Route::ServerStream {
                protocol,
                container,
                video,
                audio,
                ..
            } => {
                let offer = ServerOffer::Stream {
                    protocol: *protocol,
                    container: container.clone(),
                    video: *video,
                    audio: *audio,
                };
                route::server_routes(
                    &offer,
                    source,
                    tracks,
                    &engines,
                    context,
                    &session.failed_routes,
                )
            }
            _ => route::local_routes(source, tracks, &engines, context, &session.failed_routes)
                .into_iter()
                .map(|choice| choice.route)
                .collect(),
        }
    }

    pub fn server(&self) -> &S {
        &self.server
    }

    /// The events since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<PlayerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn quality_preference(&self) -> QualityPreference {
        self.quality
    }

    /// Settings' quality choices changed: they apply from the next item played, not to the one playing.
    pub fn set_quality_preference(&mut self, quality: QualityPreference) {
        self.quality = quality;
    }

    pub fn set_auto_skip(&mut self, auto_skip: AutoSkip) {
        self.auto_skip = auto_skip;
    }

    pub fn status(&self) -> PlayerStatus {
        let state = match &self.phase {
            Phase::Idle => PlayerState::Idle,
            Phase::Preparing => PlayerState::Preparing,
            Phase::Loading => PlayerState::Loading,
            Phase::Ended => PlayerState::Ended,
            Phase::Failed(problem) => PlayerState::Failed {
                problem: problem.clone(),
            },
            Phase::Active => match &self.session {
                Some(session) if session.seeking => PlayerState::Seeking,
                Some(session) if session.buffering => PlayerState::Buffering,
                Some(session) if session.paused => PlayerState::Paused,
                _ => PlayerState::Playing,
            },
        };
        let mut status = PlayerStatus {
            state,
            quality: self.current_quality(),
            volume: self.volume,
            muted: self.muted,
            ..PlayerStatus::default()
        };
        let Some(session) = &self.session else {
            return status;
        };
        status.item = Some(session.item.clone());
        status.position = session.position;
        status.duration = session.duration;
        status.buffered = session.buffered;
        status.segment = session.segment.map(|index| session.segments[index].clone());
        status.next = session
            .next
            .as_ref()
            .filter(|_| session.next_shown || session.countdown_until.is_some())
            .map(|item| NextEpisode {
                item: item.clone(),
                countdown: self.countdown(session),
            });
        if let Some(planned) = &session.planned {
            let tracks = |kind: StreamKind| {
                planned
                    .plan
                    .source
                    .streams_of(&kind)
                    .map(Track::from_stream)
                    .collect()
            };
            status.audio_tracks = tracks(StreamKind::AUDIO);
            status.subtitle_tracks = tracks(StreamKind::SUBTITLE);
            status.tracks = planned.plan.tracks;
            status.route = Some(planned.plan.route.clone());
        }
        status.how_to_play = session.how;
        status.how_to_play_choices = self.how_to_play_choices();
        status
    }

    /// For the playback information view.
    pub fn details(&self) -> Option<PlaybackDetails> {
        let session = self.session.as_ref()?;
        Some(PlaybackDetails {
            plan: session.planned.as_ref().map(|planned| planned.plan.clone()),
            notes: session
                .planned
                .as_ref()
                .map_or_else(|| session.notes.clone(), |planned| planned.notes.clone()),
            transcoding: session.facts.clone(),
        })
    }

    /// Plays `item` from `start` (a resume position or zero: the details screen decides, ADR 0018), on the engine already open. An
    /// item playing is stopped first.
    pub async fn play(&mut self, item: ItemRef, kind: MediaKind, start: Duration) {
        if self.session.is_some() {
            // Unloaded now, as a stop does: an end the old file reports while the new one is planned would read as the new one's.
            self.playing().stop();
            self.end_item(false).await;
        }
        let mut session = Session::new(
            item.clone(),
            self.quality.preferred,
            start,
            self.clock.now(),
        );
        session.how = self.how;
        self.session = Some(session);
        let permissions = self.server.permissions().await.unwrap_or_default();
        if let Some(session) = &mut self.session {
            session.permissions = permissions;
        }
        if let Err(problem) = self
            .plan_and_load(None, start, TrackChoice::default(), true, Vec::new(), true)
            .await
        {
            self.fail(problem).await;
            return;
        }
        // Asked after the engine has the file, so they do not delay the start.
        let has_segments = self.session.as_ref().is_some_and(|session| {
            session
                .planned
                .as_ref()
                .is_some_and(|planned| planned.plan.source.has_segments)
        });
        let segments = if has_segments {
            self.server.segments(&item).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let (next, autoplay) = if kind == MediaKind::EPISODE {
            let next = self.server.next_episode(&item).await.ok().flatten();
            let autoplay = match next {
                Some(_) => self
                    .server
                    .preferences()
                    .await
                    .is_ok_and(|preferences| preferences.next_episode_autoplay),
                None => false,
            };
            (next, autoplay)
        } else {
            (None, false)
        };
        if let Some(session) = &mut self.session {
            session.auto_skipped = vec![false; segments.len()];
            session.segments = segments;
            session.next = next;
            session.autoplay = autoplay;
        }
        self.publish(true);
    }

    /// Plays the next episode now, on the same engine.
    pub async fn play_next(&mut self) {
        let Some(next) = self
            .session
            .as_ref()
            .and_then(|session| session.next.clone())
        else {
            return;
        };
        // From its start: a resume point of the next episode is the details screen's choice, not autoplay's.
        self.play(next.item, next.kind, Duration::ZERO).await;
    }

    /// Hides the "Next episode" card and stops its countdown for this play.
    pub fn dismiss_next(&mut self) {
        if let Some(session) = &mut self.session {
            session.next_dismissed = true;
            session.next_shown = false;
            session.countdown_until = None;
        }
        self.publish(true);
    }

    pub async fn pause(&mut self) {
        self.set_paused(true).await;
    }

    pub async fn resume(&mut self) {
        self.set_paused(false).await;
    }

    pub async fn toggle(&mut self) {
        let paused = self.session.as_ref().is_some_and(|session| session.paused);
        self.set_paused(!paused).await;
    }

    async fn set_paused(&mut self, paused: bool) {
        if self.phase != Phase::Active {
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        if session.paused == paused {
            return;
        }
        session.paused = paused;
        session.last_progress = self.clock.now();
        self.playing().set_paused(paused);
        self.report_progress().await;
        self.publish(true);
    }

    pub async fn seek_to(&mut self, position: Duration) {
        if self.phase == Phase::Active {
            self.seek(position);
        }
    }

    /// Seeks by `millis`, back when negative.
    pub async fn seek_by(&mut self, millis: i64) {
        let Some(position) = self.session.as_ref().map(|session| session.position) else {
            return;
        };
        let offset = Duration::from_millis(millis.unsigned_abs());
        let target = if millis < 0 {
            position.saturating_sub(offset)
        } else {
            position + offset
        };
        self.seek_to(target).await;
    }

    /// Seeks past the segment under the position.
    pub async fn skip_segment(&mut self) {
        let end = self
            .session
            .as_ref()
            .and_then(|session| session.segment.map(|index| session.segments[index].end));
        if let Some(end) = end
            && self.phase == Phase::Active
        {
            self.seek(end);
        }
    }

    fn seek(&mut self, target: Duration) {
        let Some(session) = &mut self.session else {
            return;
        };
        let target = session
            .duration
            .map_or(target, |duration| target.min(duration));
        session.position = target;
        session.seeking = true;
        session.last_progress = self.clock.now();
        self.playing().seek_to(target);
        self.update_position();
        self.publish(true);
    }

    pub async fn stop(&mut self) {
        if self.session.is_none() {
            return;
        }
        // Silent at once: the stop report's answer can take seconds (seen through a gateway on a remux, 2026-10-26).
        self.playing().stop();
        self.end_item(false).await;
        self.session = None;
        self.phase = Phase::Idle;
        self.publish(true);
    }

    pub async fn select_audio(&mut self, index: u32) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(planned) = &mut session.planned else {
            return;
        };
        if planned.plan.tracks.audio == Some(index) || planned.plan.source.stream(index).is_none() {
            return;
        }
        let local = planned.plan.route.is_local();
        let tracks = TrackChoice {
            audio: Some(index),
            ..planned.plan.tracks
        };
        if local && self.route_fits(tracks) {
            if let Some(planned) = self.session.as_mut().and_then(|s| s.planned.as_mut()) {
                planned.plan.tracks.audio = Some(index);
            }
            self.playing().select_audio(EngineTrack::Stream(index));
            self.report_progress().await;
            self.publish(true);
        } else {
            // A server stream carries one audio stream: the server makes a new one at the position. A track the engine playing
            // cannot play moves the file to the next route that can, at the position (ADR 0025, "Track changes").
            self.replan(tracks).await;
        }
    }

    pub async fn select_subtitle(&mut self, choice: SubtitleChoice) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(planned) = &session.planned else {
            return;
        };
        let choice = planner::resolve_tracks(
            &planned.plan.source,
            TrackChoice {
                audio: None,
                subtitle: choice,
            },
        )
        .subtitle;
        let current = planned.plan.tracks.subtitle;
        if choice == current
            || matches!(choice, SubtitleChoice::Stream(index) if planned.plan.source.stream(index).is_none())
        {
            return;
        }
        let tracks = TrackChoice {
            subtitle: choice,
            ..planned.plan.tracks
        };
        if session.burned_in(choice) || session.burned_in(current) || !self.route_fits(tracks) {
            self.replan(tracks).await;
            return;
        }
        let fetched = session
            .subtitle_stream(choice)
            .is_some_and(|stream| session.fetches(stream));
        if fetched {
            if !self.add_subtitle(choice).await {
                return;
            }
        } else {
            self.playing().select_subtitle(engine_subtitle(choice));
        }
        if let Some(planned) = self.session.as_mut().and_then(|s| s.planned.as_mut()) {
            planned.plan.tracks.subtitle = choice;
        }
        self.report_progress().await;
        self.publish(true);
    }

    /// Fetches an external subtitle and adds it to the engine, or selects it when it was added before. False when the fetch failed.
    async fn add_subtitle(&mut self, choice: SubtitleChoice) -> bool {
        let SubtitleChoice::Stream(index) = choice else {
            return false;
        };
        let Some(session) = &self.session else {
            return false;
        };
        if session.added_subtitles.contains(&index) {
            self.playing().select_subtitle(EngineTrack::Stream(index));
            return true;
        }
        let Some(planned) = &session.planned else {
            return false;
        };
        let Some(stream) = planned.plan.source.stream(index) else {
            return false;
        };
        let Ok(data) = self
            .server
            .external_subtitle(&session.item, &planned.plan.source, stream)
            .await
        else {
            return false;
        };
        let subtitle = ExternalSubtitle {
            index,
            format: stream
                .codec
                .as_ref()
                .map(|codec| SubtitleFormat::new(codec.as_str().to_ascii_lowercase())),
            data,
            title: stream
                .display_title
                .clone()
                .or_else(|| stream.title.clone()),
            language: stream.language.clone(),
            select: true,
        };
        self.playing().add_subtitle(subtitle);
        if let Some(session) = &mut self.session {
            session.added_subtitles.push(index);
        }
        true
    }

    /// Sets the quality of the item playing, as picked (no snapping), and plays on at the position with it. Only this item keeps it:
    /// the next plays at the preferred quality again (owner, 2026-10-27).
    pub async fn set_quality(&mut self, quality: Quality) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.quality == quality {
            return;
        }
        session.quality = quality;
        if self.phase == Phase::Active {
            let tracks = self
                .session
                .as_ref()
                .map(Session::tracks)
                .unwrap_or_default();
            self.replan(tracks).await;
        } else {
            self.publish(true);
        }
    }

    /// On both engines, so a switch keeps it.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(100);
        self.engine.set_volume(self.volume);
        if let Some(native) = &mut self.native {
            native.engine.set_volume(self.volume);
        }
        self.publish(true);
    }

    /// On both engines, so a switch keeps it.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.engine.set_muted(muted);
        if let Some(native) = &mut self.native {
            native.engine.set_muted(muted);
        }
        self.publish(true);
    }

    /// The playback rate, 1 for normal. When the native engine plays and has said it ignores rates with these tracks (Windows' player
    /// with E-AC-3 Atmos, W1), a rate other than 1 moves the file to the app engine at the position, keeping the tracks (ADR 0025,
    /// "Track changes"; owner, 2026-10-28). No UI offers a speed yet: this is the core's part.
    ///
    /// A rate that is not finite or not above 0 is ignored; others are kept within mpv's `<0.01-100>` (`current-mpv`'s
    /// `speed_value`). Both engines get it, as volume, so neither keeps an old rate into the next file (mpv's `speed` outlives a
    /// `loadfile`).
    pub async fn set_speed(&mut self, speed: f64) {
        if !speed.is_finite() || speed <= 0.0 {
            return;
        }
        let speed = speed.clamp(0.01, 100.0);
        self.speed = speed;
        self.engine.set_speed(speed);
        if let Some(native) = &mut self.native {
            native.engine.set_speed(speed);
        }
        self.check_speed().await;
    }

    async fn check_speed(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        if self.speed == 1.0
            || session.rates_supported
            || session.loaded_on != Some(EngineRole::Native)
            || !matches!(self.phase, Phase::Active | Phase::Loading)
        {
            return;
        }
        session.speed_not_supported = true;
        let tracks = session.tracks();
        self.replan(tracks).await;
    }

    /// The display the player's window is on (departure 8, from the UI's display facts). A change that turns departure 8 for the
    /// item playing re-plans at the position (ADR 0025, "Track changes"); with the app engine only it changes nothing.
    pub async fn set_display(&mut self, display: DisplayKind) {
        let before = self.display;
        self.display = display;
        if self.native.is_none() || before == display {
            return;
        }
        if !matches!(self.phase, Phase::Active | Phase::Loading) {
            return;
        }
        let Some(planned) = self.session.as_ref().and_then(|s| s.planned.as_ref()) else {
            return;
        };
        let engines = self.engines();
        let source = &planned.plan.source;
        let was = route::hdr_on_sdr(source, &engines, before);
        let now = route::hdr_on_sdr(source, &engines, display);
        if was == now {
            return;
        }
        let tracks = planned.plan.tracks;
        let current = planned.plan.route.clone();
        // "How to play" other than Auto: never back up the order, only down when the route playing no longer plays here.
        if self.route_context().how != HowToPlay::Auto {
            if !self.route_fits(tracks) {
                self.replan(tracks).await;
            }
            return;
        }
        // Only when the route the check now puts first is not the one playing (after a fallback it may be the same); a server
        // stream moves between engines the same way.
        let first = self
            .candidates(tracks, self.route_context())
            .into_iter()
            .next();
        if first.is_some_and(|first| !same_route(&first, &current)) {
            self.replan(tracks).await;
        }
    }

    /// "Play with app engine" for this playback: "How to play" set to the app engine in the player.
    pub async fn play_with_app_engine(&mut self) {
        self.choose_how_to_play(HowToPlay::AppEngine).await;
    }

    /// "How to play" chosen in the player for this playback only (owner, 2026-10-31); the next item uses Settings' again. A new
    /// choice re-plans at the position at once, Auto included (manager, 2026-10-31), unless the route playing is already the one the
    /// check puts first.
    pub async fn choose_how_to_play(&mut self, how: HowToPlay) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.chosen_here && session.how == how {
            return;
        }
        session.how = how;
        session.chosen_here = true;
        if !matches!(self.phase, Phase::Active | Phase::Loading) {
            return;
        }
        let tracks = session.tracks();
        let current = session
            .planned
            .as_ref()
            .map(|planned| planned.plan.route.clone());
        let stays = !how.is_server()
            && current.as_ref().is_some_and(|current| {
                self.candidates(tracks, self.route_context())
                    .first()
                    .is_some_and(|first| same_route(first, current))
            });
        if stays {
            // The reasons now say the route was chosen.
            let reason = self.route_context().chosen_reason();
            if let Some(planned) = self.session.as_mut().and_then(|s| s.planned.as_mut())
                && let Some(reason) = reason
            {
                planned.plan.reasons.retain(|known| {
                    *known != RouteReason::SETTING_CHOSE_ROUTE
                        && *known != RouteReason::USER_CHOSE_ROUTE
                });
                planned.plan.reasons.insert(0, reason);
            }
            self.publish(true);
            return;
        }
        self.replan(tracks).await;
    }

    /// The choices "How to play" offers in the player for the item playing: Auto, each local step whose route the check puts first
    /// for this file with its tracks (so a step that would fall below itself is not offered; the remux only where the platform has
    /// one), and the server's steps the server's permissions allow. Empty while nothing plays.
    pub fn how_to_play_choices(&self) -> Vec<HowToPlay> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let Some(planned) = &session.planned else {
            return Vec::new();
        };
        let engines = self.engines();
        let tracks = planned.plan.tracks;
        let base = self.route_context();
        let mut choices = vec![HowToPlay::Auto];
        for how in [
            HowToPlay::Native,
            HowToPlay::DeviceRemux,
            HowToPlay::AppEngine,
        ] {
            let local = engines.native.is_some() || how == HowToPlay::AppEngine;
            if !local || (how == HowToPlay::DeviceRemux && engines.remux.is_none()) {
                continue;
            }
            let context = RouteContext {
                how,
                speed_not_supported: false,
                ..base
            };
            let first = route::local_routes(&planned.plan.source, tracks, &engines, context, &[])
                .into_iter()
                .next();
            if first.is_some_and(|first| Some(first.route.step()) == how.step()) {
                choices.push(how);
            }
        }
        if session.permissions.remuxing {
            choices.push(HowToPlay::ServerRemux);
        }
        if session.permissions.video_transcoding {
            choices.push(HowToPlay::ServerTranscode);
        }
        choices
    }

    /// An event of the app engine.
    pub async fn on_engine_event(&mut self, event: EngineEvent) {
        self.on_event(EngineRole::App, event).await;
    }

    /// An event of the native engine.
    pub async fn on_native_engine_event(&mut self, event: EngineEvent) {
        self.on_event(EngineRole::Native, event).await;
    }

    async fn on_event(&mut self, role: EngineRole, event: EngineEvent) {
        // Only the engine that has the file speaks for it; the other one's ends and errors are of a file it no longer plays.
        if role != self.role() {
            return;
        }
        let now = self.clock.now();
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
            EngineEvent::TrackFailed(_) if session.refusal_pending => {}
            EngineEvent::TrackFailed(kind) => {
                if matches!(self.phase, Phase::Active | Phase::Loading) {
                    self.engine_failed(kind).await;
                }
            }
            EngineEvent::RatesSupported(supported) => {
                session.rates_supported = supported;
                self.check_speed().await;
            }
            EngineEvent::Loaded => {
                session.last_progress = now;
                if self.phase == Phase::Loading {
                    let subtitle = session.tracks().subtitle;
                    if session
                        .subtitle_stream(subtitle)
                        .is_some_and(|stream| session.fetches(stream))
                    {
                        self.add_subtitle(subtitle).await;
                    }
                }
            }
            EngineEvent::Restarted => {
                session.last_progress = now;
                // The engine read the stream again.
                session.stream_lost = false;
                if self.phase == Phase::Loading {
                    self.phase = Phase::Active;
                    session.seeking = false;
                    if !session.started {
                        self.report_start().await;
                    }
                    self.read_transcoding_facts().await;
                    self.update_position();
                    self.publish(true);
                } else if self.phase == Phase::Active && session.seeking {
                    session.seeking = false;
                    self.report_progress().await;
                    self.update_position();
                    self.publish(true);
                }
            }
            EngineEvent::Position(position) => {
                if matches!(self.phase, Phase::Active | Phase::Loading) {
                    if session.position != position {
                        session.last_progress = now;
                    }
                    session.position = position;
                    let changed = self.update_position();
                    self.publish(changed);
                }
            }
            EngineEvent::Duration(duration) => {
                if session.duration != Some(duration) {
                    session.duration = Some(duration);
                    self.update_position();
                    self.publish(true);
                }
            }
            EngineEvent::Buffered(buffered) => {
                if session.buffered != Some(buffered) {
                    session.last_progress = now;
                }
                // More data ahead than before: the stream reads again.
                if session.buffered.is_some_and(|before| buffered > before) {
                    session.stream_lost = false;
                }
                session.buffered = Some(buffered);
                self.publish(false);
            }
            EngineEvent::Paused(paused) => {
                if self.phase == Phase::Active && session.paused != paused {
                    session.paused = paused;
                    session.last_progress = now;
                    self.report_progress().await;
                    self.publish(true);
                }
            }
            EngineEvent::Buffering(buffering) => {
                if session.buffering && !buffering {
                    session.stream_lost = false;
                }
                if session.buffering != buffering {
                    session.buffering = buffering;
                    session.last_progress = now;
                    self.publish(true);
                }
            }
            // While a refusal is with the 401 rule, its answer decides ([`Controller::stream_failed`]).
            EngineEvent::Ended(EndReason::Eof | EndReason::Error(_)) if session.refusal_pending => {
            }
            EngineEvent::Ended(EndReason::Eof) => {
                if matches!(self.phase, Phase::Active | Phase::Loading) {
                    if session.stream_lost && !session.at_end() {
                        // The connection's failure, not the end of the file: not marked played, no autoplay.
                        self.playing().stop();
                        self.fail(PlaybackProblem::Network).await;
                    } else {
                        self.end_of_file().await;
                    }
                }
            }
            EngineEvent::Ended(EndReason::Error(kind)) => {
                if matches!(self.phase, Phase::Active | Phase::Loading) {
                    self.engine_failed(kind).await;
                }
            }
            // Stops and replaced files are the controller's own commands.
            EngineEvent::Ended(EndReason::Stopped) => {}
        }
    }

    /// The relay saw the server or a gateway refuse the stream, and the composition root took the refusal to the 401 rule, whose
    /// answer comes with [`Controller::stream_failed`]. Until then the engine's error or end about the refused request is held, and
    /// the stall rule counts from now, so the user sees the rule's answer ("Sign in to the gateway again"), not an engine error, an
    /// end, or a fallback. Only while a file is loading or playing; a new plan clears it.
    pub fn stream_refused(&mut self) {
        if !matches!(self.phase, Phase::Active | Phase::Loading) {
            return;
        }
        let now = self.clock.now();
        if let Some(session) = &mut self.session {
            session.refusal_pending = true;
            session.last_progress = now;
        }
    }

    /// The stream failed outside the engine: the loopback relay saw the server refuse it, not find it, fail, become unreachable, or
    /// send a playlist it refused (ADR 0020, "Lifecycle"), and the composition root made that a problem (the 401 rule for a
    /// refusal). Playback fails with it, reported stopped with `Failed = true`, with no fallback: another route would meet the same
    /// server. Only while a file is loading or playing; the root drops events of a stream that was replaced.
    ///
    /// [`PlaybackProblem::Network`] while the engine plays on from its buffer (not loading, seeking or buffering) does not stop it:
    /// an idle connection of a paused player that the server closed is opened again at the next read. If the engine then fails or
    /// stalls, playback fails with `Network`.
    pub async fn stream_failed(&mut self, problem: PlaybackProblem) {
        if !matches!(self.phase, Phase::Active | Phase::Loading) {
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        let waiting = self.phase == Phase::Loading || session.seeking || session.buffering;
        if problem == PlaybackProblem::Network && !waiting && !session.refusal_pending {
            session.stream_lost = true;
            return;
        }
        self.playing().stop();
        self.fail(problem).await;
    }

    /// Does what is due: the stall rule, the periodic progress report, a held-back state, the autoplay countdown.
    pub async fn tick(&mut self) {
        let now = self.clock.now();
        if self
            .stall_deadline()
            .is_some_and(|deadline| now >= deadline)
        {
            self.stall().await;
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        if let Some(until) = session.countdown_until {
            if now >= until {
                self.play_next().await;
                return;
            }
            if self.countdown(session) != self.published_countdown {
                self.publish(true);
            }
        }
        if self.phase == Phase::Active
            && self.session.as_ref().is_some_and(|session| {
                session.started && now >= session.last_report + PROGRESS_INTERVAL
            })
        {
            self.report_progress().await;
        }
        if self.publish_pending {
            self.publish(false);
        }
    }

    /// When [`Controller::tick`] has something to do next, by the clock.
    pub fn next_deadline(&self) -> Option<Duration> {
        let session = self.session.as_ref()?;
        let mut deadlines: Vec<_> = self.stall_deadline().into_iter().collect();
        if self.phase == Phase::Active && session.started {
            deadlines.push(session.last_report + PROGRESS_INTERVAL);
        }
        if self.publish_pending
            && let Some(last) = self.last_published
        {
            deadlines.push(last + STATE_INTERVAL);
        }
        if let Some(until) = session.countdown_until {
            deadlines.push(until);
            if let Some(left) = self.countdown(session) {
                // When the shown seconds change.
                deadlines.push(until.saturating_sub(Duration::from_secs(u64::from(left) - 1)));
            }
        }
        deadlines.into_iter().min()
    }

    /// When the stall rule fails playback, while the player waits on the engine: loading, seeking, or buffering while not paused (a
    /// paused player is not expected to move).
    fn stall_deadline(&self) -> Option<Duration> {
        let session = self.session.as_ref()?;
        let waiting = match self.phase {
            Phase::Loading => true,
            Phase::Active => session.seeking || (session.buffering && !session.paused),
            _ => false,
        };
        waiting.then(|| session.last_progress + STALL_TIMEOUT)
    }

    /// The engine made no progress for [`STALL_TIMEOUT`]: it is stopped and playback fails, reported stopped with `Failed = true`.
    /// Nothing is retried by itself; playing the item again resumes at the reported position.
    async fn stall(&mut self) {
        self.playing().stop();
        self.fail(PlaybackProblem::Network).await;
    }

    /// The item's quality in use, else the preferred one.
    fn current_quality(&self) -> Quality {
        self.session
            .as_ref()
            .map_or(self.quality.preferred, |session| session.quality)
    }

    fn countdown(&self, session: &Session) -> Option<u32> {
        let left = session.countdown_until?.saturating_sub(self.clock.now());
        Some(
            u32::try_from(left.as_millis().div_ceil(1000))
                .unwrap_or(u32::MAX)
                .max(1),
        )
    }

    /// Asks the server for a plan for the session's item at its quality and loads it ([`Controller::plan_and_load_once`]). With a
    /// native engine, an engine that refuses the file at once is a failed route like any other: the next one is tried (ADR 0025,
    /// "Fallback on a failed start"). On a problem the caller fails.
    async fn plan_and_load(
        &mut self,
        mut media_source: Option<String>,
        start: Duration,
        tracks: TrackChoice,
        mut direct_play: bool,
        mut own: Vec<RouteReason>,
        mut snap: bool,
    ) -> Result<(), PlaybackProblem> {
        loop {
            let refused = match self
                .plan_and_load_once(media_source.clone(), start, tracks, direct_play, own, snap)
                .await
            {
                Ok(()) => return Ok(()),
                Err(LoadFailure::Problem(problem)) => return Err(problem),
                Err(LoadFailure::Refused(kind)) => kind,
            };
            let Some(next_direct_play) = self.move_on() else {
                return Err(PlaybackProblem::Engine { kind: refused });
            };
            direct_play = next_direct_play;
            own = vec![RouteReason::FALLBACK_AFTER_ENGINE_ERROR];
            snap = false;
            media_source = self
                .session
                .as_ref()
                .and_then(|session| session.planned.as_ref())
                .map(|planned| planned.plan.source.id.clone());
        }
    }

    /// The route playing failed: marks it failed and says whether the next plan may be direct play, or `None` when no route is left
    /// (ADR 0025's chain). A local route leads on to the next one or the server; a server stream on the native engine to the app
    /// engine's, which takes every stream; the app engine's server stream is the last.
    fn move_on(&mut self) -> Option<bool> {
        let app_only = self.native.is_none();
        let session = self.session.as_mut()?;
        let route = session.planned.as_ref()?.plan.route.clone();
        let next = !session.failed_routes.contains(&route)
            && (route.is_local() || route.engine() == EngineRole::Native);
        if !next {
            return None;
        }
        session.fallback_used = true;
        session.failed_routes.push(route.clone());
        Some(route.is_local() && !app_only)
    }

    /// One plan and load.
    ///
    /// `snap`: the session's quality is the preferred one, which becomes the entry of the planned version's quality menu that is not
    /// above it ([`snap_quality`]); the version's bitrate is known only from the server's answer. When that entry has the server send
    /// something else ([`Quality::sends_same`]), the plan is asked for once more, for the same version, at the entry; else the plan
    /// stands. Either way the engine loads once.
    async fn plan_and_load_once(
        &mut self,
        mut media_source: Option<String>,
        start: Duration,
        tracks: TrackChoice,
        direct_play: bool,
        own: Vec<RouteReason>,
        mut snap: bool,
    ) -> Result<(), LoadFailure> {
        self.phase = Phase::Preparing;
        self.publish(true);
        let Some(session) = &self.session else {
            return Ok(());
        };
        let item = session.item.clone();
        let mut quality = session.quality;
        let local = self.server.is_local_address();
        // With the app engine only, its capabilities are the device profile as before ADR 0025; with a native engine, the union.
        let engines = self.engines();
        let context = self.route_context();
        let capabilities = engines.device_profile();
        // "How to play" at a server step asks for the server's stream from the start, and its transcode for the video.
        let mut direct_play = direct_play && !context.how.is_server();
        let planned = loop {
            let mut request =
                planner::request(media_source.clone(), start, tracks, quality, direct_play);
            request.force_video_transcode = context.how == HowToPlay::ServerTranscode;
            let answer = self
                .server
                .playback_info(&item, &request, &capabilities)
                .await;
            let Some(session) = &mut self.session else {
                return Ok(());
            };
            let answer = match answer {
                Ok(answer) => answer,
                Err(problem) => {
                    if matches!(problem, PlaybackProblem::NotPlayable { .. }) {
                        session.notes = planner::not_playable_notes(&session.permissions);
                    }
                    session.planned = None;
                    return Err(problem.into());
                }
            };
            let server_reasons = answer.reasons.clone();
            let inputs = RouteInputs {
                engines: &engines,
                context,
                tried: &session.failed_routes,
            };
            let Some(planned) = planner::plan(
                &item,
                &request,
                answer,
                &own,
                &session.permissions,
                local,
                inputs,
            ) else {
                if direct_play {
                    // Direct play, but no local route plays the file: the server's stream (ADR 0025, step 4).
                    direct_play = false;
                    continue;
                }
                session.planned = None;
                return Err(PlaybackProblem::NotPlayable {
                    reasons: server_reasons,
                }
                .into());
            };
            if !snap {
                break planned;
            }
            snap = false;
            let source = &planned.plan.source;
            let snapped = snap_quality(quality, Some(source), self.quality.all);
            session.quality = snapped;
            if snapped.sends_same(quality, source.bitrate) {
                break planned;
            }
            quality = snapped;
            media_source = Some(source.id.clone());
        };
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let hls = matches!(
            planned.plan.route,
            Route::ServerStream {
                protocol: StreamProtocol::Hls,
                ..
            }
        );
        // A stream for the native engine always goes through the relay (ADR 0025, "URLs across the bridge").
        let always_relay = planned.plan.route.engine() == EngineRole::Native;
        let stream = match self
            .server
            .stream_url(&item, &planned.url, hls, always_relay)
        {
            Ok(stream) => stream,
            // The relay could not start for the native engine, which may only get relayed streams: a failed route, so libmpv (which
            // can be given the server's own URL on an address without a gateway) or the server is tried next.
            Err(PlaybackProblem::LocalRelay) if always_relay => {
                session.planned = Some(planned);
                return Err(LoadFailure::Refused(
                    current_model::EngineErrorKind::LOADING_FAILED,
                ));
            }
            Err(problem) => {
                session.planned = None;
                return Err(problem.into());
            }
        };
        // The relay rewrites HLS playlists only: any other relayed file opens no reference (ADR 0020).
        let file = EngineFile {
            references: hls || !stream.relayed,
            hls,
            url: stream.url,
            auth: stream.auth,
            start,
            audio: if planned.plan.route.is_local() {
                planned
                    .plan
                    .tracks
                    .audio
                    .map_or(EngineTrack::Auto, EngineTrack::Stream)
            } else {
                EngineTrack::Auto
            },
            subtitle: EngineTrack::Off,
        };
        session.planned = Some(planned);
        session.position = start;
        session.last_progress = self.clock.now();
        session.stream_lost = false;
        session.refusal_pending = false;
        session.started = false;
        session.stop_reported = false;
        // A re-plan while paused (a track or quality change, a fallback, a move to the other engine) stays paused: the state, the
        // start report and the engine. libmpv's `pause` outlives a `loadfile` (mpv v0.41.0 `DOCS/man/options.rst`,
        // `--reset-on-next-file`: "Default: Do not reset anything", read 2026-10-29), so the picture stayed paused while the state said
        // Playing; the engine is told again for one that does not keep it.
        let keep_paused = session.paused;
        session.paused = false;
        session.buffering = false;
        session.seeking = false;
        session.facts = None;
        session.added_subtitles.clear();
        // Embedded or streamed subtitles start with the file; fetched ones are added once it is open, burned-in ones are in the video.
        let subtitle = session.tracks().subtitle;
        let subtitle = match session.subtitle_stream(subtitle) {
            Some(stream) if session.fetches(stream) || session.burned_in(subtitle) => {
                EngineTrack::Off
            }
            _ => engine_subtitle(subtitle),
        };
        let role = session
            .planned
            .as_ref()
            .map_or(EngineRole::App, |planned| planned.plan.route.engine());
        let before = session.loaded_on.replace(role);
        if before.is_some_and(|before| before != role) {
            // What the other engine said of rates was about its own playback; this engine reports after the load
            // ([`EngineEvent::RatesSupported`]). A reload on the same engine keeps the last report until the next.
            session.rates_supported = true;
        }
        if keep_paused {
            session.paused = true;
        }
        if let Some(before) = before.filter(|before| *before != role) {
            // The file moves to the other engine: the one that had it lets it go.
            pick(&mut self.engine, &mut self.native, before).stop();
        }
        let two_engines = self.native.is_some();
        let engine = pick(&mut self.engine, &mut self.native, role);
        if let Err(kind) = engine.load(EngineFile { subtitle, ..file }) {
            return Err(if two_engines {
                LoadFailure::Refused(kind)
            } else {
                LoadFailure::Problem(PlaybackProblem::Engine { kind })
            });
        }
        // With two engines every load tells the engine the session's pause, either way: the file can come back to an engine that
        // still holds the pause of its last file (libmpv keeps `pause` across a `loadfile`, above) after a resume sent to the other
        // engine. With libmpv alone only a pause is sent, as before.
        if two_engines || keep_paused {
            engine.set_paused(keep_paused);
        }
        self.phase = Phase::Loading;
        self.publish(true);
        Ok(())
    }

    /// Plays on at the position with other tracks or another quality: a new play session, the old one reported stopped.
    async fn replan(&mut self, tracks: TrackChoice) {
        let Some(session) = &self.session else {
            return;
        };
        let start = session.position;
        let fallback = session.fallback_used;
        let media_source = session
            .planned
            .as_ref()
            .map(|planned| planned.plan.source.id.clone());
        self.report_stopped(false).await;
        // The fallback's reason belongs to the re-plan the failure caused; with the app engine only every later one keeps it, as
        // before ADR 0025 (direct play stays off).
        let own = if fallback && self.native.is_none() {
            vec![RouteReason::FALLBACK_AFTER_ENGINE_ERROR]
        } else {
            Vec::new()
        };
        // With a native engine the routes that failed are left out by the check; with the app engine only, direct play stays off
        // after a fallback (ADR 0019 choice 9).
        let direct_play = !fallback || self.native.is_some();
        if let Err(problem) = self
            .plan_and_load(media_source, start, tracks, direct_play, own, false)
            .await
        {
            self.fail(problem).await;
        }
    }

    /// An engine failure (an error, or a selected track it cannot play): the next route at the position with the same tracks, each
    /// route at most once per play, ending with the server stream (ADR 0025, "Fallback on a failed start"; ADR 0019 choice 9
    /// widened). With the app engine only that is choice 9 as written: one re-plan without direct play. Otherwise playback fails.
    async fn engine_failed(&mut self, kind: current_model::EngineErrorKind) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.stream_lost {
            self.fail(PlaybackProblem::Network).await;
            return;
        }
        let Some(direct_play) = self.move_on() else {
            self.fail(PlaybackProblem::Engine { kind }).await;
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let start = session.position;
        let tracks = session.tracks();
        let media_source = session
            .planned
            .as_ref()
            .map(|planned| planned.plan.source.id.clone());
        self.report_stopped(false).await;
        if let Err(problem) = self
            .plan_and_load(
                media_source,
                start,
                tracks,
                direct_play,
                vec![RouteReason::FALLBACK_AFTER_ENGINE_ERROR],
                false,
            )
            .await
        {
            self.fail(problem).await;
        }
    }

    async fn fail(&mut self, problem: PlaybackProblem) {
        // With two engines, the one that has the file lets it go (a failed re-plan after a native track failure leaves it loaded),
        // and the platform engine's gate closes. With libmpv alone, as before ADR 0025.
        if self.native.is_some() {
            self.playing().stop();
        }
        self.end_item(true).await;
        self.phase = Phase::Failed(problem);
        self.publish(true);
    }

    async fn end_of_file(&mut self) {
        let now = self.clock.now();
        if let Some(session) = &mut self.session {
            if let Some(duration) = session.duration {
                session.position = duration;
            }
            session.seeking = false;
            session.buffering = false;
            if session.next.is_some() && session.autoplay && !session.next_dismissed {
                session.countdown_until = Some(now + AUTOPLAY_COUNTDOWN);
            }
        }
        self.end_item(false).await;
        self.phase = Phase::Ended;
        self.update_position();
        self.publish(true);
    }

    /// Ends the stream (its relay route) and tells the server and the rest of the app that the item stopped, once.
    async fn end_item(&mut self, failed: bool) {
        self.server.end_stream();
        self.report_stopped(failed).await;
        let Some(session) = &mut self.session else {
            return;
        };
        if !session.item_stopped {
            session.item_stopped = true;
            self.events.push(PlayerEvent::Stopped {
                item: session.item.clone(),
                position: session.position,
                failed,
            });
        }
    }

    /// Updates the segment under the position, skips it when the setting says so, and shows the "Next episode" card. True when
    /// something the UI shows changed.
    fn update_position(&mut self) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let position = session.position;
        let segment = session
            .segments
            .iter()
            .position(|segment| segment.contains(position));
        let mut changed = segment != session.segment;
        session.segment = segment;
        let next_shown = session.next.is_some()
            && !session.next_dismissed
            && session
                .next_threshold()
                .is_some_and(|threshold| position >= threshold);
        changed |= next_shown != session.next_shown;
        session.next_shown = next_shown;
        if let Some(index) = segment
            && self.phase == Phase::Active
            && !session.auto_skipped[index]
            && self.auto_skip.skips(&session.segments[index].kind)
        {
            // Once per segment and play: a seek back into it does not skip it again (choice 8).
            session.auto_skipped[index] = true;
            let end = session.segments[index].end;
            self.seek(end);
            return true;
        }
        changed
    }

    async fn read_transcoding_facts(&mut self) {
        if self.session.as_ref().is_none_or(Session::is_local) {
            return;
        }
        let Ok(Some(facts)) = self.server.transcoding_facts().await else {
            return;
        };
        if let Some(planned) = self.session.as_mut().and_then(|s| s.planned.as_mut()) {
            planned.plan.route = planner::correct(&planned.plan.route, &facts);
        }
        if let Some(session) = &mut self.session {
            session.facts = Some(facts);
        }
    }

    fn progress(&self) -> Option<ProgressReport> {
        let session = self.session.as_ref()?;
        let planned = session.planned.as_ref()?;
        Some(ProgressReport {
            item: session.item.clone(),
            media_source: planned.plan.source.id.clone(),
            play_session_id: planned.plan.play_session_id.clone(),
            position: session.position,
            audio: planned.plan.tracks.audio,
            subtitle: match planned.plan.tracks.subtitle {
                SubtitleChoice::Stream(index) => Some(index),
                _ => None,
            },
            paused: session.paused,
            muted: self.muted,
            volume: self.volume,
            can_seek: session.duration.is_some(),
            route: planned.plan.route.clone(),
        })
    }

    async fn report_start(&mut self) {
        let Some(report) = self.progress() else {
            return;
        };
        // Reports are best effort: a lost one is followed by the next.
        let _ = self.server.report_start(&report).await;
        let now = self.clock.now();
        if let Some(session) = &mut self.session {
            session.started = true;
            session.last_report = now;
        }
    }

    /// A progress report now; the next periodic one follows 10 s later.
    async fn report_progress(&mut self) {
        if !self.session.as_ref().is_some_and(|session| session.started) {
            return;
        }
        let Some(report) = self.progress() else {
            return;
        };
        let _ = self.server.report_progress(&report).await;
        let now = self.clock.now();
        if let Some(session) = &mut self.session {
            session.last_report = now;
        }
    }

    /// Reports the play session stopped, once per play session that has one.
    async fn report_stopped(&mut self, failed: bool) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(planned) = &session.planned else {
            return;
        };
        if session.stop_reported {
            return;
        }
        session.stop_reported = true;
        let report = StopReport {
            item: session.item.clone(),
            media_source: planned.plan.source.id.clone(),
            play_session_id: planned.plan.play_session_id.clone(),
            position: session.position,
            failed,
        };
        let _ = self.server.report_stopped(&report).await;
    }

    /// Queues the state: at once when `changed` (anything but position and buffer) or after [`STATE_INTERVAL`] since the last one,
    /// else held back for [`Controller::tick`].
    fn publish(&mut self, changed: bool) {
        let now = self.clock.now();
        let due = self
            .last_published
            .is_none_or(|last| now >= last + STATE_INTERVAL);
        if !changed && !due {
            self.publish_pending = true;
            return;
        }
        let status = self.status();
        self.published_countdown = status.next.as_ref().and_then(|next| next.countdown);
        self.events.push(PlayerEvent::State(Box::new(status)));
        self.last_published = Some(now);
        self.publish_pending = false;
    }
}

fn engine_subtitle(choice: SubtitleChoice) -> EngineTrack {
    match choice {
        SubtitleChoice::Stream(index) => EngineTrack::Stream(index),
        SubtitleChoice::Off => EngineTrack::Off,
        SubtitleChoice::Default => EngineTrack::Auto,
    }
}
