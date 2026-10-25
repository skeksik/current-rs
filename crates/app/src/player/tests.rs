//! The player through `App`, with the test engine, against the Jellyfin stand-in on 127.0.0.1: start and reports, the state and the
//! media controls, the swap chain and its release, cancellation, one engine per process, the remembered quality, automatic skipping,
//! and that no URL or token shows in anything the player publishes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use current_model::{
    EngineErrorKind, ItemId, ItemRef, PlaybackProblem, PlayerState, RouteLabel, ServerRef,
};
use current_playback::{EngineEvent, PlayerStatus, Quality};
use current_store::{MemorySecrets, Secrets, StreamQuality};
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, DIRECT_PLAY_NAME, IMAGE, PASSWORD, StandIn, TRANSCODE_ITEM, USER_NAME,
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
        Some(RouteLabel::DirectPlay)
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
            credentials: false,
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
        Some(RouteLabel::Remux)
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
    // An item of a server that is not signed in.
    let other = ServerRef::new(server.service.clone(), current_model::ServerKey::new("99"));
    assert_eq!(
        player
            .play(item(&other, DIRECT_PLAY_ITEM), Duration::ZERO)
            .await,
        Err(PlayerError::NotSignedIn)
    );
    player.close().await;
    // Closed: the engine is free again.
    let (again, _) = open(&app).await;
    again.close().await;
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
async fn a_picked_quality_is_saved_for_the_kind_of_address() {
    let _engine = ENGINE.lock().await;
    let stand_in = StandIn::start().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let app = open_stored(temp.path());
    let server = signed_in(&stand_in, &app).await;
    let (player, _) = open(&app).await;
    player
        .play(item(&server, DIRECT_PLAY_ITEM), Duration::ZERO)
        .await
        .unwrap();
    until(&player.state(), playing).await;
    player
        .set_quality(Quality::Bitrate(4_000_000))
        .await
        .unwrap();
    // A re-plan at the position, with the new limit, and the old play session reported stopped.
    until(&player.state(), |status| {
        playing(status) && status.quality == Quality::Bitrate(4_000_000)
    })
    .await;
    let settings = || app.settings().unwrap().get().playback;
    eventually(|| {
        // 127.0.0.1 is this device: a local address.
        settings().quality_local == StreamQuality::Bitrate(4_000_000)
    })
    .await;
    assert_eq!(settings().quality_internet, StreamQuality::Maximum);
    player.close().await;
    drop(app);
    // Read again at the next start.
    let app = open_stored(temp.path());
    assert_eq!(app.playback.quality().local, Quality::Bitrate(4_000_000));
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

/// For a test outside a player: the test engine refuses what libmpv's mapping refuses.
#[test]
fn the_test_engine_refuses_an_unchecked_playlist_with_headers() {
    use current_playback::{Engine, EngineFile, EngineTrack, StreamAuth, StreamUrl};
    let (events, _raw) = tokio::sync::mpsc::unbounded_channel();
    // Not through `open`: no player, so the process's slot is taken only briefly.
    let guard = ENGINE.blocking_lock();
    let mut engine = test_engine::TestBackend::start(events).unwrap();
    let refused = engine.load(EngineFile {
        url: StreamUrl::new("https://media.example.com/videos/1/master.m3u8?ApiKey=t0ken"),
        auth: StreamAuth {
            headers: vec![("Authorization".into(), "Basic header-value".into())],
            ..StreamAuth::default()
        },
        start: Duration::ZERO,
        audio: EngineTrack::Auto,
        subtitle: EngineTrack::Auto,
    });
    assert_eq!(refused, Err(EngineErrorKind::GATEWAY_PLAYLIST_UNCHECKED));
    drop(engine);
    drop(guard);
}
