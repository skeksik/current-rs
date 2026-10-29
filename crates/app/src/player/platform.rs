//! A platform's own player as the player's native engine (ADR 0025, "How a platform engine plugs into the core"): the UI implements
//! [`PlatformEngine`] (in C# or Swift, through `current-ffi`'s foreign trait), and [`PlatformBackend`] makes it a
//! `current_playback::Engine` for the controller.
//!
//! Every load gets the next generation number, and the engine reports events with the generation of the load they are about. The
//! driver hands the controller only events of the load playing: an event of an older load, or of any load after a stop, is dropped.
//! That keeps the `Engine` trait's rule (nothing about a file after `stop`) for an engine behind the bridge, whose events can lag.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use current_model::{EngineCapabilities, EngineErrorKind, SubtitleFormat};
use current_playback::{Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle, StreamUrl};
use tokio::sync::mpsc;

/// What a platform engine call failed with. A failed load is a failed route (the controller tries the next one); a failed command is
/// dropped, as libmpv's are: the core has no log, and the engine reports what follows from it as events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformError {
    Failed(EngineErrorKind),
    /// The platform's code threw or panicked; treated as [`EngineErrorKind::LOADING_FAILED`] for a load.
    Unexpected,
}

/// One file for the platform engine. `url` is always on the loopback relay and holds the route's token only (ADR 0025, "URLs across
/// the bridge"): never the session token. It has no `Display`, and its `Debug` shows no URL.
#[derive(Clone, PartialEq, Eq)]
pub struct PlatformFile {
    /// The load's number; events about this file carry it.
    pub generation: u64,
    pub url: StreamUrl,
    pub start: Duration,
    pub audio: EngineTrack,
    pub subtitle: EngineTrack,
    /// The URL is an HLS playlist (for Windows, `AdaptiveMediaSource`).
    pub hls: bool,
}

impl std::fmt::Debug for PlatformFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformFile")
            .field("generation", &self.generation)
            .field("start", &self.start)
            .field("audio", &self.audio)
            .field("subtitle", &self.subtitle)
            .field("hls", &self.hls)
            .finish_non_exhaustive()
    }
}

/// A subtitle the core fetched, for the platform engine to show (ADR 0025, "Subtitles on the native route").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformSubtitle {
    pub index: u32,
    pub format: Option<SubtitleFormat>,
    pub data: Vec<u8>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub select: bool,
}

/// A platform's own player. The core calls it from its player task: each method must return quickly and never wait for the UI
/// thread (the platform hands the call to its dispatcher and returns). It answers through the [`PlatformEvents`] it is given at
/// [`PlatformEngine::attach`], from any thread.
pub trait PlatformEngine: Send + Sync {
    /// Called once, before anything else.
    fn attach(&self, events: PlatformEvents) -> Result<(), PlatformError>;
    fn load(&self, file: PlatformFile) -> Result<(), PlatformError>;
    fn add_subtitle(&self, subtitle: PlatformSubtitle) -> Result<(), PlatformError>;
    fn set_paused(&self, paused: bool) -> Result<(), PlatformError>;
    fn seek_to(&self, position: Duration) -> Result<(), PlatformError>;
    fn select_audio(&self, track: EngineTrack) -> Result<(), PlatformError>;
    fn select_subtitle(&self, track: EngineTrack) -> Result<(), PlatformError>;
    /// 0 to 100.
    fn set_volume(&self, volume: u8) -> Result<(), PlatformError>;
    fn set_muted(&self, muted: bool) -> Result<(), PlatformError>;
    fn set_speed(&self, speed: f64) -> Result<(), PlatformError>;
    /// After it returns, events of the file stopped are dropped whatever the engine sends.
    fn stop(&self) -> Result<(), PlatformError>;
}

/// Which load the driver takes events for: the last one, while it is not stopped.
#[derive(Debug, Default)]
pub(crate) struct LoadGate {
    generation: AtomicU64,
    live: AtomicBool,
}

impl LoadGate {
    /// Whether an event of load `generation` is about the file playing.
    pub(crate) fn admits(&self, generation: u64) -> bool {
        self.live.load(Ordering::SeqCst) && self.generation.load(Ordering::SeqCst) == generation
    }

    fn next(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.live.store(true, Ordering::SeqCst);
        generation
    }

    fn close(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// How a platform engine reports: events with the generation of their load. Sending never blocks and works from any thread; after
/// the player closed, events go nowhere.
#[derive(Clone, Debug)]
pub struct PlatformEvents {
    sender: mpsc::UnboundedSender<(u64, EngineEvent)>,
}

impl PlatformEvents {
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<(u64, EngineEvent)>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub fn send(&self, generation: u64, event: EngineEvent) {
        let _ = self.sender.send((generation, event));
    }
}

/// The platform engine as the controller's native engine.
pub(crate) struct PlatformBackend {
    engine: Arc<dyn PlatformEngine>,
    capabilities: EngineCapabilities,
    gate: Arc<LoadGate>,
}

impl PlatformBackend {
    pub(crate) fn new(
        engine: Arc<dyn PlatformEngine>,
        capabilities: EngineCapabilities,
        gate: Arc<LoadGate>,
    ) -> Self {
        Self {
            engine,
            capabilities,
            gate,
        }
    }
}

/// Whether `url` is on the loopback relay: the only URLs a platform engine gets (ADR 0025).
fn on_relay(url: &StreamUrl) -> bool {
    url.expose().starts_with("http://127.0.0.1:")
}

impl Engine for PlatformBackend {
    fn capabilities(&self) -> EngineCapabilities {
        self.capabilities.clone()
    }

    fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind> {
        // The controller relays every stream for the native engine; a URL off the relay would carry the session token across the
        // bridge, so it is refused here too.
        if !on_relay(&file.url) {
            return Err(EngineErrorKind::LOADING_FAILED);
        }
        let generation = self.gate.next();
        let file = PlatformFile {
            generation,
            url: file.url,
            start: file.start,
            audio: file.audio,
            subtitle: file.subtitle,
            hls: file.hls,
        };
        self.engine.load(file).map_err(|error| match error {
            PlatformError::Failed(kind) => kind,
            PlatformError::Unexpected => EngineErrorKind::LOADING_FAILED,
        })
    }

    fn add_subtitle(&mut self, subtitle: ExternalSubtitle) {
        let _ = self.engine.add_subtitle(PlatformSubtitle {
            index: subtitle.index,
            format: subtitle.format,
            data: subtitle.data,
            title: subtitle.title,
            language: subtitle.language,
            select: subtitle.select,
        });
    }

    fn set_paused(&mut self, paused: bool) {
        let _ = self.engine.set_paused(paused);
    }

    fn seek_to(&mut self, position: Duration) {
        let _ = self.engine.seek_to(position);
    }

    fn select_audio(&mut self, track: EngineTrack) {
        let _ = self.engine.select_audio(track);
    }

    fn select_subtitle(&mut self, track: EngineTrack) {
        let _ = self.engine.select_subtitle(track);
    }

    fn set_volume(&mut self, volume: u8) {
        let _ = self.engine.set_volume(volume);
    }

    fn set_muted(&mut self, muted: bool) {
        let _ = self.engine.set_muted(muted);
    }

    fn set_speed(&mut self, speed: f64) {
        let _ = self.engine.set_speed(speed);
    }

    fn stop(&mut self) {
        // Closed first: an event the engine sends while stopping is already the old file's.
        self.gate.close();
        let _ = self.engine.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        files: Mutex<Vec<PlatformFile>>,
        fail_loads: Option<PlatformError>,
    }

    impl PlatformEngine for Recorder {
        fn attach(&self, _events: PlatformEvents) -> Result<(), PlatformError> {
            Ok(())
        }
        fn load(&self, file: PlatformFile) -> Result<(), PlatformError> {
            self.files.lock().unwrap().push(file);
            self.fail_loads.clone().map_or(Ok(()), Err)
        }
        fn add_subtitle(&self, _: PlatformSubtitle) -> Result<(), PlatformError> {
            Err(PlatformError::Unexpected)
        }
        fn set_paused(&self, _: bool) -> Result<(), PlatformError> {
            Err(PlatformError::Unexpected)
        }
        fn seek_to(&self, _: Duration) -> Result<(), PlatformError> {
            Ok(())
        }
        fn select_audio(&self, _: EngineTrack) -> Result<(), PlatformError> {
            Ok(())
        }
        fn select_subtitle(&self, _: EngineTrack) -> Result<(), PlatformError> {
            Ok(())
        }
        fn set_volume(&self, _: u8) -> Result<(), PlatformError> {
            Ok(())
        }
        fn set_muted(&self, _: bool) -> Result<(), PlatformError> {
            Ok(())
        }
        fn set_speed(&self, _: f64) -> Result<(), PlatformError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), PlatformError> {
            Err(PlatformError::Unexpected)
        }
    }

    fn file(url: &str) -> EngineFile {
        EngineFile {
            url: StreamUrl::new(url),
            auth: current_playback::StreamAuth::default(),
            start: Duration::from_secs(5),
            audio: EngineTrack::Stream(1),
            subtitle: EngineTrack::Off,
            references: true,
            hls: true,
        }
    }

    const RELAYED: &str = "http://127.0.0.1:50123/route-token/Videos/e1/master.m3u8";

    #[test]
    fn each_load_has_the_next_generation_and_a_stop_closes_the_gate() {
        let engine = Arc::new(Recorder::default());
        let gate = Arc::new(LoadGate::default());
        let mut backend =
            PlatformBackend::new(engine.clone(), EngineCapabilities::default(), gate.clone());
        assert!(!gate.admits(0));
        backend.load(file(RELAYED)).unwrap();
        assert!(gate.admits(1));
        backend.load(file(RELAYED)).unwrap();
        assert!(!gate.admits(1) && gate.admits(2));
        // A throwing stop still closes the gate.
        backend.stop();
        assert!(!gate.admits(2));
        let files = engine.files.lock().unwrap();
        assert_eq!(
            files.iter().map(|file| file.generation).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(files[0].start, Duration::from_secs(5));
        assert!(files[0].hls);
    }

    #[test]
    fn a_url_off_the_relay_never_reaches_the_platform() {
        let engine = Arc::new(Recorder::default());
        let gate = Arc::new(LoadGate::default());
        let mut backend =
            PlatformBackend::new(engine.clone(), EngineCapabilities::default(), gate.clone());
        let off = "https://media.example.com/Videos/e1/stream?ApiKey=secret-token";
        assert_eq!(
            backend.load(file(off)),
            Err(EngineErrorKind::LOADING_FAILED)
        );
        assert!(engine.files.lock().unwrap().is_empty());
        assert!(!gate.admits(1));
    }

    #[test]
    fn a_throwing_platform_is_a_failed_load_and_a_dropped_command() {
        let engine = Arc::new(Recorder {
            fail_loads: Some(PlatformError::Unexpected),
            ..Recorder::default()
        });
        let mut backend =
            PlatformBackend::new(engine, EngineCapabilities::default(), Arc::default());
        assert_eq!(
            backend.load(file(RELAYED)),
            Err(EngineErrorKind::LOADING_FAILED)
        );
        // Commands that throw do nothing more.
        backend.set_paused(true);
        backend.add_subtitle(ExternalSubtitle {
            index: 3,
            format: None,
            data: vec![1, 2],
            title: None,
            language: None,
            select: true,
        });
        let failing = Arc::new(Recorder {
            fail_loads: Some(PlatformError::Failed(EngineErrorKind::UNSUPPORTED_FORMAT)),
            ..Recorder::default()
        });
        let mut backend =
            PlatformBackend::new(failing, EngineCapabilities::default(), Arc::default());
        assert_eq!(
            backend.load(file(RELAYED)),
            Err(EngineErrorKind::UNSUPPORTED_FORMAT)
        );
    }

    #[test]
    fn a_files_debug_shows_no_url() {
        let shown = format!(
            "{:?}",
            PlatformFile {
                generation: 4,
                url: StreamUrl::new(RELAYED),
                start: Duration::ZERO,
                audio: EngineTrack::Auto,
                subtitle: EngineTrack::Off,
                hls: true,
            }
        );
        assert!(
            !shown.contains("route-token") && !shown.contains("127.0.0.1"),
            "{shown}"
        );
        assert!(shown.contains("generation: 4"));
    }
}
