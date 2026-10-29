//! The platform engine through the bridge (ADR 0025, W3): a Rust implementation of the foreign trait stands in for the C# or Swift
//! one, beside the test engine, against the Jellyfin stand-in. Events from another thread, the load generation, the relay URL, the
//! labels, and an engine that throws.

use std::sync::{Arc, Mutex};

use current_app::test_engine::{self, TestCommand};
use jellyfin_stand_in::{DIRECT_PLAY_ITEM, Gate};

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
            decoders: u32::from(
                ["VideoFormatH264", "VideoFormatHevc", "AudioFormatAac"]
                    .contains(&ask.subtype.as_str()),
            ),
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
            "• Native could not start.",
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
