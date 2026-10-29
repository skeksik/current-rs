//! The player through `App`, with the test engine, against the Jellyfin stand-in on 127.0.0.1: start and reports, the state and the
//! media controls, the swap chain and its release, cancellation, one engine per process, the preferred quality, automatic skipping,
//! and that no URL or token shows in anything the player publishes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use current_model::{ItemId, ItemRef, PlaybackProblem, PlayerState, RouteLabel, ServerRef};
use current_playback::{EngineEvent, PlayerStatus, Quality};
use current_store::{MemorySecrets, Secrets, StreamQuality};
use jellyfin_stand_in::library::EPISODE_IDS;
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, DIRECT_PLAY_NAME, IMAGE, NATIVE_ITEM, NATIVE_MEDIA, PASSWORD, StandIn,
    TRANSCODE_ITEM, USER_NAME,
};
use tokio::sync::watch;

use super::test_engine::{
    self, DURATION, SWAP_CHAIN_HANDLE, TestCommand, TestEngineControl, last_control,
};
use super::{CompositionSize, DisplayFacts, MediaStatus, Player, PlayerError};
use crate::{AddServerState, App, SignInState};

/// One engine per process: the tests that open a player take turns.
static ENGINE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SIZE: CompositionSize = CompositionSize {
    width: 1920,
    height: 1080,
};

fn display() -> DisplayFacts {
    DisplayFacts {
        hdr: false,
        peak_nits: 300.0,
        sdr_white_nits: 200.0,
        refresh_hz: 60.0,
    }
}

/// Waits (at most 5 s) until `check` passes on the value `receiver` holds.
async fn until<T: Clone>(receiver: &watch::Receiver<T>, mut check: impl FnMut(&T) -> bool) -> T {
    let mut receiver = receiver.clone();
    let wait = receiver.wait_for(|value| check(value));
    match tokio::time::timeout(Duration::from_secs(5), wait).await {
        Ok(Ok(value)) => value.clone(),
        _ => panic!(
            "condition not met within 5 s: {:?}",
            std::any::type_name::<T>()
        ),
    }
}

async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 5 s");
}

async fn signed_in(stand_in: &StandIn, app: &App) -> ServerRef {
    let setup = app.add_server();
    setup.probe(stand_in.address()).await;
    let found = match setup.state().borrow().clone() {
        AddServerState::Ready { server } => server,
        state => panic!("{state:?}"),
    };
    let flow = app.sign_in(found).unwrap();
    flow.with_password(USER_NAME, PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
    eventually(|| app.capabilities().borrow().servers[0].address.is_some()).await;
    app.capabilities().borrow().servers[0].server.clone()
}

fn item(server: &ServerRef, id: &str) -> ItemRef {
    ItemRef::new(server.clone(), ItemId::new(id))
}

async fn open(app: &App) -> (Player, TestEngineControl) {
    let player = app.open_player(SIZE, display()).await.unwrap();
    let control = last_control().unwrap();
    (player, control)
}

fn playing(status: &PlayerStatus) -> bool {
    status.state == PlayerState::Playing
}

fn reports(stand_in: &StandIn) -> Vec<String> {
    stand_in
        .reports()
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

#[tokio::test]
async fn a_direct_play_starts_reports_and_closes_in_order() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, control) = open(&app).await;

    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    let status = until(&player.state(), playing).await;
    assert_eq!(status.item, Some(item(&server, DIRECT_PLAY_ITEM)));
    assert_eq!(status.duration, Some(DURATION));
    assert_eq!(
        status.route.as_ref().map(|route| route.label()),
        Some(RouteLabel::AppEngine)
    );
    assert_eq!(status.audio_tracks.len(), 1);
    eventually(|| reports(&stand_in) == ["/Sessions/Playing"]).await;
    let commands = control.commands();
    assert_eq!(
        commands[..2],
        [
            TestCommand::CompositionSize {
                width: 1920,
                height: 1080
            },
            TestCommand::DisplayFacts
        ]
    );
    assert!(commands.iter().any(|command| matches!(
        command,
        TestCommand::Load {
            secrets: 1,
            references: true,
            start: Duration::ZERO,
            audio: current_playback::EngineTrack::Stream(1),
            ..
        }
    )));

    // The media controls: title, status, timeline, and the artwork (the placeholder first, then the server's image).
    let now = until(&player.now_playing(), |now| {
        now.status == MediaStatus::Playing && now.artwork.as_deref() == Some(IMAGE)
    })
    .await;
    assert_eq!(now.title.as_deref(), Some(DIRECT_PLAY_NAME));
    assert_eq!(now.duration, Some(DURATION));
    assert!(now.can_pause && now.can_seek && now.can_stop && !now.can_next);

    // The position as the engine reports it, a pause with its report, a seek with a new timeline.
    control.send(EngineEvent::Position(Duration::from_secs(42)));
    until(&player.state(), |status| {
        status.position == Duration::from_secs(42)
    })
    .await;
    player.pause().await.unwrap();
    until(&player.state(), |status| {
        status.state == PlayerState::Paused
    })
    .await;
    eventually(|| reports(&stand_in).contains(&"/Sessions/Playing/Progress".to_owned())).await;
    let before = player.now_playing().borrow().timeline_revision;
    player.seek_to(Duration::from_secs(120)).await.unwrap();
    let now = until(&player.now_playing(), |now| {
        now.position == Duration::from_secs(120)
            && now.timeline_revision > before
            && now.status == MediaStatus::Paused
    })
    .await;
    assert!(now.can_play);

    // The swap chain: one handle, given back by the UI.
    let swap_chain = until(&player.swap_chain(), Option::is_some).await.unwrap();
    assert_eq!(swap_chain.handle(), SWAP_CHAIN_HANDLE);
    assert_eq!(swap_chain.generation(), 1);
    swap_chain.release();
    swap_chain.release();
    assert!(swap_chain.is_released());
    assert_eq!(control.swap_chain_releases(), 1);

    // Closing reports the stop, then shuts the engine down.
    player.close().await;
    assert_eq!(
        reports(&stand_in).last().map(String::as_str),
        Some("/Sessions/Playing/Stopped")
    );
    assert_eq!(control.commands().last(), Some(&TestCommand::Shutdown));
    assert!(player.swap_chain().borrow().is_none());
    assert_eq!(player.state().borrow().state, PlayerState::Idle);
    assert!(matches!(player.pause().await, Err(PlayerError::Closed)));
}

#[tokio::test]
async fn nothing_the_player_publishes_holds_a_url_or_the_token() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, control) = open(&app).await;
    player
        .play(item(&server, TRANSCODE_ITEM), Duration::from_secs(30))
        .await
        .unwrap();
    let status = until(&player.state(), playing).await;
    assert_eq!(
        status.route.as_ref().map(|route| route.label()),
        Some(RouteLabel::ServerRemux)
    );
    let info = player.playback_info().await.unwrap();
    assert!(info.details.unwrap().transcoding.is_some());
    let url = control.loaded_urls().pop().unwrap();
    let token = url
        .split("ApiKey=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();
    assert!(!token.is_empty());
    let host = stand_in.address().trim_start_matches("http://").to_owned();
    let published = format!(
        "{:?} {:?} {:?} {:?}",
        player.state().borrow().clone(),
        player.now_playing().borrow().clone(),
        player.playback_info().await.unwrap(),
        player.swap_chain().borrow().clone(),
    );
    for secret in [
        token.as_str(),
        host.as_str(),
        "ApiKey",
        "master.m3u8",
        "http",
    ] {
        assert!(!published.contains(secret), "{secret} in {published}");
    }
    player.close().await;
}

#[tokio::test]
async fn a_cancelled_play_leaves_nothing_started() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, control) = open(&app).await;
    let path = format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo");
    stand_in.hold(&path);
    let play = player.play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), play)
            .await
            .is_err()
    );
    until(&player.state(), |status| status.state == PlayerState::Idle).await;
    stand_in.release(&path);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !control
            .commands()
            .iter()
            .any(|command| matches!(command, TestCommand::Load { .. }))
    );
    // The next play works.
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    player.close().await;
}

#[tokio::test]
async fn one_engine_per_process_and_a_signed_out_item_is_refused() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, _) = open(&app).await;
    assert_eq!(
        app.open_player(SIZE, display()).await.unwrap_err(),
        PlayerError::Busy
    );
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    // An item of a server that is not signed in: refused, the item playing stays shown, and its id goes to no server.
    let other = ServerRef::new(server.service.clone(), current_model::ServerKey::new("99"));
    let unknown = "57a2d1e000000000000000000000ee01";
    assert_eq!(
        player.play(item(&other, unknown), Duration::ZERO).await,
        Err(PlayerError::NotSignedIn)
    );
    let status = until(&player.state(), |status| {
        status.state != PlayerState::Preparing
    })
    .await;
    assert_eq!(status.state, PlayerState::Playing);
    assert_eq!(status.item, Some(item(&server, DIRECT_PLAY_ITEM)));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !stand_in.paths().iter().any(|path| path.contains(unknown)),
        "{:?}",
        stand_in.paths()
    );
    player.close().await;
    // Closed: the engine is free again.
    let (again, _) = open(&app).await;
    again.close().await;
}

#[tokio::test]
async fn an_action_a_stop_replaces_answers_replaced_not_closed() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, _) = open(&app).await;
    let path = format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo");

    // A play held at the plan, then a stop.
    stand_in.hold(&path);
    let stop_later = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        player.stop().await
    };
    let (play, stop) = tokio::join!(
        player.play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO),
        stop_later
    );
    assert_eq!(play, Err(PlayerError::Replaced));
    assert_eq!(stop, Ok(()));
    stand_in.release(&path);
    until(&player.state(), |status| status.state == PlayerState::Idle).await;

    // A quality change re-plans; held there, a stop replaces it too.
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    stand_in.hold(&path);
    let stop_later = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        player.stop().await
    };
    let (quality, stop) = tokio::join!(player.set_quality(Quality::Bitrate(2_000_000)), stop_later);
    assert_eq!(quality, Err(PlayerError::Replaced));
    assert_eq!(stop, Ok(()));
    stand_in.release(&path);
    // The player goes on: the next play works.
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    player.close().await;
}

#[tokio::test]
async fn after_the_autoplay_countdown_the_media_controls_show_the_next_episode() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, control) = open(&app).await;
    player
        .play(item(&server, EPISODE_IDS[0]), Duration::ZERO)
        .await
        .unwrap();
    until(&player.now_playing(), |now| {
        now.title.as_deref() == Some("Example Episode One")
    })
    .await;
    // The end of the file starts the 10 s countdown (the user's autoplay is on), then the next episode.
    control.send(EngineEvent::Ended(current_playback::EndReason::Eof));
    let mut now = player.now_playing();
    let next = tokio::time::timeout(
        Duration::from_secs(20),
        now.wait_for(|now| {
            now.item == Some(item(&server, EPISODE_IDS[1]))
                && now.title.as_deref() == Some("Example Episode Two")
        }),
    )
    .await
    .expect("the next episode within 20 s")
    .unwrap()
    .clone();
    assert_eq!(next.episode, Some(2));
    player.close().await;
}

/// Case 7 of the owner's run (2026-10-26): finishing an episode brings up Next up on Home without a refresh. The stand-in, as Jellyfin,
/// lists a series in Next up only once one of its episodes was played (`NextUpService.GetNextUpSeriesKeys`), and takes a stop report
/// at the end as played (`UserDataManager.UpdatePlayState`).
#[tokio::test]
async fn finishing_an_episode_brings_up_next_up_on_home() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let home = browse.home();
    eventually(|| {
        home.row(&format!(
            "latest:{}",
            jellyfin_stand_in::library::SHOWS_LIBRARY_ID
        ))
        .is_some()
            && !home.state().borrow().loading
    })
    .await;
    assert!(home.row("next_up").is_none(), "nothing started yet");

    let (player, control) = open(&app).await;
    player
        .play(item(&server, EPISODE_IDS[0]), Duration::ZERO)
        .await
        .unwrap();
    until(&player.now_playing(), |now| {
        now.title.as_deref() == Some("Example Episode One")
    })
    .await;
    control.send(EngineEvent::Ended(current_playback::EndReason::Eof));
    eventually(|| {
        home.row("next_up").is_some_and(|row| {
            row.snapshot()
                .iter()
                .any(|episode| episode.item == item(&server, EPISODE_IDS[1]))
        })
    })
    .await;
    player.close().await;
}

#[tokio::test]
async fn an_item_the_server_cannot_find_fails_with_its_problem() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, _) = open(&app).await;
    player
        .play(
            item(&server, "57a2d1e00000000000000000000000ff"),
            Duration::ZERO,
        )
        .await
        .unwrap();
    let status = until(&player.state(), |status| {
        matches!(status.state, PlayerState::Failed { .. })
    })
    .await;
    assert_eq!(
        status.state,
        PlayerState::Failed {
            problem: PlaybackProblem::NotFound
        }
    );
    assert_eq!(player.now_playing().borrow().status, MediaStatus::Stopped);
    player.close().await;
}

fn open_stored(dir: &Path) -> App {
    let secrets: Arc<dyn Secrets> = Arc::new(MemorySecrets::new());
    App::open(&dir.join("data"), &dir.join("cache"), secrets, "Office PC")
        .unwrap()
        .0
}

#[tokio::test]
async fn items_play_at_the_preferred_quality_and_a_picked_one_is_not_saved() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let app = open_stored(temp.path());
    let server = signed_in(&stand_in, &app).await;
    // 1.2 Mbit/s is no step Settings lists: it is stored as the nearest listed one not above it, 1 Mbit/s, which the stand-in's menu
    // has below its 1.4 Mbit/s original (480p).
    app.set_preferred_quality(Quality::Bitrate(1_200_000))
        .unwrap();
    assert_eq!(app.preferred_quality(), Quality::Bitrate(1_000_000));
    let (player, _) = open(&app).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    // That entry, which the menu ticks.
    let status = until(&player.state(), playing).await;
    assert_eq!(status.quality, Quality::Bitrate(1_000_000));
    // A pick re-plans at the position and changes only the item playing.
    player.set_quality(Quality::Maximum).await.unwrap();
    until(&player.state(), |status| {
        playing(status) && status.quality == Quality::Maximum
    })
    .await;
    let settings = || app.settings().unwrap().get().playback;
    assert_eq!(
        settings().quality_preferred,
        StreamQuality::Bitrate(1_000_000)
    );
    assert_eq!(settings().quality_local, StreamQuality::Maximum);
    assert_eq!(settings().quality_internet, StreamQuality::Maximum);
    // The next item plays at the preferred quality again.
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), |status| {
        playing(status) && status.quality == Quality::Bitrate(1_000_000)
    })
    .await;
    player.close().await;
    drop(app);
    // Read again at the next start.
    let app = open_stored(temp.path());
    assert_eq!(app.preferred_quality(), Quality::Bitrate(1_000_000));
}

#[tokio::test]
async fn automatic_skipping_follows_the_setting_at_once() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let (player, control) = open(&app).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    // Off by default: the intro (5 s to 35 s) shows its skip button.
    control.send(EngineEvent::Position(Duration::from_secs(6)));
    let status = until(&player.state(), |status| status.segment.is_some()).await;
    assert_eq!(status.position, Duration::from_secs(6));
    assert!(
        !control
            .commands()
            .contains(&TestCommand::Seek(Duration::from_secs(35)))
    );

    app.set_auto_skip(vec!["intro".into(), "outro".into()])
        .unwrap();
    assert_eq!(*app.auto_skip().borrow(), ["intro", "outro"]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    control.send(EngineEvent::Position(Duration::from_secs(7)));
    eventually(|| {
        control
            .commands()
            .contains(&TestCommand::Seek(Duration::from_secs(35)))
    })
    .await;
    player.close().await;
}

#[test]
fn the_engine_of_this_build_is_the_test_engine() {
    assert_eq!(super::player_engine(), super::PlayerEngine::Test);
    assert_eq!(&test_engine::MARKER[..19], b"current-test-engine");
}

/// For a test outside a player: the test engine records what libmpv's mapping is given.
#[test]
fn the_test_engine_records_a_load_without_its_url() {
    use current_playback::{Engine, EngineFile, EngineTrack, StreamAuth, StreamUrl};
    let (events, _raw) = tokio::sync::mpsc::unbounded_channel();
    // Not through `open`: no player, so the process's slot is taken only briefly.
    let guard = ENGINE.blocking_lock();
    let mut engine = test_engine::TestBackend::start(events).unwrap();
    let control = last_control().unwrap();
    engine
        .load(EngineFile {
            url: StreamUrl::new("http://127.0.0.1:50123/route/Videos/1/stream?static=true"),
            auth: StreamAuth {
                secrets: vec!["route".into()],
            },
            start: Duration::ZERO,
            audio: EngineTrack::Auto,
            subtitle: EngineTrack::Auto,
            references: false,
            hls: false,
        })
        .unwrap();
    assert_eq!(
        control.commands(),
        [TestCommand::Load {
            secrets: 1,
            references: false,
            start: Duration::ZERO,
            audio: EngineTrack::Auto,
            subtitle: EngineTrack::Auto,
        }]
    );
    drop(engine);
    drop(guard);
}

#[tokio::test]
async fn a_player_start_keeps_its_engines_capabilities_for_settings() {
    let _engine = ENGINE.lock().await;
    let app = App::new();
    assert_eq!(app.engine_capabilities.known(), None);
    let (player, _) = open(&app).await;
    assert_eq!(
        app.engine_capabilities.known(),
        Some(super::test_engine::capabilities())
    );
    player.close().await;
}

/// D6, an upstream Jellyfin bug: after its series was unmarked, playing an episode marked on its own before must not make it played
/// again on the server. Jellyfin saves the start report with the user data its cached instance of the episode still holds, which the
/// series' sweep left played (the stand-in models it: `Marks::write_back`).
#[tokio::test]
async fn playing_an_episode_after_a_series_unmark_keeps_it_unplayed_on_the_server() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let series_id = jellyfin_stand_in::library::SERIES_ID;
    let series = browse.details(&item(&server, series_id));
    eventually(|| series.episodes().len() == 2 && !series.state().borrow().loading).await;
    let settle =
        || tokio::time::sleep(crate::browse::DEPENDENT_RELOAD_DELAY + Duration::from_millis(700));
    browse.set_played(&item(&server, EPISODE_IDS[0]), true);
    settle().await;
    browse.set_played(&item(&server, series_id), true);
    settle().await;
    browse.set_played(&item(&server, series_id), false);
    settle().await;
    assert!(!series.episodes().snapshot()[0].user.played);

    let (player, _control) = open(&app).await;
    player
        .play(item(&server, EPISODE_IDS[0]), Duration::ZERO)
        .await
        .unwrap();
    eventually(|| reports(&stand_in).contains(&"/Sessions/Playing".to_owned())).await;
    player.close().await;
    settle().await;
    // A list read: what the server's database holds now.
    series.refresh();
    settle().await;
    assert!(
        !series.episodes().snapshot()[0].user.played,
        "the sweep's unmark stays on the server"
    );
}

// ---- A platform engine beside the test engine (ADR 0025, W3) ----

use std::sync::Mutex as StdMutex;

use super::{
    CodecAnswer, PlatformEngine, PlatformError, PlatformEvents, PlatformFile, PlatformSubtitle,
};
use current_model::EngineErrorKind;
use current_playback::{EndReason, EngineTrack};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A platform engine that records what it was asked and answers through its events object when the test says so.
#[derive(Default)]
struct FakePlatform {
    events: StdMutex<Option<PlatformEvents>>,
    loads: StdMutex<Vec<PlatformFile>>,
    commands: StdMutex<Vec<String>>,
    /// Every call throws (a foreign exception).
    throws: bool,
}

impl FakePlatform {
    fn last(&self) -> PlatformFile {
        self.loads.lock().unwrap().last().cloned().expect("a load")
    }

    fn send(&self, generation: u64, event: EngineEvent) {
        self.events
            .lock()
            .unwrap()
            .as_ref()
            .expect("attached")
            .send(generation, event);
    }

    /// Opens and starts the last load, from another thread, as a platform's own threads do.
    fn start_from_another_thread(&self) {
        let generation = self.last().generation;
        let events = self.events.lock().unwrap().clone().expect("attached");
        std::thread::spawn(move || {
            for event in [
                EngineEvent::Loaded,
                EngineEvent::Duration(DURATION),
                EngineEvent::Restarted,
            ] {
                events.send(generation, event);
            }
        })
        .join()
        .unwrap();
    }

    fn record(&self, command: String) -> Result<(), PlatformError> {
        self.commands.lock().unwrap().push(command);
        if self.throws {
            Err(PlatformError::Unexpected)
        } else {
            Ok(())
        }
    }
}

impl PlatformEngine for FakePlatform {
    fn attach(&self, events: PlatformEvents) -> Result<(), PlatformError> {
        *self.events.lock().unwrap() = Some(events);
        Ok(())
    }
    fn load(&self, file: PlatformFile) -> Result<(), PlatformError> {
        self.loads.lock().unwrap().push(file);
        self.record("load".into())
    }
    fn add_subtitle(&self, subtitle: PlatformSubtitle) -> Result<(), PlatformError> {
        self.record(format!("subtitle {}", subtitle.index))
    }
    fn set_paused(&self, paused: bool) -> Result<(), PlatformError> {
        self.record(format!("paused {paused}"))
    }
    fn seek_to(&self, position: Duration) -> Result<(), PlatformError> {
        self.record(format!("seek {}", position.as_secs()))
    }
    fn select_audio(&self, track: EngineTrack) -> Result<(), PlatformError> {
        self.record(format!("audio {track:?}"))
    }
    fn select_subtitle(&self, track: EngineTrack) -> Result<(), PlatformError> {
        self.record(format!("subtitles {track:?}"))
    }
    fn set_volume(&self, volume: u8) -> Result<(), PlatformError> {
        self.record(format!("volume {volume}"))
    }
    fn set_muted(&self, muted: bool) -> Result<(), PlatformError> {
        self.record(format!("muted {muted}"))
    }
    fn set_speed(&self, speed: f64) -> Result<(), PlatformError> {
        self.record(format!("speed {speed}"))
    }
    fn stop(&self) -> Result<(), PlatformError> {
        self.record("stop".into())
    }
}

/// Windows with the HEVC extension (or without it).
fn windows_answers(hevc: bool) -> Vec<CodecAnswer> {
    super::windows_codec_queries()
        .into_iter()
        .map(|query| CodecAnswer {
            decoders: u32::from(
                ["VideoFormatH264", "AudioFormatAac"].contains(&query.subtype.as_str())
                    || (hevc && query.subtype == "VideoFormatHevc"),
            ),
            subtype: query.subtype,
        })
        .collect()
}

async fn open_with(
    app: &App,
    platform: Arc<FakePlatform>,
    hevc: bool,
) -> (Player, TestEngineControl) {
    let player = app
        .open_player_with_platform_engine(SIZE, display(), platform, &windows_answers(hevc))
        .await
        .unwrap();
    let control = last_control().unwrap();
    (player, control)
}

/// The status of a GET for `url` on the relay; 0 when nothing listens there.
async fn relay_status(url: &str) -> u16 {
    let rest = url.strip_prefix("http://127.0.0.1:").expect("a relay URL");
    let (port, path) = rest.split_at(rest.find('/').expect("a path"));
    let Ok(mut stream) =
        tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).await
    else {
        return 0;
    };
    // The relay takes only its own host and port (ADR 0020).
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: bytes=0-1\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = vec![0; 12];
    stream.read_exact(&mut head).await.unwrap();
    String::from_utf8_lossy(&head[9..12]).parse().unwrap()
}

/// A GET for `url` on the relay with `range`: the status and the body.
async fn relay_get(url: &str, range: &str) -> (u16, Vec<u8>) {
    let rest = url.strip_prefix("http://127.0.0.1:").expect("a relay URL");
    let (port, path) = rest.split_at(rest.find('/').expect("a path"));
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
        .await
        .unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: {range}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    let end = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8_lossy(&answer[..end]).into_owned();
    // A chunked answer is not expected for a file with a length.
    assert!(!head.to_ascii_lowercase().contains("chunked"), "{head}");
    (head[9..12].parse().unwrap(), answer[end + 4..].to_vec())
}

fn loads(control: &TestEngineControl) -> usize {
    control
        .commands()
        .iter()
        .filter(|command| matches!(command, TestCommand::Load { .. }))
        .count()
}

#[tokio::test]
async fn a_file_the_platform_engine_plays_goes_there_through_the_relay() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let platform = Arc::new(FakePlatform::default());
    let (player, control) = open_with(&app, platform.clone(), true).await;

    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::from_secs(5))
        .await
        .unwrap();
    // The stand-in's address has no gateway; the platform engine's stream is relayed all the same, with the route's token only.
    let file = platform.last();
    let url = file.url.expose().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"), "a relay URL");
    let token = stand_in.live_token().expect("signed in");
    assert!(!url.contains(&token), "no session token across the bridge");
    assert_eq!(file.start, Duration::from_secs(5));
    assert_eq!(file.audio, EngineTrack::Stream(1));
    assert_eq!(relay_status(&url).await, 206);
    assert_eq!(loads(&control), 0, "libmpv has nothing");

    // Events from the platform's own thread reach the state.
    platform.start_from_another_thread();
    let status = until(&player.state(), playing).await;
    assert_eq!(
        status.route.as_ref().map(|route| route.label()),
        Some(RouteLabel::Native)
    );
    let info = player.playback_info().await.unwrap();
    assert!(info.platform_engine);
    let lines = crate::info_panel_lines(
        &info,
        None,
        &super::PanelSizes::default(),
        &crate::numbers::NumberFormat::default(),
    );
    assert_eq!(
        lines[..2],
        ["Native", "• Native playback is fully compatible."]
    );
    // Nothing the player publishes holds the relay URL.
    let shown = format!("{status:?} {info:?}");
    assert!(
        !shown.contains(&url) && !shown.contains("127.0.0.1"),
        "{shown}"
    );

    // An event of an older load is dropped.
    platform.send(
        file.generation - 1,
        EngineEvent::Ended(EndReason::Error(EngineErrorKind::DECODER_FAILED)),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(playing(&player.state().borrow()));

    // A track the platform cannot play after all: libmpv takes over at the position.
    platform.send(
        file.generation,
        EngineEvent::Position(Duration::from_secs(9)),
    );
    platform.send(
        file.generation,
        EngineEvent::TrackFailed(EngineErrorKind::DECODER_FAILED),
    );
    eventually(|| loads(&control) == 1).await;
    assert!(
        platform
            .commands
            .lock()
            .unwrap()
            .contains(&"stop".to_owned())
    );
    let status = player.state().borrow().clone();
    assert_eq!(
        status.route.as_ref().map(|route| route.label()),
        Some(RouteLabel::AppEngine)
    );
    // The platform engine's route is gone with it (refused, or the relay no longer listens), and its old events open nothing.
    assert!(matches!(relay_status(&url).await, 0 | 404));
    platform.send(file.generation, EngineEvent::Loaded);
    platform.send(file.generation, EngineEvent::Restarted);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(platform.loads.lock().unwrap().len(), 1);
    assert!(matches!(relay_status(&url).await, 0 | 404));
    player.close().await;
}

#[tokio::test]
async fn the_stand_ins_native_file_goes_to_the_platform_engine_with_ranges() {
    // What the Windows smoke run opens in Windows' own player (ADR 0025, W4): H.264 and AAC in MP4, native without any Store
    // extension, and the relay answers Media Foundation's range requests with the file's own bytes.
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let platform = Arc::new(FakePlatform::default());
    let (player, control) = open_with(&app, platform.clone(), false).await;
    player
        .play(item(&server, NATIVE_ITEM), Duration::ZERO)
        .await
        .unwrap();
    let file = platform.last();
    let url = file.url.expose().to_owned();
    assert!(!file.hls);
    assert_eq!(loads(&control), 0, "libmpv has nothing");
    let (status, body) = relay_get(&url, "bytes=0-").await;
    assert_eq!(status, 206);
    assert_eq!(body, NATIVE_MEDIA);
    let (status, body) = relay_get(&url, "bytes=100-199").await;
    assert_eq!(status, 206);
    assert_eq!(body, &NATIVE_MEDIA[100..200]);
    platform.start_from_another_thread();
    let status = until(&player.state(), playing).await;
    assert_eq!(
        status.route.as_ref().map(|route| route.label()),
        Some(RouteLabel::Native)
    );
    player.close().await;
}

#[tokio::test]
async fn without_the_codec_the_file_goes_to_libmpv_and_the_setting_and_choice_move_it_there_too() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;

    // No HEVC extension: libmpv, with what the native player lacks.
    let platform = Arc::new(FakePlatform::default());
    let (player, control) = open_with(&app, platform.clone(), false).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    eventually(|| loads(&control) == 1).await;
    assert!(platform.loads.lock().unwrap().is_empty());
    let info = player.playback_info().await.unwrap();
    let lines = crate::info_panel_lines(
        &info,
        None,
        &super::PanelSizes::default(),
        &crate::numbers::NumberFormat::default(),
    );
    assert_eq!(
        lines[..2],
        [
            "App engine",
            "• The native player can't decode this HEVC (H.265) video."
        ]
    );
    player.close().await;

    // "Play with app engine" moves the item playing.
    let platform = Arc::new(FakePlatform::default());
    let (player, control) = open_with(&app, platform.clone(), true).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    platform.start_from_another_thread();
    until(&player.state(), playing).await;
    player.play_with_app_engine().await.unwrap();
    eventually(|| loads(&control) == 1).await;
    player.close().await;

    // "Always app engine" plays the next item there.
    app.set_player_choice(current_store::PlayerChoice::AlwaysAppEngine)
        .unwrap();
    assert_eq!(
        app.player_choice(),
        current_store::PlayerChoice::AlwaysAppEngine
    );
    let platform = Arc::new(FakePlatform::default());
    let (player, control) = open_with(&app, platform.clone(), true).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    eventually(|| loads(&control) == 1).await;
    assert!(platform.loads.lock().unwrap().is_empty());
    player.close().await;
}

#[tokio::test]
async fn a_throwing_platform_engine_never_brings_the_core_down() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let platform = Arc::new(FakePlatform {
        throws: true,
        ..FakePlatform::default()
    });
    let (player, control) = open_with(&app, platform.clone(), true).await;
    // Its load throws: a failed route, so libmpv plays.
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    eventually(|| loads(&control) == 1).await;
    // Commands to it throw too (volume goes to both engines) and change nothing.
    player.send(super::PlayerAction::SetVolume(30)).unwrap();
    eventually(|| {
        platform
            .commands
            .lock()
            .unwrap()
            .contains(&"volume 30".to_owned())
    })
    .await;
    assert!(control.commands().contains(&TestCommand::Volume(30)));
    player.close().await;
}

#[tokio::test]
async fn the_platform_engines_relay_route_closes_when_the_item_stops() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let platform = Arc::new(FakePlatform::default());
    let (player, _control) = open_with(&app, platform.clone(), true).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    platform.start_from_another_thread();
    until(&player.state(), playing).await;
    let file = platform.last();
    let url = file.url.expose().to_owned();
    assert_eq!(relay_status(&url).await, 206);
    player.send(super::PlayerAction::Stop).unwrap();
    eventually(|| {
        platform
            .commands
            .lock()
            .unwrap()
            .contains(&"stop".to_owned())
    })
    .await;
    eventually(|| player.state().borrow().state == PlayerState::Idle).await;
    assert!(matches!(relay_status(&url).await, 0 | 404));
    // A late event of the stopped load starts nothing.
    platform.send(file.generation, EngineEvent::Restarted);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(player.state().borrow().state, PlayerState::Idle);
    player.close().await;
}
