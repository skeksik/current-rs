//! The playback controller (ADR 0019, "Playback controller"): one per player, a state machine driven by user actions, engine events
//! and a clock.
//!
//! The composition root owns it and calls it from one task: actions as the user acts, [`Controller::on_engine_event`] for each
//! engine event, and [`Controller::tick`] at [`Controller::next_deadline`]. It reports to the server, drives the engine and queues
//! [`PlayerEvent`]s, which the root drains with [`Controller::take_events`] and delivers to listeners. A future dropped part way (a
//! cancelled action) leaves the controller in a state the next action can leave.

use std::time::Duration;

use current_model::{
    ItemRef, ItemSummary, MediaKind, MediaStream, PlaybackProblem, PlayerState, Route, RouteReason,
    Segment, SegmentKind, StreamKind, StreamProtocol, SubtitleChoice, SubtitleDelivery,
    SubtitleFormat, TrackChoice, TranscodingFacts,
};

use crate::clock::Clock;
use crate::engine::{EndReason, Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};
use crate::planner::{self, Planned};
use crate::quality::{Quality, QualityMemory};
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
    fallback_used: bool,
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
    fn new(item: ItemRef, start: Duration, now: Duration) -> Self {
        Self {
            item,
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

    fn is_direct_play(&self) -> bool {
        self.planned
            .as_ref()
            .is_some_and(|planned| planned.plan.route.is_direct_play())
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
        if self.is_direct_play() {
            stream.is_external
        } else {
            stream.subtitle_delivery.as_ref() == Some(&SubtitleDelivery::EXTERNAL)
        }
    }

    /// Whether the server burns this subtitle into a server stream's video.
    fn burned_in(&self, choice: SubtitleChoice) -> bool {
        !self.is_direct_play()
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

/// The playback controller. `E` is the player's one engine, kept for every file it plays.
pub struct Controller<E, S, C> {
    engine: E,
    server: S,
    clock: C,
    quality: QualityMemory,
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
    /// A controller with the remembered quality and the auto-skip setting, both loaded by the composition root.
    pub fn new(
        engine: E,
        server: S,
        clock: C,
        quality: QualityMemory,
        auto_skip: AutoSkip,
    ) -> Self {
        Self {
            engine,
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

    pub fn server(&self) -> &S {
        &self.server
    }

    /// The events since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<PlayerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn quality_memory(&self) -> QualityMemory {
        self.quality
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
            self.end_item(false).await;
            // Unloaded now, as a stop does: an end the old file reports while the new one is planned would read as the new one's.
            self.engine.stop();
        }
        self.session = Some(Session::new(item.clone(), start, self.clock.now()));
        let permissions = self.server.permissions().await.unwrap_or_default();
        if let Some(session) = &mut self.session {
            session.permissions = permissions;
        }
        if let Err(problem) = self
            .plan_and_load(None, start, TrackChoice::default(), true, Vec::new())
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
        self.engine.set_paused(paused);
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
        self.engine.seek_to(target);
        self.update_position();
        self.publish(true);
    }

    pub async fn stop(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.end_item(false).await;
        self.engine.stop();
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
        if planned.plan.route.is_direct_play() {
            planned.plan.tracks.audio = Some(index);
            self.engine.select_audio(EngineTrack::Stream(index));
            self.report_progress().await;
            self.publish(true);
        } else {
            // A server stream carries one audio stream: the server makes a new one at the position.
            let tracks = TrackChoice {
                audio: Some(index),
                ..planned.plan.tracks
            };
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
        if session.burned_in(choice) || session.burned_in(current) {
            let tracks = TrackChoice {
                subtitle: choice,
                ..planned.plan.tracks
            };
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
            self.engine.select_subtitle(engine_subtitle(choice));
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
            self.engine.select_subtitle(EngineTrack::Stream(index));
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
        self.engine.add_subtitle(ExternalSubtitle {
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
        });
        if let Some(session) = &mut self.session {
            session.added_subtitles.push(index);
        }
        true
    }

    /// Sets the quality for the kind of address in use, remembers it, and plays on at the position with it.
    pub async fn set_quality(&mut self, quality: Quality) {
        let local = self.server.is_local_address();
        if self.quality.for_address(local) == quality {
            return;
        }
        self.quality.set(local, quality);
        self.events
            .push(PlayerEvent::QualityChanged { local, quality });
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

    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(100);
        self.engine.set_volume(self.volume);
        self.publish(true);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.engine.set_muted(muted);
        self.publish(true);
    }

    pub async fn on_engine_event(&mut self, event: EngineEvent) {
        let now = self.clock.now();
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
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
                        self.engine.stop();
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
        self.engine.stop();
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
        self.engine.stop();
        self.fail(PlaybackProblem::Network).await;
    }

    fn current_quality(&self) -> Quality {
        self.quality.for_address(self.server.is_local_address())
    }

    fn countdown(&self, session: &Session) -> Option<u32> {
        let left = session.countdown_until?.saturating_sub(self.clock.now());
        Some(
            u32::try_from(left.as_millis().div_ceil(1000))
                .unwrap_or(u32::MAX)
                .max(1),
        )
    }

    /// Asks the server for a plan for the session's item and loads it. On a problem the caller fails.
    async fn plan_and_load(
        &mut self,
        media_source: Option<String>,
        start: Duration,
        tracks: TrackChoice,
        direct_play: bool,
        own: Vec<RouteReason>,
    ) -> Result<(), PlaybackProblem> {
        self.phase = Phase::Preparing;
        self.publish(true);
        let Some(session) = &self.session else {
            return Ok(());
        };
        let item = session.item.clone();
        let local = self.server.is_local_address();
        let request = planner::request(
            media_source,
            start,
            tracks,
            self.quality.for_address(local),
            direct_play,
        );
        let capabilities = self.engine.capabilities();
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
                return Err(problem);
            }
        };
        let planned = planner::plan(&item, &request, answer, &own, &session.permissions, local);
        let hls = matches!(
            planned.plan.route,
            Route::ServerStream {
                protocol: StreamProtocol::Hls,
                ..
            }
        );
        let stream = match self.server.stream_url(&item, &planned.url, hls) {
            Ok(stream) => stream,
            Err(problem) => {
                session.planned = None;
                return Err(problem);
            }
        };
        // The relay rewrites HLS playlists only: any other relayed file opens no reference (ADR 0020).
        let file = EngineFile {
            references: hls || !stream.relayed,
            hls,
            url: stream.url,
            auth: stream.auth,
            start,
            audio: if planned.plan.route.is_direct_play() {
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
        self.engine
            .load(EngineFile { subtitle, ..file })
            .map_err(|kind| PlaybackProblem::Engine { kind })?;
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
        let own = if fallback {
            vec![RouteReason::FALLBACK_AFTER_ENGINE_ERROR]
        } else {
            Vec::new()
        };
        if let Err(problem) = self
            .plan_and_load(media_source, start, tracks, !fallback, own)
            .await
        {
            self.fail(problem).await;
        }
    }

    /// An engine failure: on direct play, once per item, the plan is made again without direct play at the position with the same
    /// tracks (ADR 0019, choice 9); otherwise playback fails.
    async fn engine_failed(&mut self, kind: current_model::EngineErrorKind) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.stream_lost {
            self.fail(PlaybackProblem::Network).await;
            return;
        }
        if !session.is_direct_play() || session.fallback_used {
            self.fail(PlaybackProblem::Engine { kind }).await;
            return;
        }
        session.fallback_used = true;
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
                false,
                vec![RouteReason::FALLBACK_AFTER_ENGINE_ERROR],
            )
            .await
        {
            self.fail(problem).await;
        }
    }

    async fn fail(&mut self, problem: PlaybackProblem) {
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
        if self.session.as_ref().is_none_or(Session::is_direct_play) {
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
