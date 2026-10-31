//! The controller with a native engine beside the app engine (ADR 0025): the chain on a failed start, track, speed and display
//! changes across engines, "Play with app engine" and the "Player" setting. Fake engines shaped like Windows' player and libmpv; no
//! test waits for real time.

use std::time::Duration;

use current_model::{
    AudioFacts, Codec, CodecSupport, Container, EngineCapabilities, EngineErrorKind, EngineRole,
    HowToPlay, MediaKind, MediaSource, MediaStream, PlaybackPlan, PlayerState, Route, RouteReason,
    StreamAction, StreamKind, SubtitleChoice, SubtitleDelivery, SubtitleFormat, VideoFacts,
    VideoRangeType,
};

use crate::clock::Clock;
use crate::controller::{Controller, NativeEngine, STALL_TIMEOUT};
use crate::engine::{EndReason, EngineEvent, EngineTrack};
use crate::engines::{DisplayKind, PlatformRules};
use crate::quality::QualityPreference;
use crate::status::AutoSkip;
use crate::test_support::{
    Command, FakeClock, FakeEngine, FakeServer, SharedEngine, answer, block_on, hls, item,
};

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Windows' player, as far as these tests need: MKV and MP4 (and HLS), H.264 up to 8 bit and HEVC, AAC and E-AC-3, SubRip and PGS.
fn native_caps() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4],
        video: vec![
            CodecSupport {
                max_bit_depth: Some(8),
                ..CodecSupport::any(Codec::H264)
            },
            CodecSupport::any(Codec::HEVC),
        ],
        audio: vec![
            CodecSupport::any(Codec::AAC),
            CodecSupport::any(Codec::EAC3),
        ],
        subtitles: vec![SubtitleFormat::SUBRIP, SubtitleFormat::PGSSUB],
        external_subtitles: true,
    }
}

/// libmpv, as far as these tests need.
fn app_caps() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4, Container::TS],
        video: vec![
            CodecSupport::any(Codec::H264),
            CodecSupport::any(Codec::HEVC),
        ],
        audio: [Codec::AAC, Codec::EAC3, Codec::TRUEHD]
            .map(CodecSupport::any)
            .to_vec(),
        subtitles: vec![
            SubtitleFormat::SUBRIP,
            SubtitleFormat::PGSSUB,
            SubtitleFormat::DVDSUB,
        ],
        external_subtitles: true,
    }
}

fn stream(index: u32, kind: StreamKind, codec: &str) -> MediaStream {
    MediaStream {
        index,
        kind,
        codec: Some(Codec::new(codec)),
        profile: None,
        level: None,
        language: None,
        title: None,
        display_title: None,
        is_default: false,
        is_forced: false,
        is_hearing_impaired: false,
        is_external: false,
        bitrate: None,
        video: None,
        audio: None,
        subtitle_delivery: Some(SubtitleDelivery::EMBED),
    }
}

/// An MKV with video 0 (`codec`, `bit_depth`, `range`), AAC 1, TrueHD 2, E-AC-3 Atmos 3; subtitles SubRip 4, PGS 5, DVD 6.
fn movie_with(codec: &str, bit_depth: u32, range: VideoRangeType) -> MediaSource {
    let mut video = stream(0, StreamKind::VIDEO, codec);
    video.video = Some(VideoFacts {
        width: Some(3840),
        height: Some(2160),
        bit_depth: Some(bit_depth),
        frame_rate: Some(24.0),
        reference_frame_rate: None,
        range: None,
        range_type: Some(range),
        dv_profile: None,
    });
    let audio = |index, codec| MediaStream {
        audio: Some(AudioFacts {
            channels: Some(6),
            layout: None,
            sample_rate: Some(48_000),
        }),
        ..stream(index, StreamKind::AUDIO, codec)
    };
    MediaSource {
        id: "source-1".into(),
        name: None,
        container: Some("mkv".into()),
        size: None,
        bitrate: Some(20_000_000),
        duration: Some(secs(3600)),
        streams: vec![
            video,
            audio(1, "aac"),
            audio(2, "truehd"),
            audio(3, "eac3"),
            stream(4, StreamKind::SUBTITLE, "subrip"),
            stream(5, StreamKind::SUBTITLE, "pgssub"),
            stream(6, StreamKind::SUBTITLE, "dvdsub"),
        ],
        default_audio: Some(1),
        default_subtitle: None,
        has_segments: false,
        is_remote: false,
    }
}

fn movie() -> MediaSource {
    movie_with("h264", 8, VideoRangeType::SDR)
}

struct Two {
    player: Controller<FakeEngine, FakeServer, FakeClock>,
    server: FakeServer,
    native: SharedEngine,
    clock: FakeClock,
}

impl Two {
    fn new(source: MediaSource) -> Self {
        let clock = FakeClock::default();
        clock.advance(secs(1000));
        let server = FakeServer::new(clock.clone());
        server.state().source = Some(source);
        let app = FakeEngine {
            caps: Some(app_caps()),
            ..FakeEngine::default()
        };
        let native = SharedEngine::default();
        native.0.lock().expect("fake engine").caps = Some(native_caps());
        let player = Controller::new(
            app,
            server.clone(),
            clock.clone(),
            QualityPreference::default(),
            AutoSkip::off(),
        )
        .with_native_engine(NativeEngine {
            engine: Box::new(native.clone()),
            hls: true,
            remux: None,
            rules: PlatformRules::WINDOWS,
        });
        native.take();
        Self {
            player,
            server,
            native,
            clock,
        }
    }

    /// Plays e1 and starts it on whichever engine has it.
    fn playing(source: MediaSource) -> Self {
        let mut two = Self::new(source);
        block_on(
            two.player
                .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
        );
        two.start();
        two
    }

    fn role(&self) -> EngineRole {
        self.plan().route.engine()
    }

    fn event(&mut self, event: EngineEvent) {
        match self.role() {
            EngineRole::Native => block_on(self.player.on_native_engine_event(event)),
            EngineRole::App => block_on(self.player.on_engine_event(event)),
        }
    }

    fn start(&mut self) {
        self.event(EngineEvent::Loaded);
        self.event(EngineEvent::Duration(secs(3600)));
        self.event(EngineEvent::Restarted);
    }

    fn at(&mut self, position: u64) {
        self.event(EngineEvent::Position(secs(position)));
    }

    fn fail(&mut self) {
        self.event(EngineEvent::Ended(EndReason::Error(
            EngineErrorKind::DECODER_FAILED,
        )));
    }

    /// Moves the clock on by `by`, running every tick that falls due on the way, at its time.
    fn wait(&mut self, by: Duration) {
        let target = self.clock.now() + by;
        while let Some(deadline) = self.player.next_deadline().filter(|due| *due <= target) {
            let now = self.clock.now();
            self.clock.advance(deadline.saturating_sub(now));
            block_on(self.player.tick());
        }
        let now = self.clock.now();
        self.clock.advance(target.saturating_sub(now));
    }

    fn plan(&self) -> PlaybackPlan {
        self.player
            .details()
            .and_then(|details| details.plan)
            .expect("a plan")
    }

    fn app_commands(&mut self) -> Vec<Command> {
        self.player.engine_mut().take()
    }

    fn native_commands(&self) -> Vec<Command> {
        self.native.take()
    }

    fn requests(&self) -> Vec<crate::server::PlanRequest> {
        self.server
            .state()
            .requests
            .iter()
            .map(|(_, request)| request.clone())
            .collect()
    }

    fn clear(&mut self) {
        self.app_commands();
        self.native_commands();
        self.server.take_reports();
        self.player.take_events();
    }
}

fn loads(commands: &[Command]) -> Vec<&crate::engine::EngineFile> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Load(file) => Some(file),
            _ => None,
        })
        .collect()
}

#[test]
fn a_file_the_native_engine_plays_starts_there_with_the_union_as_the_profile() {
    let mut two = Two::new(movie());
    block_on(two.player.play(item("e1"), MediaKind::MOVIE, secs(5)));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(
        two.plan().reasons,
        vec![RouteReason::NATIVE_FULLY_COMPATIBLE]
    );
    assert_eq!(loads(&two.native_commands()).len(), 1);
    assert!(loads(&two.app_commands()).is_empty());
    // The server is asked with what any engine plays: TrueHD (app engine) and the native engine's formats.
    let profile = two.server.state().profiles[0].clone();
    assert!(profile.decodes_audio(&Codec::TRUEHD));
    assert!(profile.subtitles.contains(&SubtitleFormat::DVDSUB));
    assert!(two.requests()[0].direct_play);
    // The app engine's events are not this file's.
    block_on(
        two.player
            .on_engine_event(EngineEvent::Ended(EndReason::Error(
                EngineErrorKind::DECODER_FAILED,
            ))),
    );
    assert_eq!(two.requests().len(), 1);
    assert_eq!(two.player.status().state, PlayerState::Loading);
}

#[test]
fn a_failed_start_tries_each_route_once_then_fails() {
    let mut two = Two::playing(movie());
    // The app engine after the native one, then a stream the native engine takes, then the app engine's, then the end.
    let queue = |two: &Two| {
        let source_answer = |offer| {
            let mut answer = answer(offer, vec![RouteReason::AUDIO_CODEC_NOT_SUPPORTED]);
            answer.source = movie();
            Ok(answer)
        };
        let mut direct = answer(current_model::ServerOffer::DirectPlay, vec![]);
        direct.source = movie();
        let server = two.server.clone();
        server.answer(Ok(direct.clone()));
        server.answer(Ok(direct));
        server.answer(source_answer(hls(
            StreamAction::Copy,
            StreamAction::Transcode,
        )));
        server.answer(source_answer(hls(
            StreamAction::Copy,
            StreamAction::Transcode,
        )));
    };
    queue(&two);
    two.at(100);
    two.clear();

    two.fail();
    let plan = two.plan();
    assert_eq!(plan.route, Route::AppEngine);
    assert_eq!(plan.reasons, vec![RouteReason::PREVIOUS_ROUTE_FAILED]);
    assert_eq!(plan.failed_routes, vec![Route::Native]);
    assert_eq!(plan.start, secs(100));
    assert_eq!(plan.tracks.audio, Some(1), "the same tracks");
    assert_eq!(two.native_commands(), vec![Command::Stop]);
    let app = two.app_commands();
    let app_loads = loads(&app);
    assert_eq!(app_loads.len(), 1);
    assert_eq!(app_loads[0].start, secs(100));
    assert_eq!(app_loads[0].audio, EngineTrack::Stream(1));
    assert!(two.requests()[1].direct_play);

    two.start();
    two.at(130);
    two.fail();
    // No local route is left: asked again without direct play, the server's stream on the native engine.
    let plan = two.plan();
    assert!(matches!(
        plan.route,
        Route::ServerStream {
            engine: EngineRole::Native,
            ..
        }
    ));
    assert_eq!(
        plan.reasons,
        vec![
            RouteReason::FALLBACK_AFTER_ENGINE_ERROR,
            RouteReason::AUDIO_CODEC_NOT_SUPPORTED
        ]
    );
    assert_eq!(plan.failed_routes, vec![Route::Native, Route::AppEngine]);
    assert_eq!(plan.start, secs(130));
    let requests = two.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].direct_play && !requests[3].direct_play);

    two.start();
    two.fail();
    assert!(matches!(
        two.plan().route,
        Route::ServerStream {
            engine: EngineRole::App,
            ..
        }
    ));
    assert!(!two.requests()[4].direct_play);

    two.start();
    two.fail();
    assert!(matches!(
        two.player.status().state,
        PlayerState::Failed { .. }
    ));
    assert_eq!(two.requests().len(), 5, "each route once");
}

#[test]
fn a_track_the_native_engine_cannot_play_after_opening_moves_on() {
    let mut two = Two::playing(movie());
    two.clear();
    two.event(EngineEvent::TrackFailed(EngineErrorKind::DECODER_FAILED));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().failed_routes, vec![Route::Native]);
}

#[test]
fn an_audio_track_the_native_engine_lacks_moves_the_file_at_the_position() {
    let mut two = Two::playing(movie());
    two.at(200);
    two.clear();
    // E-AC-3: the native engine plays it, so it switches there.
    block_on(two.player.select_audio(3));
    assert_eq!(
        two.native_commands(),
        vec![Command::Audio(EngineTrack::Stream(3))]
    );
    assert_eq!(two.requests().len(), 1);
    // TrueHD: the app engine, at the position, with that track.
    block_on(two.player.select_audio(2));
    let plan = two.plan();
    assert_eq!(plan.route, Route::AppEngine);
    assert_eq!(plan.reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(plan.start, secs(200));
    assert_eq!(two.native_commands(), vec![Command::Stop]);
    let app = two.app_commands();
    let file = loads(&app)[0].clone();
    assert_eq!(file.audio, EngineTrack::Stream(2));
    // Back to AAC: the app engine plays it too, so it stays.
    two.start();
    two.clear();
    block_on(two.player.select_audio(1));
    assert_eq!(
        two.app_commands(),
        vec![Command::Audio(EngineTrack::Stream(1))]
    );
    assert_eq!(two.plan().route, Route::AppEngine);
    // W7 §19: no reason about TrueHD is left; it says the file stayed although the native engine would play it now.
    assert_eq!(two.plan().reasons, vec![RouteReason::ROUTE_KEPT]);
    assert!(two.plan().native_lacks.is_empty());
    assert_eq!(two.plan().tracks.audio, Some(1));
}

#[test]
fn pgs_stays_native_and_a_dvd_subtitle_moves_to_the_app_engine() {
    let mut two = Two::playing(movie());
    two.clear();
    block_on(two.player.select_subtitle(SubtitleChoice::Stream(5)));
    assert_eq!(
        two.native_commands(),
        vec![Command::Subtitle(EngineTrack::Stream(5))]
    );
    assert_eq!(two.plan().route, Route::Native);
    block_on(two.player.select_subtitle(SubtitleChoice::Stream(6)));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(
        two.plan().reasons,
        vec![RouteReason::SUBTITLE_NEEDS_RENDERING]
    );
    assert_eq!(
        two.plan().tracks.subtitle,
        SubtitleChoice::Stream(6),
        "the subtitle kept"
    );
}

#[test]
fn a_speed_the_native_engine_ignores_moves_the_file_to_the_app_engine() {
    let mut two = Two::playing(movie());
    two.clear();
    // Rates work with these tracks: the native engine takes the speed.
    block_on(two.player.set_speed(1.5));
    assert_eq!(two.native_commands(), vec![Command::Speed(150)]);
    assert_eq!(two.plan().route, Route::Native);
    // With E-AC-3 Atmos selected Windows' player says it ignores rates: the app engine, keeping the track and the speed.
    block_on(two.player.select_audio(3));
    two.at(300);
    two.event(EngineEvent::RatesSupported(false));
    let plan = two.plan();
    assert_eq!(plan.route, Route::AppEngine);
    assert_eq!(plan.reasons, vec![RouteReason::SPEED_NOT_SUPPORTED]);
    assert_eq!(plan.tracks.audio, Some(3));
    assert_eq!(plan.start, secs(300));
    let app = two.app_commands();
    assert_eq!(loads(&app).len(), 1);
    assert!(app.contains(&Command::Speed(150)));
}

#[test]
fn at_normal_speed_an_ignored_rate_changes_nothing_until_a_speed_is_set() {
    let mut two = Two::playing(movie());
    two.clear();
    two.event(EngineEvent::RatesSupported(false));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(two.requests().len(), 1);
    block_on(two.player.set_speed(2.0));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::SPEED_NOT_SUPPORTED]);
}

#[test]
fn a_display_change_that_turns_departure_8_moves_the_file_both_ways() {
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HDR10));
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    two.start();
    assert_eq!(two.plan().route, Route::Native);
    two.at(60);
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::HDR_ON_SDR_DISPLAY]);
    assert_eq!(two.plan().start, secs(60));
    two.start();
    two.at(90);
    block_on(two.player.set_display(DisplayKind::Hdr));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(two.plan().start, secs(90));
}

#[test]
fn hlg_stays_native_when_the_display_turns_sdr() {
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HLG));
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    two.start();
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(two.requests().len(), 1);
}

#[test]
fn play_with_app_engine_moves_this_playback_only() {
    let mut two = Two::playing(movie());
    two.at(42);
    block_on(two.player.play_with_app_engine());
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::USER_CHOSE_ROUTE]);
    assert_eq!(two.plan().start, secs(42));
    // Asked again: nothing to do.
    block_on(two.player.play_with_app_engine());
    assert_eq!(two.requests().len(), 2);
    // The next item starts natively again.
    block_on(
        two.player
            .play(item("e2"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::Native);
}

#[test]
fn always_app_engine_plays_every_item_there() {
    let mut two = Two::new(movie());
    two.player.set_how_to_play(HowToPlay::AppEngine);
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::SETTING_CHOSE_ROUTE]);
    assert!(loads(&two.native_commands()).is_empty());
}

#[test]
fn volume_and_mute_reach_both_engines() {
    let mut two = Two::playing(movie());
    two.clear();
    two.player.set_volume(40);
    two.player.set_muted(true);
    let both = vec![Command::Volume(40), Command::Muted(true)];
    assert_eq!(two.native_commands(), both);
    assert_eq!(two.app_commands(), both);
}

#[test]
fn reports_name_the_play_method_and_the_item() {
    // The server is told direct play on the native route as on the app engine (ADR 0019's play method).
    let two = Two::playing(movie());
    let reports = two.server.take_reports();
    let start = reports
        .iter()
        .find_map(|(_, report)| match report {
            crate::test_support::Report::Start(start) => Some(start.clone()),
            _ => None,
        })
        .expect("a start report");
    assert_eq!(start.route, Route::Native);
    assert!(start.route.is_local());
    assert_eq!(start.item, item("e1"));
}

#[test]
fn the_player_setting_applies_from_the_next_item() {
    let mut two = Two::playing(movie());
    two.player.set_how_to_play(HowToPlay::AppEngine);
    // A re-plan of the item playing keeps the setting it started with.
    block_on(
        two.player
            .set_quality(crate::quality::Quality::Bitrate(4_000_000)),
    );
    assert_eq!(two.plan().route, Route::Native);
    block_on(
        two.player
            .play(item("e2"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::SETTING_CHOSE_ROUTE]);
}

#[test]
fn speed_reaches_both_engines_and_bad_rates_are_ignored() {
    let mut two = Two::playing(movie());
    two.clear();
    block_on(two.player.set_speed(1.25));
    assert_eq!(two.native_commands(), vec![Command::Speed(125)]);
    assert_eq!(two.app_commands(), vec![Command::Speed(125)]);
    for bad in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        block_on(two.player.set_speed(bad));
    }
    assert!(two.native_commands().is_empty() && two.app_commands().is_empty());
    // Kept within mpv's range.
    block_on(two.player.set_speed(500.0));
    assert_eq!(two.app_commands(), vec![Command::Speed(10_000)]);
    block_on(two.player.set_speed(0.001));
    assert_eq!(two.app_commands(), vec![Command::Speed(1)]);
}

#[test]
fn a_re_plan_while_paused_stays_paused() {
    let mut two = Two::playing(movie());
    block_on(two.player.pause());
    assert_eq!(two.player.status().state, PlayerState::Paused);
    two.clear();
    block_on(two.player.select_audio(2));
    assert_eq!(two.plan().route, Route::AppEngine);
    let app = two.app_commands();
    assert_eq!(loads(&app).len(), 1);
    assert_eq!(app.last(), Some(&Command::Paused(true)));
    two.start();
    assert_eq!(two.player.status().state, PlayerState::Paused);
    let reports = two.server.take_reports();
    assert!(reports.iter().any(|(_, report)| matches!(
        report,
        crate::test_support::Report::Start(start) if start.paused
    )));
}

#[test]
fn a_pause_and_a_resume_follow_the_file_to_either_engine() {
    // Paused natively, moved to libmpv (departure 8), resumed there, moved back: the native engine is told to play, and each load
    // on either engine carries the session's pause.
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HDR10));
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(two.native_commands().last(), Some(&Command::Paused(false)));
    two.start();
    block_on(two.player.pause());
    two.clear();
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.plan().route, Route::AppEngine);
    let app = two.app_commands();
    assert_eq!(loads(&app).len(), 1);
    assert_eq!(app.last(), Some(&Command::Paused(true)));
    two.start();
    block_on(two.player.resume());
    assert_eq!(two.app_commands(), vec![Command::Paused(false)]);
    assert_eq!(two.player.status().state, PlayerState::Playing);
    two.clear();
    block_on(two.player.set_display(DisplayKind::Hdr));
    assert_eq!(two.plan().route, Route::Native);
    let native = two.native_commands();
    assert_eq!(loads(&native).len(), 1);
    assert_eq!(native.last(), Some(&Command::Paused(false)));
    two.start();
    assert_eq!(two.player.status().state, PlayerState::Playing);
}

#[test]
fn a_native_engine_that_refuses_the_file_at_once_is_a_failed_route() {
    let mut two = Two::new(movie());
    two.native.0.lock().expect("fake engine").refuse_loads = true;
    block_on(two.player.play(item("e1"), MediaKind::MOVIE, secs(7)));
    let plan = two.plan();
    assert_eq!(plan.route, Route::AppEngine);
    assert_eq!(plan.reasons, vec![RouteReason::PREVIOUS_ROUTE_FAILED]);
    assert_eq!(plan.failed_routes, vec![Route::Native]);
    assert_eq!(plan.start, secs(7));
    assert_eq!(two.player.status().state, PlayerState::Loading);
}

#[test]
fn a_display_change_after_a_fallback_reloads_nothing() {
    // On an SDR display the app engine plays HDR10 first; it fails, so the native engine plays it. Display changes then leave it
    // there: the app engine failed in this play, so the native route stays first.
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HDR10));
    block_on(two.player.set_display(DisplayKind::Sdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::AppEngine);
    two.start();
    two.fail();
    assert_eq!(two.plan().route, Route::Native);
    two.start();
    let requests = two.requests().len();
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.requests().len(), requests, "no re-plan");
    assert_eq!(two.plan().route, Route::Native);
}

#[test]
fn a_server_stream_moves_back_to_the_native_engine_when_the_display_turns_hdr() {
    let source = movie_with("hevc", 10, VideoRangeType::HDR10);
    let mut two = Two::new(source.clone());
    let stream = || {
        let mut answer = answer(
            hls(StreamAction::Transcode, StreamAction::Transcode),
            vec![RouteReason::CONTAINER_BITRATE_EXCEEDS_LIMIT],
        );
        answer.source = source.clone();
        Ok(answer)
    };
    two.server.answer(stream());
    two.server.answer(stream());
    block_on(two.player.set_display(DisplayKind::Sdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.role(), EngineRole::App);
    two.start();
    block_on(two.player.set_display(DisplayKind::Hdr));
    assert!(matches!(
        two.plan().route,
        Route::ServerStream {
            engine: EngineRole::Native,
            ..
        }
    ));
}

#[test]
fn what_an_engine_said_of_rates_holds_for_a_reload_there_and_not_for_the_other_engine() {
    // A reload on the native engine keeps "rates ignored" until it reports again.
    let mut two = Two::playing(movie());
    two.event(EngineEvent::RatesSupported(false));
    block_on(
        two.player
            .set_quality(crate::quality::Quality::Bitrate(4_000_000)),
    );
    assert_eq!(two.plan().route, Route::Native);
    block_on(two.player.set_speed(1.5));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::SPEED_NOT_SUPPORTED]);

    // Moved to the other engine and back, the old report no longer holds.
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HDR10));
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    two.start();
    two.event(EngineEvent::RatesSupported(false));
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.role(), EngineRole::App);
    two.start();
    block_on(two.player.set_display(DisplayKind::Hdr));
    assert_eq!(two.role(), EngineRole::Native);
    two.start();
    block_on(two.player.set_speed(1.5));
    assert_eq!(two.plan().route, Route::Native);
}

#[test]
fn the_fallbacks_reason_stays_with_the_re_plan_it_caused() {
    let mut two = Two::playing(movie());
    two.fail();
    assert_eq!(two.plan().route, Route::AppEngine);
    two.start();
    // A later re-plan of the same item onto a server stream is the user's, not the failure's.
    let mut stream = answer(
        hls(StreamAction::Copy, StreamAction::Transcode),
        vec![RouteReason::AUDIO_CODEC_NOT_SUPPORTED],
    );
    stream.source = movie();
    two.server.answer(Ok(stream));
    block_on(
        two.player
            .set_quality(crate::quality::Quality::Bitrate(4_000_000)),
    );
    assert!(!two.plan().route.is_local());
    assert!(
        !two.plan()
            .reasons
            .contains(&RouteReason::FALLBACK_AFTER_ENGINE_ERROR)
    );
}

#[test]
fn a_stream_for_the_native_engine_always_goes_through_the_relay() {
    let mut two = Two::playing(movie());
    assert_eq!(two.server.state().always_relayed, vec![true]);
    // The relay's URL, with its route token only.
    let native = two.native_commands();
    let file = loads(&native)[0].clone();
    assert!(file.url.expose().starts_with("http://127.0.0.1:"));
    assert!(!file.url.expose().contains(crate::test_support::SECRETS[0]));
    // On the app engine, as before: the address decides.
    block_on(two.player.select_audio(2));
    assert_eq!(two.server.state().always_relayed, vec![true, false]);
}

#[test]
fn a_relay_that_cannot_start_for_the_native_engine_is_a_failed_route() {
    let mut two = Two::new(movie());
    two.server.state().relay_problem = Some(current_model::PlaybackProblem::LocalRelay);
    block_on(two.player.play(item("e1"), MediaKind::MOVIE, secs(4)));
    let plan = two.plan();
    assert_eq!(plan.route, Route::AppEngine);
    assert_eq!(plan.reasons, vec![RouteReason::PREVIOUS_ROUTE_FAILED]);
    assert_eq!(plan.failed_routes, vec![Route::Native]);
    assert_eq!(plan.start, secs(4));
    assert!(loads(&two.native_commands()).is_empty());
    assert_eq!(loads(&two.app_commands()).len(), 1);
    assert_eq!(two.player.status().state, PlayerState::Loading);
}

#[test]
fn a_failed_play_lets_the_engine_that_has_the_file_go() {
    let mut two = Two::playing(movie());
    two.clear();
    // The native engine cannot play a track; the re-plan's answer is an error, so the play fails.
    two.server
        .answer(Err(current_model::PlaybackProblem::NotFound));
    two.event(EngineEvent::TrackFailed(EngineErrorKind::DECODER_FAILED));
    assert!(matches!(
        two.player.status().state,
        PlayerState::Failed { .. }
    ));
    assert!(two.native_commands().contains(&Command::Stop));
}

#[test]
fn how_to_play_chosen_here_re_plans_at_once_and_auto_goes_back() {
    let mut two = Two::playing(movie());
    two.at(42);
    // The choices for this file: it plays natively and in the app engine; no remux on Windows; the server's by its permissions.
    assert_eq!(
        two.player.how_to_play_choices(),
        vec![
            HowToPlay::Auto,
            HowToPlay::Native,
            HowToPlay::AppEngine,
            HowToPlay::ServerRemux,
            HowToPlay::ServerTranscode
        ]
    );
    block_on(two.player.choose_how_to_play(HowToPlay::AppEngine));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.plan().reasons, vec![RouteReason::USER_CHOSE_ROUTE]);
    assert_eq!(two.plan().start, secs(42));
    // Auto again: Auto's route at once (manager, 2026-10-31).
    two.start();
    two.at(50);
    block_on(two.player.choose_how_to_play(HowToPlay::Auto));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(two.plan().start, secs(50));
    // Native chosen while native plays: no re-plan, the reason says it was chosen.
    let asked = two.requests().len();
    block_on(two.player.choose_how_to_play(HowToPlay::Native));
    assert_eq!(two.requests().len(), asked);
    assert_eq!(two.plan().reasons[0], RouteReason::USER_CHOSE_ROUTE);
    // The next item uses Settings' choice (Auto) again.
    block_on(
        two.player
            .play(item("e2"), MediaKind::MOVIE, Duration::ZERO),
    );
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(
        two.plan().reasons,
        vec![RouteReason::NATIVE_FULLY_COMPATIBLE]
    );
}

#[test]
fn how_to_play_at_the_servers_transcode_asks_for_it_without_direct_play() {
    let mut two = Two::playing(movie());
    two.at(30);
    let mut stream = answer(hls(StreamAction::Transcode, StreamAction::Copy), vec![]);
    stream.source = movie();
    two.server.answer(Ok(stream));
    block_on(two.player.choose_how_to_play(HowToPlay::ServerTranscode));
    let request = two.requests().pop().expect("a request");
    assert!(!request.direct_play);
    assert!(request.force_video_transcode);
    assert!(matches!(two.plan().route, Route::ServerStream { .. }));
    assert_eq!(two.plan().reasons[0], RouteReason::USER_CHOSE_ROUTE);
}

#[test]
fn the_servers_remux_chosen_that_must_convert_says_it_fell() {
    let mut two = Two::playing(movie());
    let mut stream = answer(
        hls(StreamAction::Copy, StreamAction::Transcode),
        vec![RouteReason::AUDIO_CODEC_NOT_SUPPORTED],
    );
    stream.source = movie();
    two.server.answer(Ok(stream));
    block_on(two.player.choose_how_to_play(HowToPlay::ServerRemux));
    let request = two.requests().pop().expect("a request");
    assert!(!request.direct_play && !request.force_video_transcode);
    assert_eq!(
        two.plan().reasons,
        vec![
            RouteReason::CHOSEN_ROUTE_UNAVAILABLE,
            RouteReason::AUDIO_CODEC_NOT_SUPPORTED
        ]
    );
}

#[test]
fn native_chosen_never_moves_back_up_on_a_display_change_and_moves_down_when_it_must() {
    let mut two = Two::new(movie_with("hevc", 10, VideoRangeType::HDR10));
    two.player.set_how_to_play(HowToPlay::Native);
    block_on(two.player.set_display(DisplayKind::Hdr));
    block_on(
        two.player
            .play(item("e1"), MediaKind::MOVIE, Duration::ZERO),
    );
    two.start();
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(
        two.plan().reasons,
        vec![
            RouteReason::SETTING_CHOSE_ROUTE,
            RouteReason::NATIVE_FULLY_COMPATIBLE
        ]
    );
    // SDR: native can't play PQ here, so down to the app engine, saying why.
    two.at(60);
    block_on(two.player.set_display(DisplayKind::Sdr));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(
        two.plan().reasons,
        vec![
            RouteReason::CHOSEN_ROUTE_UNAVAILABLE,
            RouteReason::HDR_ON_SDR_DISPLAY
        ]
    );
    // HDR again: Auto would move back to native; a choice never moves back up.
    two.start();
    two.at(90);
    let asked = two.requests().len();
    block_on(two.player.set_display(DisplayKind::Hdr));
    assert_eq!(two.plan().route, Route::AppEngine);
    assert_eq!(two.requests().len(), asked);
}

#[test]
fn a_native_engine_that_never_plays_a_stream_loaded_while_paused_is_a_failed_route() {
    // W7 §8: paused natively, a lower quality loads the server's stream on Windows' player, still paused; on resume the engine
    // buffers and never plays. After the stall timeout the file moves on to the app engine's stream at the position, as after the
    // engine's own error, instead of failing as a network problem.
    let mut two = Two::playing(movie());
    let stream = || {
        let mut answer = answer(
            hls(StreamAction::Transcode, StreamAction::Transcode),
            vec![RouteReason::CONTAINER_BITRATE_EXCEEDS_LIMIT],
        );
        answer.source = movie();
        Ok(answer)
    };
    two.server.answer(stream());
    two.server.answer(stream());
    two.at(4);
    block_on(two.player.pause());
    block_on(
        two.player
            .set_quality(crate::quality::Quality::Bitrate(4_000_000)),
    );
    let native_stream = two.plan().route;
    assert!(matches!(
        native_stream,
        Route::ServerStream {
            engine: EngineRole::Native,
            ..
        }
    ));
    two.start();
    assert_eq!(two.player.status().state, PlayerState::Paused);
    // Paused, nothing is expected to move: no stall however long.
    two.wait(secs(600));
    assert_eq!(two.player.status().state, PlayerState::Paused);
    two.clear();

    block_on(two.player.resume());
    assert_eq!(two.native_commands(), vec![Command::Paused(false)]);
    two.event(EngineEvent::Buffering(true));
    two.wait(STALL_TIMEOUT - secs(1));
    assert_eq!(two.player.status().state, PlayerState::Buffering);
    two.wait(secs(1));

    let plan = two.plan();
    assert!(matches!(
        plan.route,
        Route::ServerStream {
            engine: EngineRole::App,
            ..
        }
    ));
    assert_eq!(plan.failed_routes, vec![native_stream]);
    assert!(
        plan.reasons
            .contains(&RouteReason::FALLBACK_AFTER_ENGINE_ERROR)
    );
    assert_eq!(plan.start, secs(4));
    assert!(two.native_commands().contains(&Command::Stop));
    let app = two.app_commands();
    assert_eq!(loads(&app).len(), 1);
    assert_eq!(
        app.last(),
        Some(&Command::Paused(false)),
        "playing, as the user asked"
    );
    assert_eq!(two.player.status().state, PlayerState::Loading);
    let stops: Vec<_> = two
        .server
        .take_reports()
        .into_iter()
        .filter_map(|(_, report)| match report {
            crate::test_support::Report::Stopped(stop) => Some(stop),
            _ => None,
        })
        .collect();
    assert!(
        stops.iter().all(|stop| !stop.failed),
        "a moved file is not a failed play: {stops:?}"
    );

    // The app engine's stream is the last route: its stall fails as before.
    two.start();
    two.event(EngineEvent::Buffering(true));
    two.wait(STALL_TIMEOUT);
    assert_eq!(
        two.player.status().state,
        PlayerState::Failed {
            problem: current_model::PlaybackProblem::Network
        }
    );
}

#[test]
fn a_track_change_under_a_choice_keeps_saying_it_was_chosen() {
    let mut two = Two::playing(movie());
    two.at(10);
    block_on(two.player.choose_how_to_play(HowToPlay::Native));
    assert_eq!(two.plan().reasons[0], RouteReason::USER_CHOSE_ROUTE);
    // E-AC-3: native plays it too, so the route stays and is re-checked for the new track.
    block_on(two.player.select_audio(3));
    assert_eq!(two.plan().route, Route::Native);
    assert_eq!(
        two.plan().reasons,
        vec![
            RouteReason::USER_CHOSE_ROUTE,
            RouteReason::NATIVE_FULLY_COMPATIBLE
        ]
    );
}
