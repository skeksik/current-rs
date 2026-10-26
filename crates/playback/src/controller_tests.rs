//! Controller tests with a fake engine, a fake server and a fake clock (ADR 0019, task 2). No test waits for real time.

use std::time::Duration;

use current_model::{
    EngineErrorKind, ItemRef, MediaKind, PlaybackProblem, PlayerState, Route, RouteLabel,
    RouteReason, Segment, SegmentKind, StreamAction, SubtitleChoice, TrackChoice, TranscodingFacts,
};

use crate::clock::Clock;
use crate::controller::{Controller, PROGRESS_INTERVAL, STALL_TIMEOUT};
use crate::engine::{EndReason, EngineEvent, EngineTrack};
use crate::planner::PlanNote;
use crate::quality::{Quality, QualityMemory};
use crate::server::{ProgressReport, StopReport, TranscodePermissions};
use crate::status::{AutoSkip, PlayerEvent, PlayerStatus};
use crate::test_support::{
    Command, FakeClock, FakeEngine, FakeServer, Report, SECRETS, answer, block_on, hls, item,
    summary,
};

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

struct Harness {
    player: Controller<FakeEngine, FakeServer, FakeClock>,
    server: FakeServer,
    clock: FakeClock,
}

impl Harness {
    fn new(auto_skip: AutoSkip) -> Self {
        let clock = FakeClock::default();
        // An arbitrary start, so no timing passes only because the clock starts at zero.
        clock.advance(secs(1000));
        let server = FakeServer::new(clock.clone());
        let player = Controller::new(
            FakeEngine::default(),
            server.clone(),
            clock.clone(),
            QualityMemory::default(),
            auto_skip,
        );
        Self {
            player,
            server,
            clock,
        }
    }

    fn now(&self) -> Duration {
        self.clock.now()
    }

    fn play(&mut self, kind: MediaKind, start: Duration) {
        block_on(self.player.play(item("e1"), kind, start));
    }

    fn event(&mut self, event: EngineEvent) {
        block_on(self.player.on_engine_event(event));
    }

    /// The engine opens the file and shows the first frame.
    fn start(&mut self) {
        self.event(EngineEvent::Loaded);
        self.event(EngineEvent::Duration(secs(3600)));
        self.event(EngineEvent::Restarted);
    }

    fn at(&mut self, position: u64) {
        self.event(EngineEvent::Position(secs(position)));
    }

    /// Moves the clock to `by` from now, running every tick that falls due on the way, at its time.
    fn wait(&mut self, by: Duration) {
        let target = self.now() + by;
        for _ in 0..10_000 {
            match self.player.next_deadline() {
                Some(deadline) if deadline <= target => {
                    let now = self.now();
                    if deadline > now {
                        self.clock.advance(deadline - now);
                    }
                    block_on(self.player.tick());
                }
                _ => break,
            }
        }
        let now = self.now();
        self.clock.advance(target - now);
    }

    fn commands(&mut self) -> Vec<Command> {
        self.player.engine_mut().take()
    }

    fn reports(&self) -> Vec<(Duration, Report)> {
        self.server.take_reports()
    }

    fn events(&mut self) -> Vec<PlayerEvent> {
        self.player.take_events()
    }

    fn states(&mut self) -> Vec<PlayerStatus> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                PlayerEvent::State(status) => Some(*status),
                _ => None,
            })
            .collect()
    }

    fn state(&self) -> PlayerState {
        self.player.status().state
    }

    fn requests(&self) -> Vec<(ItemRef, crate::server::PlanRequest)> {
        self.server.state().requests.clone()
    }

    /// Plays e1 from zero, started, with the reports and events so far cleared.
    fn playing(auto_skip: AutoSkip) -> Self {
        let mut harness = Self::new(auto_skip);
        harness.play(MediaKind::MOVIE, Duration::ZERO);
        harness.start();
        harness.reports();
        harness.events();
        harness.commands();
        harness
    }
}

fn progress(report: &Report) -> &ProgressReport {
    match report {
        Report::Progress(report) | Report::Start(report) => report,
        Report::Stopped(_) => panic!("not a progress report: {report:?}"),
    }
}

fn stopped(report: &Report) -> &StopReport {
    match report {
        Report::Stopped(report) => report,
        _ => panic!("not a stop report: {report:?}"),
    }
}

#[test]
fn a_resume_goes_into_the_request_and_the_file_and_starts_reporting_at_the_first_frame() {
    let mut player = Harness::new(AutoSkip::off());
    player.play(MediaKind::MOVIE, secs(600));

    let requests = player.requests();
    assert_eq!(requests.len(), 1);
    let (asked, request) = &requests[0];
    assert_eq!(asked, &item("e1"));
    assert_eq!(request.start, secs(600));
    assert!(request.direct_play);
    assert_eq!(request.quality, Quality::Maximum);
    assert_eq!(request.tracks, TrackChoice::default());
    assert_eq!(request.media_source, None);

    let commands = player.commands();
    let [Command::Load(file)] = commands.as_slice() else {
        panic!("one load: {commands:?}");
    };
    assert_eq!(file.start, secs(600));
    // The server's default audio; no subtitle, as the server picked none.
    assert_eq!(file.audio, EngineTrack::Stream(1));
    assert_eq!(file.subtitle, EngineTrack::Off);
    // An address without a gateway: the server's own URL, with references (ADR 0020).
    assert!(file.url.expose().contains(SECRETS[0]));
    assert_eq!(file.auth.secrets, [SECRETS[0]]);
    assert!(file.references);

    let states: Vec<_> = player.states().into_iter().map(|s| s.state).collect();
    assert_eq!(states.first(), Some(&PlayerState::Preparing));
    assert_eq!(states.last(), Some(&PlayerState::Loading));

    // Nothing is reported before the file is open and the first frame shows.
    player.event(EngineEvent::Loaded);
    assert!(player.reports().is_empty());
    player.event(EngineEvent::Position(secs(600)));
    player.event(EngineEvent::Restarted);
    let reports = player.reports();
    let [(when, Report::Start(start))] = reports.as_slice() else {
        panic!("one start report: {reports:?}");
    };
    assert_eq!(*when, player.now());
    assert_eq!(start.position, secs(600));
    assert_eq!(start.play_session_id, "ps-1");
    assert_eq!(start.media_source, "source-1");
    assert_eq!(start.audio, Some(1));
    assert_eq!(start.subtitle, None);
    assert_eq!(start.route, Route::DirectPlay);
    assert!(!start.paused);
    assert_eq!(player.state(), PlayerState::Playing);
}

#[test]
fn progress_is_reported_every_10_seconds_paused_or_not_and_at_once_on_pause_and_resume() {
    let mut player = Harness::playing(AutoSkip::off());
    let started = player.now();
    player.wait(secs(35));
    let times: Vec<_> = player
        .reports()
        .into_iter()
        .map(|(t, _)| t - started)
        .collect();
    assert_eq!(times, vec![secs(10), secs(20), secs(30)]);

    block_on(player.player.pause());
    assert_eq!(player.commands(), vec![Command::Paused(true)]);
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0 - started, secs(35));
    assert!(progress(&reports[0].1).paused);
    assert_eq!(player.state(), PlayerState::Paused);
    // The engine confirming the pause reports nothing more.
    player.event(EngineEvent::Paused(true));
    assert!(player.reports().is_empty());

    // Paused, the cadence goes on from the pause report.
    player.wait(secs(25));
    let reports = player.reports();
    let times: Vec<_> = reports.iter().map(|(t, _)| *t - started).collect();
    assert_eq!(times, vec![secs(45), secs(55)]);
    assert!(reports.iter().all(|(_, report)| progress(report).paused));

    block_on(player.player.resume());
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0 - started, secs(60));
    assert!(!progress(&reports[0].1).paused);

    // A pause by the engine itself (the system's media keys, for example) is reported as well.
    player.event(EngineEvent::Paused(true));
    assert_eq!(player.reports().len(), 1);
    assert_eq!(player.state(), PlayerState::Paused);
}

#[test]
fn a_seek_is_reported_when_it_is_done_and_restarts_the_cadence() {
    let mut player = Harness::playing(AutoSkip::off());
    player.wait(secs(4));
    block_on(player.player.seek_to(secs(1200)));
    assert_eq!(player.commands(), vec![Command::Seek(secs(1200))]);
    assert_eq!(player.state(), PlayerState::Seeking);
    let states = player.states();
    assert_eq!(states.last().map(|s| s.position), Some(secs(1200)));
    assert!(player.reports().is_empty());

    player.wait(secs(1));
    player.event(EngineEvent::Restarted);
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(progress(&reports[0].1).position, secs(1200));
    assert_eq!(player.state(), PlayerState::Playing);
    let seek_done = player.now();

    player.wait(secs(10));
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, seek_done + PROGRESS_INTERVAL);

    // seek_by goes from the position and stops at the start.
    block_on(player.player.seek_by(-30_000));
    assert_eq!(player.commands(), vec![Command::Seek(secs(1170))]);
    player.event(EngineEvent::Restarted);
    block_on(player.player.seek_by(-2_000_000));
    assert_eq!(player.commands(), vec![Command::Seek(Duration::ZERO)]);
}

#[test]
fn stop_reports_the_position_unloads_and_tells_the_app_once() {
    let mut player = Harness::playing(AutoSkip::off());
    player.at(742);
    block_on(player.player.stop());
    let reports = player.reports();
    let [(_, report)] = reports.as_slice() else {
        panic!("one stop report: {reports:?}");
    };
    let report = stopped(report);
    assert_eq!(report.position, secs(742));
    assert_eq!(report.play_session_id, "ps-1");
    assert!(!report.failed);
    assert_eq!(player.commands(), vec![Command::Stop]);
    let events = player.events();
    assert!(events.contains(&PlayerEvent::Stopped {
        item: item("e1"),
        position: secs(742),
        failed: false,
    }));
    assert_eq!(player.state(), PlayerState::Idle);
    assert_eq!(player.player.next_deadline(), None);

    block_on(player.player.stop());
    assert!(player.reports().is_empty());
    assert!(player.events().is_empty());
}

#[test]
fn the_end_is_reported_once_and_stop_after_it_reports_nothing() {
    let mut player = Harness::playing(AutoSkip::off());
    player.event(EngineEvent::Ended(EndReason::Eof));
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(stopped(&reports[0].1).position, secs(3600));
    assert_eq!(player.state(), PlayerState::Ended);
    let stops = player
        .events()
        .into_iter()
        .filter(|event| matches!(event, PlayerEvent::Stopped { .. }))
        .count();
    assert_eq!(stops, 1);
    // No progress after the end.
    player.wait(secs(60));
    assert!(player.reports().is_empty());

    block_on(player.player.stop());
    assert!(player.reports().is_empty());
    assert!(
        !player
            .events()
            .iter()
            .any(|event| matches!(event, PlayerEvent::Stopped { .. }))
    );
}

#[test]
fn another_item_stops_the_first_and_loads_on_the_same_engine() {
    let mut player = Harness::playing(AutoSkip::off());
    player.at(100);
    block_on(
        player
            .player
            .play(item("m2"), MediaKind::MOVIE, Duration::ZERO),
    );
    let reports = player.reports();
    let [(_, report)] = reports.as_slice() else {
        panic!("{reports:?}");
    };
    assert_eq!(stopped(report).item, item("e1"));
    assert_eq!(stopped(report).position, secs(100));
    assert!(player.events().contains(&PlayerEvent::Stopped {
        item: item("e1"),
        position: secs(100),
        failed: false,
    }));
    let commands = player.commands();
    assert!(
        matches!(commands.as_slice(), [Command::Load(_)]),
        "{commands:?}"
    );
    assert_eq!(player.requests()[1].0, item("m2"));
}

#[test]
fn tracks_switch_in_the_engine_under_direct_play() {
    let mut player = Harness::playing(AutoSkip::off());
    block_on(player.player.select_audio(2));
    assert_eq!(
        player.commands(),
        vec![Command::Audio(EngineTrack::Stream(2))]
    );
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(progress(&reports[0].1).audio, Some(2));
    assert_eq!(player.requests().len(), 1, "no new plan");

    block_on(player.player.select_subtitle(SubtitleChoice::Stream(3)));
    assert_eq!(
        player.commands(),
        vec![Command::Subtitle(EngineTrack::Stream(3))]
    );
    assert_eq!(progress(&player.reports()[0].1).subtitle, Some(3));

    // An external file is fetched by the core once and added to the engine.
    block_on(player.player.select_subtitle(SubtitleChoice::Stream(5)));
    let commands = player.commands();
    let [Command::AddSubtitle(added)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    assert_eq!(added.index, 5);
    assert!(added.select);
    assert!(!added.data.is_empty());
    block_on(player.player.select_subtitle(SubtitleChoice::Off));
    block_on(player.player.select_subtitle(SubtitleChoice::Stream(5)));
    assert_eq!(
        player.commands(),
        vec![
            Command::Subtitle(EngineTrack::Off),
            Command::Subtitle(EngineTrack::Stream(5))
        ]
    );
    assert_eq!(player.server.state().subtitle_fetches, vec![5]);
    let status = player.player.status();
    assert_eq!(
        status.tracks,
        TrackChoice {
            audio: Some(2),
            subtitle: SubtitleChoice::Stream(5)
        }
    );
    assert_eq!(status.audio_tracks.len(), 2);
    assert_eq!(status.subtitle_tracks.len(), 3);
    assert_eq!(player.requests().len(), 1, "still no new plan");
}

#[test]
fn audio_and_burned_in_subtitles_re_plan_a_server_stream_at_the_position() {
    let mut player = Harness::new(AutoSkip::off());
    let stream = || hls(StreamAction::Transcode, StreamAction::Transcode);
    for _ in 0..4 {
        player.server.answer(Ok(answer(stream(), vec![])));
    }
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    player.at(300);
    player.reports();
    player.events();
    player.commands();

    block_on(player.player.select_audio(2));
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(stopped(&reports[0].1).play_session_id, "ps-1");
    assert_eq!(stopped(&reports[0].1).position, secs(300));
    let requests = player.requests();
    let request = &requests[1].1;
    assert_eq!(request.start, secs(300));
    assert_eq!(request.tracks.audio, Some(2));
    assert_eq!(request.media_source.as_deref(), Some("source-1"));
    assert!(request.direct_play);
    let states: Vec<_> = player.states().into_iter().map(|s| s.state).collect();
    assert!(states.contains(&PlayerState::Preparing));
    assert_eq!(player.state(), PlayerState::Loading);
    let commands = player.commands();
    let [Command::Load(file)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    assert_eq!(file.start, secs(300));
    // The server picks the audio of its stream.
    assert_eq!(file.audio, EngineTrack::Auto);
    // The item did not stop for the rest of the app.
    assert!(
        !player
            .events()
            .iter()
            .any(|event| matches!(event, PlayerEvent::Stopped { .. }))
    );
    player.event(EngineEvent::Restarted);
    let reports = player.reports();
    assert_eq!(progress(&reports[0].1).play_session_id, "ps-2");
    assert_eq!(progress(&reports[0].1).audio, Some(2));

    // An embedded subtitle the stream carries switches in the engine.
    block_on(player.player.select_subtitle(SubtitleChoice::Stream(3)));
    assert_eq!(
        player.commands(),
        vec![Command::Subtitle(EngineTrack::Stream(3))]
    );
    assert_eq!(player.requests().len(), 2);

    // A burned-in one needs a new stream, and so does leaving it.
    block_on(player.player.select_subtitle(SubtitleChoice::Stream(4)));
    let requests = player.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].1.tracks.subtitle, SubtitleChoice::Stream(4));
    let commands = player.commands();
    let [Command::Load(file)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    assert_eq!(file.subtitle, EngineTrack::Off, "burned into the video");
    player.event(EngineEvent::Restarted);
    block_on(player.player.select_subtitle(SubtitleChoice::Off));
    assert_eq!(player.requests().len(), 4);
    assert_eq!(player.requests()[3].1.tracks.subtitle, SubtitleChoice::Off);
}

#[test]
fn an_engine_failure_on_direct_play_falls_back_once_and_a_second_one_fails() {
    let mut player = Harness::playing(AutoSkip::off());
    player.server.answer(Ok(answer(
        hls(StreamAction::Copy, StreamAction::Copy),
        vec![],
    )));
    block_on(player.player.select_audio(2));
    player.at(100);
    player.reports();
    player.commands();

    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::DECODER_FAILED,
    )));
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    let report = stopped(&reports[0].1);
    assert_eq!(report.play_session_id, "ps-1");
    assert_eq!(report.position, secs(100));
    let requests = player.requests();
    assert_eq!(requests.len(), 2);
    let request = &requests[1].1;
    assert!(!request.direct_play);
    assert_eq!(request.start, secs(100));
    assert_eq!(request.tracks.audio, Some(2), "the same tracks");
    let details = player.player.details().and_then(|d| d.plan);
    let plan = details.as_ref();
    assert_eq!(
        plan.map(|plan| plan.reasons.first().cloned()),
        Some(Some(RouteReason::FALLBACK_AFTER_ENGINE_ERROR))
    );
    assert_eq!(player.state(), PlayerState::Loading);
    assert!(
        !player
            .events()
            .iter()
            .any(|event| matches!(event, PlayerEvent::Stopped { .. }))
    );
    let commands = player.commands();
    let [Command::Load(file)] = commands.as_slice() else {
        panic!("{commands:?}");
    };
    assert_eq!(file.start, secs(100));

    player.event(EngineEvent::Restarted);
    assert_eq!(progress(&player.reports()[0].1).play_session_id, "ps-2");
    player.at(130);
    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::LOADING_FAILED,
    )));
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::Engine {
                kind: EngineErrorKind::LOADING_FAILED
            }
        }
    );
    assert_eq!(player.requests().len(), 2, "no second fallback");
    let reports = player.reports();
    assert_eq!(reports.len(), 1);
    assert!(stopped(&reports[0].1).failed);
    assert_eq!(stopped(&reports[0].1).position, secs(130));
    assert!(player.events().contains(&PlayerEvent::Stopped {
        item: item("e1"),
        position: secs(130),
        failed: true,
    }));
    // A failed player reports nothing more.
    player.wait(secs(60));
    assert!(player.reports().is_empty());
}

#[test]
fn a_failure_before_the_first_frame_falls_back_too_and_one_after_a_direct_play_answer_fails() {
    let mut player = Harness::new(AutoSkip::off());
    player.play(MediaKind::MOVIE, secs(50));
    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::UNSUPPORTED_FORMAT,
    )));
    let requests = player.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[1].1.direct_play);
    assert_eq!(requests[1].1.start, secs(50));
    // The fake answers direct play again: a failure now is the second one.
    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::UNSUPPORTED_FORMAT,
    )));
    assert!(matches!(player.state(), PlayerState::Failed { .. }));
    assert_eq!(player.requests().len(), 2);
}

#[test]
fn a_failure_on_a_server_stream_fails_at_once() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.answer(Ok(answer(
        hls(StreamAction::Transcode, StreamAction::Copy),
        vec![RouteReason::VIDEO_CODEC_NOT_SUPPORTED],
    )));
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::LOADING_FAILED,
    )));
    assert!(matches!(player.state(), PlayerState::Failed { .. }));
    assert_eq!(player.requests().len(), 1);
}

#[test]
fn no_route_fails_with_the_permissions_explained() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().permissions = TranscodePermissions {
        video_transcoding: false,
        ..TranscodePermissions::default()
    };
    player.server.answer(Err(PlaybackProblem::NotPlayable {
        reasons: vec![RouteReason::VIDEO_CODEC_NOT_SUPPORTED],
    }));
    player.play(MediaKind::MOVIE, Duration::ZERO);
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::NotPlayable {
                reasons: vec![RouteReason::VIDEO_CODEC_NOT_SUPPORTED]
            }
        }
    );
    assert!(player.commands().is_empty());
    assert!(player.reports().is_empty(), "no play session to stop");
    assert_eq!(
        player.player.details().map(|d| d.notes),
        Some(vec![PlanNote::TranscodingNotAllowed])
    );
    assert!(player.events().contains(&PlayerEvent::Stopped {
        item: item("e1"),
        position: Duration::ZERO,
        failed: true,
    }));
}

#[test]
fn the_servers_transcoding_facts_correct_the_route_after_start() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.answer(Ok(answer(
        hls(StreamAction::Transcode, StreamAction::Transcode),
        vec![],
    )));
    player.server.state().facts = Some(TranscodingFacts {
        video_direct: true,
        audio_direct: true,
        ..TranscodingFacts::default()
    });
    player.play(MediaKind::MOVIE, Duration::ZERO);
    assert_eq!(
        player.player.status().route.map(|route| route.label()),
        Some(RouteLabel::Transcode)
    );
    player.start();
    assert_eq!(
        player.player.status().route.map(|route| route.label()),
        Some(RouteLabel::Remux)
    );
    assert!(
        player
            .player
            .details()
            .and_then(|d| d.transcoding)
            .is_some()
    );
}

fn with_segments(player: &mut Harness, segments: Vec<Segment>) {
    let mut with = answer(Route::DirectPlay, vec![]);
    with.source.has_segments = true;
    player.server.answer(Ok(with));
    player.server.state().segments = segments;
}

fn intro() -> Segment {
    Segment {
        kind: SegmentKind::INTRO,
        start: secs(60),
        end: secs(90),
    }
}

#[test]
fn segments_are_fetched_once_and_shown_with_a_skip_button_without_auto_skip() {
    let mut player = Harness::new(AutoSkip::off());
    with_segments(&mut player, vec![intro()]);
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    player.commands();
    player.events();
    player.at(59);
    assert_eq!(player.player.status().segment, None);
    player.at(60);
    // Entering a segment is published at once.
    assert_eq!(
        player.states().last().and_then(|s| s.segment.clone()),
        Some(intro())
    );
    player.at(61);
    assert!(player.commands().is_empty(), "no automatic skip");

    block_on(player.player.skip_segment());
    assert_eq!(player.commands(), vec![Command::Seek(secs(90))]);
    assert_eq!(player.player.status().segment, None);
    player.event(EngineEvent::Restarted);

    // A re-plan keeps the segments without asking again.
    block_on(player.player.set_quality(Quality::Bitrate(4_000_000)));
    assert_eq!(player.server.state().segment_calls, 1);

    // A source without segments is not asked for them.
    let mut plain = Harness::new(AutoSkip::off());
    plain.play(MediaKind::MOVIE, Duration::ZERO);
    assert_eq!(plain.server.state().segment_calls, 0);
}

#[test]
fn auto_skip_skips_a_segment_once_and_not_again_after_a_seek_back() {
    let mut player = Harness::new(AutoSkip::intros_and_credits());
    let recap = Segment {
        kind: SegmentKind::RECAP,
        start: secs(0),
        end: secs(30),
    };
    with_segments(&mut player, vec![recap.clone(), intro()]);
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    player.commands();
    player.at(10);
    assert!(
        player.commands().is_empty(),
        "a recap is not in the setting"
    );
    assert_eq!(player.player.status().segment, Some(recap));

    player.at(60);
    assert_eq!(player.commands(), vec![Command::Seek(secs(90))]);
    assert_eq!(player.state(), PlayerState::Seeking);
    player.event(EngineEvent::Restarted);
    player.at(91);
    assert!(
        player
            .reports()
            .iter()
            .any(|(_, r)| matches!(r, Report::Progress(_)))
    );

    // Back into the intro: it plays.
    block_on(player.player.seek_to(secs(65)));
    assert_eq!(player.commands(), vec![Command::Seek(secs(65))]);
    player.event(EngineEvent::Restarted);
    for position in 65..92 {
        player.at(position);
    }
    assert!(player.commands().is_empty());
    assert_eq!(player.state(), PlayerState::Playing);
}

#[test]
fn auto_skip_takes_any_kind_and_starts_inside_a_segment_skip_it() {
    let mut player = Harness::new(AutoSkip::new([SegmentKind::RECAP]));
    with_segments(
        &mut player,
        vec![Segment {
            kind: SegmentKind::RECAP,
            start: secs(0),
            end: secs(30),
        }],
    );
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    let commands = player.commands();
    assert_eq!(commands.last(), Some(&Command::Seek(secs(30))));
}

#[test]
fn the_next_episode_card_shows_in_the_last_30_seconds_without_segments() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    player.at(3569);
    assert_eq!(player.player.status().next, None);
    player.events();
    player.at(3571);
    let next = player.states().last().and_then(|s| s.next.clone());
    assert_eq!(next.as_ref().map(|n| n.item.item.clone()), Some(item("e2")));
    assert_eq!(
        next.and_then(|n| n.countdown),
        None,
        "no countdown before the end"
    );

    player.player.dismiss_next();
    assert_eq!(player.player.status().next, None);
    player.at(3580);
    assert_eq!(player.player.status().next, None, "dismissed for this play");
}

#[test]
fn the_next_episode_card_shows_from_the_outro() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    with_segments(
        &mut player,
        vec![Segment {
            kind: SegmentKind::OUTRO,
            start: secs(3400),
            end: secs(3500),
        }],
    );
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    player.at(3399);
    assert_eq!(player.player.status().next, None);
    player.at(3400);
    assert!(player.player.status().next.is_some());

    // A movie has no next episode, and the server is not asked.
    let mut movie = Harness::new(AutoSkip::off());
    movie.server.state().next = Some(summary("e2"));
    movie.play(MediaKind::MOVIE, Duration::ZERO);
    movie.start();
    movie.at(3590);
    assert_eq!(movie.player.status().next, None);
    assert_eq!(movie.server.state().next_calls, 0);
}

#[test]
fn at_the_end_the_next_episode_starts_after_a_10_second_countdown_on_the_same_engine() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    player.commands();
    player.events();
    player.event(EngineEvent::Ended(EndReason::Eof));
    let ended = player.now();
    let status = player.player.status();
    assert_eq!(status.state, PlayerState::Ended);
    assert_eq!(status.next.and_then(|n| n.countdown), Some(10));

    player.wait(secs(3));
    let countdowns: Vec<_> = player
        .states()
        .into_iter()
        .filter_map(|s| s.next.and_then(|n| n.countdown))
        .collect();
    assert_eq!(countdowns, vec![10, 9, 8, 7]);
    assert_eq!(player.requests().len(), 1);

    player.wait(secs(7));
    assert_eq!(player.now() - ended, secs(10));
    let requests = player.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].0, item("e2"));
    assert_eq!(requests[1].1.start, Duration::ZERO);
    let commands = player.commands();
    assert!(
        matches!(commands.as_slice(), [Command::Load(_)]),
        "{commands:?}"
    );
    assert_eq!(player.state(), PlayerState::Loading);
    // e1's end was reported once, before e2 began.
    let stops = player
        .reports()
        .into_iter()
        .filter(|(_, r)| matches!(r, Report::Stopped(_)))
        .count();
    assert_eq!(stops, 1);
}

#[test]
fn without_autoplay_the_card_stays_and_nothing_starts() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    player.server.state().preferences.next_episode_autoplay = false;
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    player.event(EngineEvent::Ended(EndReason::Eof));
    let next = player.player.status().next;
    assert!(next.is_some());
    assert_eq!(next.and_then(|n| n.countdown), None);
    player.wait(secs(60));
    assert_eq!(player.requests().len(), 1);

    block_on(player.player.play_next());
    assert_eq!(player.requests()[1].0, item("e2"));
}

#[test]
fn dismissing_the_card_stops_the_countdown() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    player.event(EngineEvent::Ended(EndReason::Eof));
    player.wait(secs(4));
    player.player.dismiss_next();
    player.wait(secs(30));
    assert_eq!(player.requests().len(), 1);
    assert_eq!(player.player.next_deadline(), None);
}

#[test]
fn the_position_is_published_at_most_4_times_a_second_and_changes_at_once() {
    let mut player = Harness::playing(AutoSkip::off());
    // A position event per 10 ms (100 a second, a fast display) for 2 s, with ticks when due.
    for step in 0..200u64 {
        player.clock.advance(Duration::from_millis(10));
        player.event(EngineEvent::Position(Duration::from_millis(step * 10)));
        if player
            .player
            .next_deadline()
            .is_some_and(|d| d <= player.now())
        {
            block_on(player.player.tick());
        }
    }
    let states = player.states();
    assert!((7..=9).contains(&states.len()), "{} states", states.len());
    // The last position is not held back for long.
    player.wait(Duration::from_millis(250));
    let last = player.states();
    let latest = last
        .last()
        .map(|s| s.position)
        .or(states.last().map(|s| s.position));
    assert_eq!(latest, Some(Duration::from_millis(1990)));

    // A change of state is published at once, even right after a position.
    player.event(EngineEvent::Position(secs(3)));
    block_on(player.player.pause());
    let states = player.states();
    assert_eq!(
        states.last().map(|s| s.state.clone()),
        Some(PlayerState::Paused)
    );
    player.event(EngineEvent::Buffering(true));
    assert_eq!(
        player.states().last().map(|s| s.state.clone()),
        Some(PlayerState::Buffering)
    );
}

#[test]
fn a_quality_change_is_remembered_for_the_address_kind_and_re_plans() {
    let mut player = Harness::playing(AutoSkip::off());
    player.server.answer(Ok(answer(
        hls(StreamAction::Transcode, StreamAction::Copy),
        vec![],
    )));
    player.at(200);
    block_on(player.player.set_quality(Quality::Bitrate(8_000_000)));
    let events = player.events();
    assert!(events.contains(&PlayerEvent::QualityChanged {
        local: true,
        quality: Quality::Bitrate(8_000_000),
    }));
    assert_eq!(
        player.player.quality_memory(),
        QualityMemory {
            local: Quality::Bitrate(8_000_000),
            internet: Quality::Maximum,
        }
    );
    let requests = player.requests();
    assert_eq!(requests[1].1.quality, Quality::Bitrate(8_000_000));
    assert_eq!(requests[1].1.start, secs(200));
    let reasons = player
        .player
        .details()
        .and_then(|d| d.plan)
        .map(|p| p.reasons);
    assert_eq!(reasons, Some(vec![RouteReason::QUALITY_LIMIT]));

    // On an internet address the other memory is used.
    player.server.state().local = false;
    assert_eq!(player.player.status().quality, Quality::Maximum);
}

#[test]
fn nothing_published_or_loaded_shows_a_secret() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().next = Some(summary("e2"));
    player.play(MediaKind::EPISODE, Duration::ZERO);
    player.start();
    block_on(player.player.select_subtitle(SubtitleChoice::Stream(5)));
    player.event(EngineEvent::Ended(EndReason::Eof));
    let shown = format!(
        "{:?} {:?} {:?} {:?} {:?}",
        player.events(),
        player.player.engine(),
        player.player.details(),
        player.player.status(),
        answer(Route::DirectPlay, vec![]),
    );
    for secret in SECRETS {
        assert!(!shown.contains(secret), "{secret} shown");
    }
}

#[test]
fn a_relayed_stream_loads_the_relay_url_and_opens_references_only_for_hls() {
    let progressive = Route::ServerStream {
        protocol: current_model::StreamProtocol::Progressive,
        container: Some("mkv".into()),
        video: StreamAction::Copy,
        audio: StreamAction::Transcode,
    };
    for (route, references) in [
        (Route::DirectPlay, false),
        (progressive, false),
        (hls(StreamAction::Copy, StreamAction::Copy), true),
    ] {
        let mut player = Harness::new(AutoSkip::off());
        player.server.state().relay = true;
        player.server.answer(Ok(answer(route.clone(), vec![])));
        player.play(MediaKind::MOVIE, Duration::ZERO);
        // The server's URL went to `stream_url`; the engine got the relay's, with its token to redact and no session token.
        assert!(player.server.state().streams[0].contains(SECRETS[0]));
        let commands = player.commands();
        let [Command::Load(file)] = commands.as_slice() else {
            panic!("one load: {commands:?}");
        };
        assert!(!file.url.expose().contains(SECRETS[0]), "{route:?}");
        assert!(file.url.expose().contains(SECRETS[1]));
        assert_eq!(file.auth.secrets, [SECRETS[1]]);
        assert_eq!(file.references, references, "{route:?}");
    }
}

#[test]
fn a_relay_that_cannot_start_fails_without_loading() {
    let mut player = Harness::new(AutoSkip::off());
    player.server.state().stream_problem = Some(PlaybackProblem::LocalRelay);
    player.play(MediaKind::MOVIE, Duration::ZERO);
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::LocalRelay
        }
    );
    assert!(player.commands().is_empty());
}

#[test]
fn a_stream_failure_stops_the_engine_and_fails_once() {
    for problem in [
        PlaybackProblem::GatewaySignInNeeded,
        PlaybackProblem::SignedOut,
        PlaybackProblem::NotFound,
        PlaybackProblem::Server { status: 500 },
        PlaybackProblem::PlaylistElsewhere,
    ] {
        let mut player = Harness::playing(AutoSkip::off());
        player.at(120);
        block_on(player.player.stream_failed(problem.clone()));
        assert_eq!(
            player.state(),
            PlayerState::Failed {
                problem: problem.clone()
            }
        );
        assert_eq!(player.commands(), vec![Command::Stop]);
        let reports = player.reports();
        let [(_, report)] = reports.as_slice() else {
            panic!("one stop report: {reports:?}");
        };
        assert!(stopped(report).failed);
        assert_eq!(stopped(report).position, secs(120));
        assert!(!player.server.state().stream_live);
        // No fallback: the same server would refuse another route too.
        assert_eq!(player.requests().len(), 1);
        // A second event of the stream changes nothing.
        block_on(player.player.stream_failed(PlaybackProblem::Network));
        assert_eq!(player.state(), PlayerState::Failed { problem });
        assert!(player.commands().is_empty());
        assert!(player.reports().is_empty());
    }
}

#[test]
fn a_connection_lost_while_playing_from_the_buffer_fails_only_with_the_engine() {
    // While loading the engine waits on the stream: it fails at once.
    let mut player = Harness::new(AutoSkip::off());
    player.play(MediaKind::MOVIE, Duration::ZERO);
    block_on(player.player.stream_failed(PlaybackProblem::Network));
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::Network
        }
    );

    // Playing (or paused) from the buffer: the next read opens the connection again.
    let mut player = Harness::playing(AutoSkip::off());
    block_on(player.player.pause());
    block_on(player.player.stream_failed(PlaybackProblem::Network));
    assert_eq!(player.state(), PlayerState::Paused);
    // If the engine then fails, it is the connection's failure: no fallback to a server stream.
    player.event(EngineEvent::Ended(EndReason::Error(
        EngineErrorKind::LOADING_FAILED,
    )));
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::Network
        }
    );
    assert_eq!(player.requests().len(), 1);
}

#[test]
fn stop_another_item_and_a_re_plan_end_the_stream() {
    let mut player = Harness::playing(AutoSkip::off());
    let server = player.server.clone();
    let ends = || player_ends(&server);
    let before = ends();
    assert!(player.server.state().stream_live);
    block_on(player.player.stop());
    assert!(!player.server.state().stream_live);
    assert_eq!(ends(), before + 1);

    // Another item ends the first one's stream before its own is made.
    player.play(MediaKind::MOVIE, Duration::ZERO);
    player.start();
    let before = ends();
    player.play(MediaKind::MOVIE, Duration::ZERO);
    assert_eq!(ends(), before + 1);
    assert_eq!(player.server.state().streams.len(), 3);
    // A re-plan makes a new stream, which replaces the old one.
    player.start();
    block_on(player.player.set_quality(Quality::Bitrate(4_000_000)));
    assert_eq!(player.server.state().streams.len(), 4);
    assert!(player.server.state().stream_live);
    // A stream event while nothing plays changes nothing.
    block_on(player.player.stop());
    block_on(player.player.stream_failed(PlaybackProblem::NotFound));
    assert_eq!(player.state(), PlayerState::Idle);
}

fn player_ends(server: &FakeServer) -> u32 {
    server.state().stream_ends
}

/// The stall ends playback: the engine stopped, the play session reported stopped with `Failed = true` at `position`, the app told
/// once, and nothing due afterwards.
fn assert_stalled(player: &mut Harness, position: Duration) {
    assert_eq!(
        player.state(),
        PlayerState::Failed {
            problem: PlaybackProblem::Network
        }
    );
    assert_eq!(player.commands(), vec![Command::Stop]);
    let reports = player.reports();
    let [(_, Report::Stopped(report))] = reports.as_slice() else {
        panic!("one stop report: {reports:?}");
    };
    assert!(report.failed);
    assert_eq!(report.position, position);
    let stops: Vec<_> = player
        .events()
        .into_iter()
        .filter(|event| matches!(event, PlayerEvent::Stopped { .. }))
        .collect();
    assert_eq!(
        stops,
        vec![PlayerEvent::Stopped {
            item: item("e1"),
            position,
            failed: true,
        }]
    );
    assert_eq!(player.player.next_deadline(), None);
    player.wait(secs(600));
    assert!(player.reports().is_empty());
    assert!(player.commands().is_empty());
}

#[test]
fn a_load_with_no_progress_fails_as_a_network_problem_after_the_stall_timeout() {
    let mut player = Harness::new(AutoSkip::off());
    player.play(MediaKind::MOVIE, secs(600));
    player.commands();
    player.reports();
    player.events();
    assert_eq!(
        player.player.next_deadline(),
        Some(player.now() + STALL_TIMEOUT)
    );

    player.wait(STALL_TIMEOUT - Duration::from_millis(1));
    assert_eq!(player.state(), PlayerState::Loading);
    assert!(player.commands().is_empty());
    player.wait(Duration::from_millis(1));
    assert_stalled(&mut player, secs(600));
}

#[test]
fn buffering_with_no_progress_fails_and_data_arriving_holds_it_off() {
    let mut player = Harness::playing(AutoSkip::off());
    player.at(100);
    player.event(EngineEvent::Buffering(true));
    assert_eq!(player.state(), PlayerState::Buffering);
    // Data trickling in, slower than playback: still buffering, not stalled.
    for ahead in 1..=5 {
        player.wait(secs(50));
        player.event(EngineEvent::Buffered(secs(ahead)));
    }
    assert_eq!(player.state(), PlayerState::Buffering);
    player.reports();
    player.events();

    player.wait(STALL_TIMEOUT - secs(1));
    assert_eq!(player.state(), PlayerState::Buffering);
    // The progress reports go on meanwhile.
    assert!(!player.reports().is_empty());
    player.wait(secs(1));
    assert_stalled(&mut player, secs(100));
}

#[test]
fn a_seek_that_never_finishes_fails() {
    let mut player = Harness::playing(AutoSkip::off());
    player.at(100);
    player.wait(secs(5));
    block_on(player.player.seek_to(secs(1200)));
    assert_eq!(player.commands(), vec![Command::Seek(secs(1200))]);
    player.reports();
    player.events();

    player.wait(STALL_TIMEOUT - secs(1));
    assert_eq!(player.state(), PlayerState::Seeking);
    player.reports();
    player.wait(secs(1));
    assert_stalled(&mut player, secs(1200));
}

#[test]
fn a_paused_player_never_stalls_and_the_wait_starts_again_on_resume() {
    let mut player = Harness::playing(AutoSkip::off());
    player.at(100);
    block_on(player.player.pause());
    player.wait(secs(600));
    assert_eq!(player.state(), PlayerState::Paused);
    // Waiting for data while the user has it paused is not a stall either.
    player.event(EngineEvent::Buffering(true));
    player.wait(secs(600));
    assert_eq!(player.state(), PlayerState::Buffering);
    player.commands();
    player.reports();
    player.events();

    // From the resume, still waiting for data: the full timeout, not the time since the last event.
    block_on(player.player.resume());
    assert_eq!(player.commands(), vec![Command::Paused(false)]);
    player.wait(STALL_TIMEOUT - secs(1));
    assert_eq!(player.state(), PlayerState::Buffering);
    player.reports();
    player.wait(secs(1));
    assert_stalled(&mut player, secs(100));
}

#[test]
fn progress_resets_the_stall_timer_and_a_repeated_value_does_not() {
    let mut player = Harness::new(AutoSkip::off());
    player.play(MediaKind::MOVIE, Duration::ZERO);
    // A slow load: the file opens late, then positions and buffer come in, each well inside the timeout.
    player.wait(secs(50));
    player.event(EngineEvent::Loaded);
    for second in 1..=10 {
        player.wait(secs(50));
        if second % 2 == 0 {
            player.at(second);
        } else {
            player.event(EngineEvent::Buffered(secs(second)));
        }
    }
    assert_eq!(player.state(), PlayerState::Loading);
    player.event(EngineEvent::Restarted);
    assert_eq!(player.state(), PlayerState::Playing);
    // Playing with no events at all is no stall: the rule watches only a player waiting on the engine.
    player.wait(secs(600));
    assert_eq!(player.state(), PlayerState::Playing);

    // The same position and buffer again are no progress.
    player.event(EngineEvent::Buffering(true));
    player.wait(secs(30));
    player.at(10);
    player.event(EngineEvent::Buffered(secs(9)));
    player.wait(secs(29));
    assert_eq!(player.state(), PlayerState::Buffering);
    player.commands();
    player.reports();
    player.events();
    player.wait(secs(1));
    assert_stalled(&mut player, secs(10));
}
