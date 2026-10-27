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
