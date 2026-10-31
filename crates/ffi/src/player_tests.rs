//! The player through the bridge (ADR 0019 task 4), with the test engine, against the Jellyfin stand-in on 127.0.0.1: the state,
//! the media controls and the swap chain through their listeners, the reports the stand-in receives, release, cancelling, one
//! engine per process, the playback settings, and that no URL, token, cookie or header value is in any record that crosses.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use current_app::test_engine::{self, TestCommand};
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, DIRECT_PLAY_NAME, GATE_HEADER, GATE_HEADER_VALUE, Gate, PASSWORD, StandIn,
    TRANSCODE_ITEM, USER_NAME,
};

use super::*;
use crate::tests::{WAIT, block_on, eventually};

/// One engine per process: the tests that open a player take turns.
static ENGINE: Mutex<()> = Mutex::new(());

pub(crate) fn engine_turn() -> std::sync::MutexGuard<'static, ()> {
    ENGINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct Recorder<T>(Mutex<Sender<T>>);

pub(crate) fn recorder<T>() -> (Box<Recorder<T>>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(Recorder(Mutex::new(tx))), rx)
}

impl<T> Recorder<T> {
    fn send(&self, value: T) {
        let _ = self.0.lock().unwrap().send(value);
    }
}

impl PlayerStateListener for Recorder<PlayerStateSnapshot> {
    fn on_value(&self, value: PlayerStateSnapshot) {
        self.send(value);
    }
}

impl NowPlayingListener for Recorder<NowPlayingSnapshot> {
    fn on_value(&self, value: NowPlayingSnapshot) {
        self.send(value);
    }
}

impl SwapChainListener for Recorder<Option<Arc<SwapChainHandle>>> {
    fn on_value(&self, value: Option<Arc<SwapChainHandle>>) {
        self.send(value);
    }
}

/// Receives until `done` holds; returns everything received, the matching value last.
pub(crate) fn until<T: std::fmt::Debug>(
    rx: &Receiver<T>,
    what: &str,
    done: impl Fn(&T) -> bool,
) -> Vec<T> {
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(WAIT) {
            Ok(value) => {
                let matched = done(&value);
                seen.push(value);
                if matched {
                    return seen;
                }
            }
            Err(_) => panic!("timed out waiting for {what}; received {seen:?}"),
        }
    }
}

pub(crate) fn stand_in(gate: Gate) -> StandIn {
    let stand_in = runtime::handle()
        .unwrap()
        .block_on(StandIn::start())
        .unwrap();
    stand_in.set_gate(gate);
    stand_in
}

/// Adds the stand-in (with its gateway header when it has the header gate) and signs in.
pub(crate) fn signed_in(core: &Core, stand_in: &StandIn, gate_header: bool) {
    let add = core.add_server();
    block_on(add.probe(stand_in.address().into(), CancelToken::new())).unwrap();
    if gate_header {
        block_on(add.submit_headers(
            vec![GatewayHeader {
                name: GATE_HEADER.into(),
                value: GATE_HEADER_VALUE.into(),
            }],
            false,
            CancelToken::new(),
        ))
        .unwrap();
    }
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(
        matches!(flow.state(), SignInState::SignedIn { .. }),
        "{:?}",
        flow.state()
    );
    eventually("an address", || {
        let (listener, rx) = flow_recorder();
        let _handle = core.subscribe_capabilities(listener);
        rx.recv_timeout(WAIT)
            .is_ok_and(|capabilities| capabilities.servers[0].address_position.is_some())
    });
}

struct CapabilitiesRecorder(Mutex<Sender<CapabilitiesSnapshot>>);

impl CapabilitiesListener for CapabilitiesRecorder {
    fn on_value(&self, value: CapabilitiesSnapshot) {
        let _ = self.0.lock().unwrap().send(value);
    }
}

fn flow_recorder() -> (Box<CapabilitiesRecorder>, Receiver<CapabilitiesSnapshot>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(CapabilitiesRecorder(Mutex::new(tx))), rx)
}

pub(crate) fn item(id: &str) -> ItemRef {
    ItemRef {
        server: ServerRef {
            service: "jellyfin".into(),
            key: "1".into(),
        },
        id: id.into(),
    }
}

pub(crate) const SIZE: CompositionSize = CompositionSize {
    width: 1280,
    height: 720,
};

pub(crate) const DISPLAY: DisplayFacts = DisplayFacts {
    hdr: true,
    peak_nits: 1000.0,
    sdr_white_nits: 240.0,
    refresh_hz: 120.0,
};

fn reports(stand_in: &StandIn) -> Vec<String> {
    stand_in
        .reports()
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

#[test]
fn a_player_plays_reports_and_releases_through_the_bridge() {
    let _turn = engine_turn();
    assert_eq!(player_engine(), PlayerEngineKind::Test);
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    let control = test_engine::last_control().unwrap();
    let (states, state_rx) = recorder();
    let (nows, now_rx) = recorder();
    let (swaps, swap_rx) = recorder();
    let handles = [
        player.subscribe_state(states),
        player.subscribe_now_playing(nows),
        player.subscribe_swap_chain(swaps),
    ];

    block_on(player.play(item(DIRECT_PLAY_ITEM), 0, CancelToken::new())).unwrap();
    let playing = until(&state_rx, "playing", |state| {
        state.state == PlayerStateKind::Playing
    });
    let last = playing.last().unwrap();
    assert_eq!(last.item, Some(item(DIRECT_PLAY_ITEM)));
    assert_eq!(last.duration_ms, Some(600_000));
    assert_eq!(
        last.route.as_ref().map(|route| route.kind),
        Some(RouteKind::DirectPlay)
    );
    assert_eq!(last.audio, Some(1));
    assert_eq!(last.quality, QualityChoice::Maximum);
    // The state before the plan shows at once.
    assert!(
        playing
            .iter()
            .any(|state| state.state == PlayerStateKind::Preparing)
    );
    eventually("the start report", || {
        reports(&stand_in) == ["/Sessions/Playing"]
    });
    let now = until(&now_rx, "the media controls", |now| {
        now.status == MediaStatusKind::Playing && now.title.is_some()
    });
    let now = now.last().unwrap();
    assert_eq!(now.title.as_deref(), Some(DIRECT_PLAY_NAME));
    assert!(now.can_pause && now.can_seek);

    // The quick actions run in order: pause, then a seek.
    player.pause().unwrap();
    player.seek_to(90_000).unwrap();
    until(&state_rx, "paused at 90 s", |state| {
        state.state == PlayerStateKind::Paused && state.position_ms == 90_000
    });
    eventually("a progress report", || {
        reports(&stand_in).contains(&"/Sessions/Playing/Progress".to_owned())
    });
    let commands = control.commands();
    let pause = commands
        .iter()
        .position(|c| *c == TestCommand::Paused(true));
    let seek = commands
        .iter()
        .position(|c| *c == TestCommand::Seek(std::time::Duration::from_secs(90)));
    assert!(pause.is_some() && pause < seek, "{commands:?}");
    assert!(commands.contains(&TestCommand::CompositionSize {
        width: 1280,
        height: 720
    }));

    // The swap chain: its handle as an integer, released once whatever the UI calls.
    let swap_chain = until(&swap_rx, "a swap chain", Option::is_some)
        .pop()
        .flatten()
        .unwrap();
    assert_eq!(swap_chain.pointer(), test_engine::SWAP_CHAIN_HANDLE);
    assert_eq!(swap_chain.generation(), 1);
    swap_chain.release();
    swap_chain.release();
    assert!(swap_chain.is_released());
    assert_eq!(control.swap_chain_releases(), 1);

    player.stop().unwrap();
    until(&state_rx, "idle", |state| {
        state.state == PlayerStateKind::Idle
    });
    eventually("the stop report", || {
        reports(&stand_in).last().map(String::as_str) == Some("/Sessions/Playing/Stopped")
    });
    block_on(player.close());
    assert_eq!(control.commands().last(), Some(&TestCommand::Shutdown));
    until(&swap_rx, "no swap chain after closing", Option::is_none);
    assert_eq!(
        player.pause(),
        Err(CoreError::Player {
            problem: PlayerProblem::Closed
        })
    );
    for handle in handles {
        handle.cancel();
    }
    eventually("listeners dropped", || core.live_listeners() == 0);
}

#[test]
fn one_engine_per_process_and_a_cancelled_play() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    assert_eq!(
        block_on(core.open_player(SIZE, DISPLAY)).err(),
        Some(CoreError::Player {
            problem: PlayerProblem::Busy
        })
    );

    let path = format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo");
    stand_in.hold(&path);
    let token = CancelToken::new();
    let cancel = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancel.cancel();
    });
    assert_eq!(
        block_on(player.play(item(DIRECT_PLAY_ITEM), 0, token)),
        Err(CoreError::Cancelled)
    );
    eventually("idle after the cancel", || {
        player.state().state == PlayerStateKind::Idle
    });
    // A stop that replaces a play held at its plan answers it `Replaced`, never `Closed`.
    let stopper = player.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = stopper.stop();
    });
    assert_eq!(
        block_on(player.play(item(DIRECT_PLAY_ITEM), 0, CancelToken::new())),
        Err(CoreError::Player {
            problem: PlayerProblem::Replaced
        })
    );
    stand_in.release(&path);
    block_on(player.close());
    // The engine is free for the next player.
    let next = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    block_on(next.close());
}

#[test]
fn the_playback_settings_switch_skipping() {
    let core = Core::new().unwrap();
    assert_eq!(
        core.playback_settings(),
        PlaybackSettingsSnapshot {
            preferred_quality: QualityChoice::Maximum,
            auto_skip: Vec::new(),
            skips_intros_and_credits: false,
            shows_quality_resolution: true,
            shows_all_qualities: false,
            how_to_play: HowToPlayKind::Auto,
        }
    );
    // "How to play" round trips (ADR 0025, choice 3 as amended), every choice.
    for how in crate::settings_how_to_play_choices(true) {
        block_on(core.set_how_to_play(how)).unwrap();
        assert_eq!(core.playback_settings().how_to_play, how);
    }
    block_on(core.set_how_to_play(HowToPlayKind::Auto)).unwrap();
    // Windows has no remux on the device: Settings leaves it out.
    assert_eq!(
        crate::settings_how_to_play_choices(false),
        [
            HowToPlayKind::Auto,
            HowToPlayKind::Native,
            HowToPlayKind::AppEngine,
            HowToPlayKind::ServerRemux,
            HowToPlayKind::ServerTranscode
        ]
    );
    assert_eq!(
        core.how_to_play_name(HowToPlayKind::ServerTranscode),
        "Server transcode"
    );
    let preferred = QualityChoice::Bitrate {
        bits_per_second: 20_000_000,
    };
    block_on(core.set_preferred_quality(preferred)).unwrap();
    assert_eq!(core.playback_settings().preferred_quality, preferred);
    // A step the short list does not offer is stored as the nearest it does, so Settings always has one selected.
    block_on(core.set_preferred_quality(QualityChoice::Bitrate {
        bits_per_second: 15_000_000,
    }))
    .unwrap();
    let ten = QualityChoice::Bitrate {
        bits_per_second: 10_000_000,
    };
    assert_eq!(core.playback_settings().preferred_quality, ten);
    // 10 Mbit/s is a common step (owner, 2026-10-27): stored as it is.
    block_on(core.set_preferred_quality(ten)).unwrap();
    assert_eq!(core.playback_settings().preferred_quality, ten);
    // No bitrate is no limit: "Maximum", as the settings file reads it.
    block_on(core.set_preferred_quality(QualityChoice::Bitrate { bits_per_second: 0 })).unwrap();
    assert_eq!(
        core.playback_settings().preferred_quality,
        QualityChoice::Maximum
    );
    block_on(core.set_show_all_qualities(true)).unwrap();
    assert!(core.playback_settings().shows_all_qualities);
    // A step only "Show all" lists, then the switch off: the nearest listed step not above it, 4 Mbit/s.
    block_on(core.set_preferred_quality(QualityChoice::Bitrate {
        bits_per_second: 8_000_000,
    }))
    .unwrap();
    block_on(core.set_show_all_qualities(false)).unwrap();
    assert_eq!(
        core.playback_settings().preferred_quality,
        QualityChoice::Bitrate {
            bits_per_second: 4_000_000
        }
    );
    block_on(core.set_show_quality_resolution(false)).unwrap();
    assert!(!core.playback_settings().shows_quality_resolution);
    block_on(core.set_show_quality_resolution(true)).unwrap();
    assert!(core.playback_settings().shows_quality_resolution);
    block_on(core.set_skip_intros_and_credits(true)).unwrap();
    assert!(core.playback_settings().skips_intros_and_credits);
    assert_eq!(core.playback_settings().auto_skip, ["intro", "outro"]);
    block_on(core.set_auto_skip(vec!["intro".into(), "recap".into()])).unwrap();
    assert!(!core.playback_settings().skips_intros_and_credits);
    block_on(core.set_skip_intros_and_credits(false)).unwrap();
    assert!(core.playback_settings().auto_skip.is_empty());
    assert_eq!(
        quality_steps()[0],
        QualityChoice::Bitrate {
            bits_per_second: 40_000_000
        }
    );
}

/// Every type `player_records.rs` declares, so that a new one must be added here and so to the check below. The item reference is
/// browse's (`browse::records`), which holds a server reference and an id only.
/// Sizes for the info panel's last line, as a window measures them.
fn panel_sizes() -> PanelSizes {
    PanelSizes {
        composition_width: 1_280,
        composition_height: 720,
        view_width: 1_280.0,
        view_height: 720.0,
        content_width: Some(1_280.0),
        content_height: Some(800.0),
        window_width: Some(1_280),
        window_height: Some(800),
    }
}

const RECORD_TYPES: &[&str] = &[
    "CompositionSize",
    "DisplayFacts",
    "PlayerEngineKind",
    "PlayerProblem",
    "PlaybackProblem",
    "PlayerStateKind",
    "RouteKind",
    "RouteLabelKind",
    "EngineKind",
    "StreamProtocolKind",
    "RouteInfo",
    "TrackInfo",
    "SubtitleSelection",
    "QualityChoice",
    "SegmentInfo",
    "NextEpisodeInfo",
    "PlayerStateSnapshot",
    "MediaStatusKind",
    "NowPlayingSnapshot",
    "StreamInfo",
    "SourceInfo",
    "PlanNoteInfo",
    "TranscodingInfo",
    "EngineOutputInfo",
    "PlaybackInfoSnapshot",
    "PlaybackSettingsSnapshot",
    "HowToPlayKind",
    "QualityOptionInfo",
    "QualityMenuEntry",
];

#[test]
fn the_record_list_covers_every_player_record() {
    let source = include_str!("player_records.rs");
    let declared: Vec<&str> = source
        .lines()
        .filter_map(|line| {
            line.strip_prefix("pub struct ")
                .or_else(|| line.strip_prefix("pub enum "))
        })
        .map(|rest| {
            rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
                .unwrap_or_default()
        })
        .collect();
    let mut sorted_declared = declared.clone();
    sorted_declared.sort_unstable();
    let mut sorted_listed = RECORD_TYPES.to_vec();
    sorted_listed.sort_unstable();
    assert_eq!(sorted_declared, sorted_listed);
}

#[test]
fn no_url_token_cookie_or_header_value_crosses_the_bridge() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Header);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, true);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    let control = test_engine::last_control().unwrap();
    let (states, state_rx) = recorder();
    let (nows, now_rx) = recorder();
    let (swaps, swap_rx) = recorder();
    let _handles = [
        player.subscribe_state(states),
        player.subscribe_now_playing(nows),
        player.subscribe_swap_chain(swaps),
    ];
    let mut crossed: Vec<String> = Vec::new();

    // Direct play behind the gateway: the engine gets a relay URL, with no credential; the bridge gets neither.
    block_on(player.play(item(DIRECT_PLAY_ITEM), 0, CancelToken::new())).unwrap();
    let states_seen = until(&state_rx, "playing", |state| {
        state.state == PlayerStateKind::Playing
    });
    let relayed = control.loaded_urls().pop().unwrap();
    assert!(relayed.starts_with("http://127.0.0.1:") && !relayed.contains("ApiKey"));
    crossed.extend(states_seen.iter().map(|state| format!("{state:?}")));
    crossed.extend(
        until(&now_rx, "a title", |now| now.title.is_some())
            .iter()
            .map(|now| format!("{now:?}")),
    );
    crossed.push(format!("{:?}", block_on(player.playback_info()).unwrap()));
    crossed.push(format!(
        "{:?}",
        block_on(player.playback_info_lines(Some(DISPLAY), panel_sizes())).unwrap()
    ));
    let swap_chain = until(&swap_rx, "a swap chain", Option::is_some)
        .pop()
        .flatten()
        .unwrap();
    crossed.push(format!(
        "{} {} {}",
        swap_chain.pointer(),
        swap_chain.generation(),
        swap_chain.is_released()
    ));

    // An HLS stream behind a header gateway, through the relay too.
    block_on(player.play(item(TRANSCODE_ITEM), 30_000, CancelToken::new())).unwrap();
    let hls = until(&state_rx, "the HLS stream playing", |state| {
        state.state == PlayerStateKind::Playing && state.item == Some(item(TRANSCODE_ITEM))
    });
    assert!(control.commands().iter().any(|command| matches!(
        command,
        TestCommand::Load {
            references: true,
            ..
        }
    )));
    crossed.extend(hls.iter().map(|state| format!("{state:?}")));

    // A playlist that names another host is refused by the relay when the player asks for it.
    stand_in.set_playlist(
        &jellyfin_stand_in::master_playlist_path(),
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=8000000\nhttps://cdn.example.com/main.m3u8\n",
    );
    block_on(player.play(item(TRANSCODE_ITEM), 0, CancelToken::new())).unwrap();
    let url = control.loaded_urls().pop().unwrap();
    assert_eq!(crate::relay_tests::fetch(&url, None).unwrap().status, 502);
    let failed = until(&state_rx, "the refusal", |state| {
        state.state == PlayerStateKind::Failed
    });
    assert_eq!(
        failed.last().unwrap().problem,
        Some(PlaybackProblem::PlaylistElsewhere)
    );
    crossed.extend(failed.iter().map(|state| format!("{state:?}")));
    crossed.push(format!("{:?}", block_on(player.playback_info()).unwrap()));
    crossed.push(format!(
        "{:?}",
        block_on(player.playback_info_lines(Some(DISPLAY), panel_sizes())).unwrap()
    ));
    crossed.push(format!("{:?}", player.state()));
    crossed.push(format!("{:?}", core.playback_settings()));

    // The relay's route token, the first path segment of its URLs.
    let route = url
        .trim_start_matches("http://127.0.0.1:")
        .split('/')
        .nth(1)
        .unwrap()
        .to_owned();
    assert!(route.len() > 8);
    let host = stand_in.address().trim_start_matches("http://").to_owned();
    let crossed = crossed.join("\n");
    for secret in [
        route.as_str(),
        "t0ken",
        "127.0.0.1",
        GATE_HEADER_VALUE,
        host.as_str(),
        "ApiKey",
        "api_key",
        "Videos/",
        "master.m3u8",
        "cdn.example.com",
        "http",
    ] {
        assert!(
            !crossed.contains(secret),
            "{secret} crossed the bridge:\n{crossed}"
        );
    }
    block_on(player.close());
}
