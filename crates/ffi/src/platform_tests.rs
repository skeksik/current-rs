//! The platform engine through the bridge (ADR 0025, W3): a Rust implementation of the foreign trait stands in for the C# or Swift
//! one, beside the test engine, against the Jellyfin stand-in. Events from another thread, the load generation, the relay URL, the
//! labels, and an engine that throws.

use std::sync::{Arc, Mutex};

use current_app::test_engine::{self, TestCommand};
use jellyfin_stand_in::{DIRECT_PLAY_ITEM, Gate, STREAM_LENGTH, stream_byte};

use super::*;
use crate::player_tests::{DISPLAY, SIZE, engine_turn, item, recorder, signed_in, stand_in, until};
use crate::tests::{block_on, eventually};

#[derive(Default)]
struct FakeEngine {
    events: Mutex<Option<Arc<PlatformEngineEvents>>>,
    files: Mutex<Vec<PlatformFile>>,
    calls: Mutex<Vec<String>>,
    /// Every call fails as a foreign exception does.
    throws: bool,
}

impl FakeEngine {
    fn call(&self, what: String) -> Result<(), PlatformEngineError> {
        self.calls.lock().unwrap().push(what);
        if self.throws {
            Err(uniffi::UnexpectedUniFFICallbackError::new("boom").into())
        } else {
            Ok(())
        }
    }

    fn last(&self) -> PlatformFile {
        self.files.lock().unwrap().last().cloned().expect("a load")
    }

    fn events(&self) -> Arc<PlatformEngineEvents> {
        self.events.lock().unwrap().clone().expect("attached")
    }
}

impl PlatformEngine for FakeEngine {
    fn attach(&self, events: Arc<PlatformEngineEvents>) -> Result<(), PlatformEngineError> {
        *self.events.lock().unwrap() = Some(events);
        Ok(())
    }
    fn load(&self, file: PlatformFile) -> Result<(), PlatformEngineError> {
        self.files.lock().unwrap().push(file);
        self.call("load".into())
    }
    fn add_subtitle(&self, subtitle: PlatformSubtitleFile) -> Result<(), PlatformEngineError> {
        self.call(format!("subtitle {}", subtitle.index))
    }
    fn set_paused(&self, paused: bool) -> Result<(), PlatformEngineError> {
        self.call(format!("paused {paused}"))
    }
    fn seek_to(&self, position_ms: u64) -> Result<(), PlatformEngineError> {
        self.call(format!("seek {position_ms}"))
    }
    fn select_audio(&self, track: PlatformTrack) -> Result<(), PlatformEngineError> {
        self.call(format!("audio {track:?}"))
    }
    fn select_subtitle(&self, track: PlatformTrack) -> Result<(), PlatformEngineError> {
        self.call(format!("subtitles {track:?}"))
    }
    fn set_volume(&self, volume: u8) -> Result<(), PlatformEngineError> {
        self.call(format!("volume {volume}"))
    }
    fn set_muted(&self, muted: bool) -> Result<(), PlatformEngineError> {
        self.call(format!("muted {muted}"))
    }
    fn set_speed(&self, speed: f64) -> Result<(), PlatformEngineError> {
        self.call(format!("speed {speed}"))
    }
    fn stop(&self) -> Result<(), PlatformEngineError> {
        self.call("stop".into())
    }
}

fn panel() -> PanelSizes {
    PanelSizes {
        composition_width: 1280,
        composition_height: 720,
        view_width: 1280.0,
        view_height: 720.0,
        content_width: None,
        content_height: None,
        window_width: None,
        window_height: None,
    }
}

/// Windows with H.264, HEVC and AAC decoders.
fn answers() -> Vec<CodecQueryAnswer> {
    windows_codec_queries()
        .into_iter()
        .map(|ask| CodecQueryAnswer {
            decoders: if ["VideoFormatH264", "VideoFormatHevc", "AudioFormatAac"]
                .contains(&ask.subtype.as_str())
            {
                vec!["A decoder".to_owned()]
            } else {
                Vec::new()
            },
            subtype: ask.subtype,
        })
        .collect()
}

fn test_engine_loads() -> usize {
    test_engine::last_control()
        .map(|control| {
            control
                .commands()
                .iter()
                .filter(|command| matches!(command, TestCommand::Load { .. }))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn the_core_asks_windows_for_its_decoders_by_codec_subtypes_names() {
    let asks = windows_codec_queries();
    assert!(asks.contains(&CodecQueryAsk {
        kind: CodecQueryKind::Video,
        subtype: "VideoFormatHevc".into()
    }));
    assert!(asks.contains(&CodecQueryAsk {
        kind: CodecQueryKind::Audio,
        subtype: "AudioFormatDolbyDDPlus".into()
    }));
}

#[test]
fn a_platform_engine_plays_through_the_bridge_and_hands_over_on_a_failed_track() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let engine = Arc::new(FakeEngine::default());
    let player =
        block_on(core.open_player_with_platform_engine(SIZE, DISPLAY, engine.clone(), answers()))
            .unwrap();
    let (states, state_rx) = recorder();
    let _handle = player.subscribe_state(states);

    block_on(player.play(item(DIRECT_PLAY_ITEM), 3_000, CancelToken::new())).unwrap();
    let file = engine.last();
    assert!(file.url.starts_with("http://127.0.0.1:"));
    assert!(!file.url.contains(&stand_in.live_token().unwrap()));
    // The stream reader's switch is off by default (ADR 0026, until R3).
    assert!(file.reader.is_none());
    assert_eq!(file.start_ms, 3_000);
    assert_eq!(file.audio, PlatformTrack::Stream { index: 1 });
    assert!(!format!("{file:?}").contains("127.0.0.1"));

    // The platform's own thread reports.
    let events = engine.events();
    let generation = file.generation;
    std::thread::spawn(move || {
        for event in [
            PlatformEvent::Loaded,
            PlatformEvent::Duration { ms: 600_000 },
            PlatformEvent::Restarted,
            PlatformEvent::RatesSupported { supported: true },
        ] {
            events.send(generation, event);
        }
    })
    .join()
    .unwrap();
    let playing = until(&state_rx, "playing natively", |state| {
        state.state == PlayerStateKind::Playing
    });
    let route = playing.last().unwrap().route.clone().unwrap();
    assert_eq!(route.label, RouteLabelKind::Native);
    assert_eq!(route.engine, EngineKind::Platform);
    // Today's UI name stays what it was.
    assert_eq!(route.kind, RouteKind::DirectPlay);
    let lines = block_on(player.playback_info_lines(None, panel())).unwrap();
    assert_eq!(lines[0], "Native");
    assert_eq!(
        core.route_label_name(RouteLabelKind::AppEngine),
        "App engine"
    );

    // An older generation is dropped; the current one's failed track hands the file to libmpv.
    engine.events().send(
        generation - 1,
        PlatformEvent::Ended {
            reason: PlatformEndReason::Error {
                kind: "decoder_failed".into(),
            },
        },
    );
    engine.events().send(
        generation,
        PlatformEvent::TrackFailed {
            kind: "decoder_failed".into(),
        },
    );
    eventually("libmpv loads", || test_engine_loads() == 1);
    let lines = block_on(player.playback_info_lines(None, panel())).unwrap();
    assert_eq!(
        lines[..3],
        [
            "App engine",
            "• The native player could not start the file.",
            "• The previous playback route could not start."
        ]
    );
    block_on(player.close());
}

#[test]
fn an_engine_that_throws_never_brings_the_core_down() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let engine = Arc::new(FakeEngine {
        throws: true,
        ..FakeEngine::default()
    });
    let player =
        block_on(core.open_player_with_platform_engine(SIZE, DISPLAY, engine.clone(), answers()))
            .unwrap();
    // The load throws: a failed route, so libmpv plays.
    block_on(player.play(item(DIRECT_PLAY_ITEM), 0, CancelToken::new())).unwrap();
    eventually("libmpv loads", || test_engine_loads() == 1);
    // A throwing command changes nothing.
    player.set_volume(25).unwrap();
    eventually("the volume reaches it", || {
        engine
            .calls
            .lock()
            .unwrap()
            .contains(&"volume 25".to_owned())
    });
    let state = player.state();
    assert_ne!(state.state, PlayerStateKind::Failed);
    block_on(player.close());
}

/// The direct-play item's stream path, as the core asks for it.
fn stream_path() -> String {
    format!("/Videos/{DIRECT_PLAY_ITEM}/stream")
}

/// A player with the fake platform engine and the stream reader's switch on, playing the direct-play item from `start_ms`.
fn reading(core: &Core, start_ms: u64) -> (Arc<FakeEngine>, Arc<Player>, Arc<StreamReader>) {
    let engine = Arc::new(FakeEngine::default());
    let player =
        block_on(core.open_player_with_platform_engine(SIZE, DISPLAY, engine.clone(), answers()))
            .unwrap();
    player.set_stream_reader(true);
    block_on(player.play(item(DIRECT_PLAY_ITEM), start_ms, CancelToken::new())).unwrap();
    let file = engine.last();
    let reader = file.reader.clone().expect("a reader with the switch on");
    (engine, player, reader)
}

/// Starts `call` on another thread, returning its result's receiver.
fn in_background<T: Send + 'static>(
    call: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(call());
    });
    rx
}

const PENDING: std::time::Duration = std::time::Duration::from_millis(300);
const DONE: std::time::Duration = std::time::Duration::from_secs(10);

#[test]
fn with_the_switch_on_a_direct_play_load_reads_its_file_through_the_core() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let (engine, player, reader) = reading(&core, 2_000);
    let file = engine.last();
    assert!(file.url.is_empty(), "no URL with a reader");
    assert_eq!(file.start_ms, 2_000);
    assert!(format!("{file:?}").contains("reader: true"));

    let info = block_on(reader.open(CancelToken::new())).unwrap();
    assert_eq!(info.size, STREAM_LENGTH as u64);
    assert_eq!(info.content_type, "video/mp4");
    // The same answer again, without another request.
    let requests = stand_in.streams().len();
    assert_eq!(block_on(reader.open(CancelToken::new())).unwrap(), info);
    assert_eq!(stand_in.streams().len(), requests);

    let expected =
        |from: usize, len: usize| (from..from + len).map(stream_byte).collect::<Vec<_>>();
    let start = block_on(reader.read(0, 100, CancelToken::new())).unwrap();
    assert_eq!(start, expected(0, 100));
    let middle = block_on(reader.read(100_000, 64, CancelToken::new())).unwrap();
    assert!(!middle.is_empty());
    assert_eq!(middle, expected(100_000, middle.len()));
    let end = STREAM_LENGTH as u64;
    assert!(
        block_on(reader.read(end, 10, CancelToken::new()))
            .unwrap()
            .is_empty()
    );
    // The reader's requests carry no session token in their query, as the relay's.
    assert!(
        stand_in
            .streams()
            .iter()
            .all(|seen| !seen.target.to_ascii_lowercase().contains("apikey"))
    );

    // Closing it ends its calls; the playback goes on.
    reader.close();
    assert_eq!(
        block_on(reader.read(0, 10, CancelToken::new())),
        Err(StreamReadError::Closed)
    );
    block_on(player.close());
}

#[test]
fn a_read_waiting_for_the_server_ends_when_its_token_is_cancelled() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let (_engine, player, reader) = reading(&core, 0);
    stand_in.hold(&stream_path());
    let token = CancelToken::new();
    let pending = {
        let (reader, token) = (reader.clone(), token.clone());
        in_background(move || block_on(reader.read(0, 10, token)))
    };
    // The control: the read waits for the held answer.
    assert!(pending.recv_timeout(PENDING).is_err());
    token.cancel();
    assert_eq!(
        pending.recv_timeout(DONE).unwrap(),
        Err(StreamReadError::Cancelled)
    );
    stand_in.release(&stream_path());
    // A new read goes through.
    assert!(
        !block_on(reader.read(0, 10, CancelToken::new()))
            .unwrap()
            .is_empty()
    );
    block_on(player.close());
}

#[test]
fn a_refused_stream_fails_the_read_and_the_player_says_why() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let (_engine, player, reader) = reading(&core, 0);
    let (states, state_rx) = recorder();
    let _handle = player.subscribe_state(states);
    stand_in.fail_stream(&stream_path(), 500);
    assert_eq!(
        block_on(reader.open(CancelToken::new())),
        Err(StreamReadError::Failed)
    );
    until(&state_rx, "the server's problem shown", |state| {
        state.problem.is_some()
    });
    block_on(player.close());
}

/// A read of the held stream that waits, checked to wait.
fn pending_read(
    reader: &Arc<StreamReader>,
) -> std::sync::mpsc::Receiver<Result<Vec<u8>, StreamReadError>> {
    let reader = reader.clone();
    let pending = in_background(move || block_on(reader.read(0, 10, CancelToken::new())));
    assert!(pending.recv_timeout(PENDING).is_err(), "the read waits");
    pending
}

#[test]
fn a_pending_read_ends_cancelled_on_a_stop_a_re_plan_and_a_close() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    stand_in.hold(&stream_path());

    // A stop.
    let (_engine, player, reader) = reading(&core, 0);
    let pending = pending_read(&reader);
    player.stop().unwrap();
    assert_eq!(
        pending.recv_timeout(DONE).unwrap(),
        Err(StreamReadError::Cancelled)
    );
    block_on(player.close());

    // A re-plan: a quality below the file's bitrate moves it to a server stream at the position.
    let (engine, player, reader) = reading(&core, 0);
    let (states, state_rx) = recorder();
    let _handle = player.subscribe_state(states);
    let generation = engine.last().generation;
    for event in [
        PlatformEvent::Loaded,
        PlatformEvent::Duration { ms: 600_000 },
        PlatformEvent::Restarted,
    ] {
        engine.events().send(generation, event);
    }
    until(&state_rx, "playing natively", |state| {
        state.state == PlayerStateKind::Playing
    });
    let pending = pending_read(&reader);
    block_on(player.set_quality(
        QualityChoice::Bitrate {
            bits_per_second: 500_000,
        },
        CancelToken::new(),
    ))
    .unwrap();
    assert_eq!(
        pending.recv_timeout(DONE).unwrap(),
        Err(StreamReadError::Cancelled)
    );
    eventually("the file given again", || {
        engine.files.lock().unwrap().len() >= 2
    });
    block_on(player.close());

    // A close.
    let (_engine, player, reader) = reading(&core, 0);
    let pending = pending_read(&reader);
    block_on(player.close());
    assert_eq!(
        pending.recv_timeout(DONE).unwrap(),
        Err(StreamReadError::Cancelled)
    );
    stand_in.release(&stream_path());
}
