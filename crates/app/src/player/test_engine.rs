//! A stand-in engine for tests: built only for this crate's tests and with the `test-engine` feature, never into an app a user runs
//! (the app's build checks the published library for [`MARKER`]). It opens nothing: a load "plays" a file of [`DURATION`] at once
//! (loaded, its duration, restarted at the start position), seeks and pauses answer as an engine would, and the position moves only
//! when a test says so through [`TestEngineControl`]. It publishes one fake swap chain per engine and counts its releases.
//!
//! Like libmpv's engine it allows one engine per process: a start waits up to [`START_WAIT`] for the earlier one to shut down, then
//! fails with `Busy`.

// With libmpv built in too, players use libmpv and this engine is unused.
#![cfg_attr(feature = "libmpv", allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, EngineErrorKind, SubtitleFormat,
};
use current_playback::{EndReason, Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};
use tokio::sync::mpsc::UnboundedSender;

use super::backend::{Backend, CompositionSize, DisplayFacts, EngineOutput, Signal, SwapChain};
use super::mpv::MpvMapper;

/// Compiled into every build with the test engine, so a build check can tell such a library from a shipped one. Read when an engine
/// starts, so no linker drops it from a library that can open a test engine.
#[used]
pub static MARKER: [u8; 20] = *b"current-test-engine\0";

/// How long the fake file is.
pub const DURATION: Duration = Duration::from_secs(600);
/// The fake swap chain's handle: not a pointer, never passed to Windows.
pub const SWAP_CHAIN_HANDLE: u64 = 0x5157_0001;
/// How long a start waits for an earlier engine.
pub const START_WAIT: Duration = Duration::from_secs(2);

/// What the test engine was asked to do, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestCommand {
    /// A load, without its URL: whether it carried secrets to redact and may open references, the start and the tracks. No load
    /// carries gateway credentials any more (ADR 0020).
    Load {
        secrets: usize,
        references: bool,
        start: Duration,
        audio: EngineTrack,
        subtitle: EngineTrack,
    },
    AddSubtitle {
        index: u32,
        bytes: usize,
    },
    Paused(bool),
    Seek(Duration),
    Audio(EngineTrack),
    Subtitle(EngineTrack),
    Volume(u8),
    Muted(bool),
    /// The rate in hundredths (150 for 1.5).
    Speed(u32),
    Stop,
    DisplayFacts,
    CompositionSize {
        width: u32,
        height: u32,
    },
    Shutdown,
}

#[derive(Default)]
struct Log {
    commands: Vec<TestCommand>,
    /// Every load's URL, for tests that check where the URL went (never shown, logged or compared to anything else).
    urls: Vec<String>,
}

/// A test's view of one test engine: what it was asked, and a way to report events as libmpv would.
#[derive(Clone)]
pub struct TestEngineControl {
    log: Arc<Mutex<Log>>,
    events: UnboundedSender<TestRaw>,
    releases: Arc<AtomicU64>,
}

impl std::fmt::Debug for TestEngineControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestEngineControl").finish_non_exhaustive()
    }
}

impl TestEngineControl {
    pub fn commands(&self) -> Vec<TestCommand> {
        lock(&self.log).commands.clone()
    }

    /// The URLs loaded, oldest first.
    pub fn loaded_urls(&self) -> Vec<String> {
        lock(&self.log).urls.clone()
    }

    /// Reports `event` as the engine's event thread would.
    pub fn send(&self, event: EngineEvent) {
        let _ = self.events.send(TestRaw::Event(event));
    }

    /// Reports that the file failed to load or play, as libmpv's end with an error: for tests that cannot name
    /// `current-playback`'s events (the bridge's).
    pub fn report_error(&self) {
        self.send(EngineEvent::Ended(EndReason::Error(
            EngineErrorKind::LOADING_FAILED,
        )));
    }

    /// Reports the end of the file, as libmpv's `eof-reached`.
    pub fn report_eof(&self) {
        self.send(EngineEvent::Ended(EndReason::Eof));
    }

    /// How many times the swap chain was released.
    pub fn swap_chain_releases(&self) -> u64 {
        self.releases.load(Ordering::SeqCst)
    }
}

/// What the test engine's "thread" sends.
pub(crate) enum TestRaw {
    Event(EngineEvent),
    SwapChain,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One engine per process, as libmpv's.
static SLOT: Mutex<bool> = Mutex::new(false);
static SLOT_FREED: Condvar = Condvar::new();

/// The last test engine started, for tests that open a player through the bridge.
static LAST: Mutex<Option<TestEngineControl>> = Mutex::new(None);

/// The control of the test engine started last in this process.
pub fn last_control() -> Option<TestEngineControl> {
    lock(&LAST).clone()
}

/// What the test engine plays: its answer to [`Engine::capabilities`], and what Settings' "Supported formats" shows in a build with it
/// (ADR 0021).
pub(crate) fn capabilities() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4, Container::TS],
        video: vec![
            CodecSupport::any(Codec::HEVC),
            CodecSupport::any(Codec::H264),
        ],
        audio: vec![
            CodecSupport::any(Codec::AAC),
            CodecSupport::any(Codec::AC3),
            CodecSupport::any(Codec::EAC3),
        ],
        subtitles: vec![SubtitleFormat::new("srt"), SubtitleFormat::new("ass")],
        external_subtitles: true,
    }
}

pub(crate) struct TestBackend {
    control: TestEngineControl,
    capabilities: EngineCapabilities,
    position: Duration,
    running: bool,
    generation: u64,
}

impl TestBackend {
    /// Starts the engine. Blocking while an earlier one lives, as libmpv's start is.
    pub(crate) fn start(events: UnboundedSender<TestRaw>) -> Result<Self, EngineErrorKind> {
        std::hint::black_box(&MARKER);
        let deadline = Instant::now() + START_WAIT;
        let mut taken = lock(&SLOT);
        while *taken {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(EngineErrorKind::new("busy"));
            }
            taken = SLOT_FREED
                .wait_timeout(taken, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *taken = true;
        drop(taken);
        let control = TestEngineControl {
            log: Arc::default(),
            events,
            releases: Arc::default(),
        };
        *lock(&LAST) = Some(control.clone());
        let _ = control.events.send(TestRaw::SwapChain);
        Ok(Self {
            control,
            capabilities: capabilities(),
            position: Duration::ZERO,
            running: true,
            generation: 0,
        })
    }

    fn record(&self, command: TestCommand) {
        lock(&self.control.log).commands.push(command);
    }

    fn emit(&self, event: EngineEvent) {
        self.control.send(event);
    }
}

impl Engine for TestBackend {
    fn capabilities(&self) -> EngineCapabilities {
        self.capabilities.clone()
    }

    fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind> {
        let (start, audio, subtitle, references, secrets) = (
            file.start,
            file.audio,
            file.subtitle,
            file.references,
            file.auth.secrets.len(),
        );
        let url = file.url.expose().to_owned();
        // The same mapping as libmpv's, with its guard.
        MpvMapper::default().file_request(file)?;
        self.record(TestCommand::Load {
            secrets,
            references,
            start,
            audio,
            subtitle,
        });
        lock(&self.control.log).urls.push(url);
        self.position = start;
        self.emit(EngineEvent::Loaded);
        self.emit(EngineEvent::Duration(DURATION));
        self.emit(EngineEvent::Position(start));
        self.emit(EngineEvent::Restarted);
        Ok(())
    }

    fn add_subtitle(&mut self, subtitle: ExternalSubtitle) {
        self.record(TestCommand::AddSubtitle {
            index: subtitle.index,
            bytes: subtitle.data.len(),
        });
    }

    fn set_paused(&mut self, paused: bool) {
        self.record(TestCommand::Paused(paused));
        self.emit(EngineEvent::Paused(paused));
    }

    fn seek_to(&mut self, position: Duration) {
        self.record(TestCommand::Seek(position));
        self.position = position;
        self.emit(EngineEvent::Position(position));
        self.emit(EngineEvent::Restarted);
    }

    fn select_audio(&mut self, track: EngineTrack) {
        self.record(TestCommand::Audio(track));
    }

    fn select_subtitle(&mut self, track: EngineTrack) {
        self.record(TestCommand::Subtitle(track));
    }

    fn set_volume(&mut self, volume: u8) {
        self.record(TestCommand::Volume(volume));
    }

    fn set_muted(&mut self, muted: bool) {
        self.record(TestCommand::Muted(muted));
    }

    fn set_speed(&mut self, speed: f64) {
        // Rounded to hundredths for the record; a rate is never negative or that large.
        self.record(TestCommand::Speed((speed * 100.0).round().clamp(0.0, 1e6) as u32));
    }

    fn stop(&mut self) {
        self.record(TestCommand::Stop);
        self.emit(EngineEvent::Ended(EndReason::Stopped));
    }
}

impl Backend for TestBackend {
    type Raw = TestRaw;

    fn translate(&mut self, raw: TestRaw, out: &mut Vec<Signal>) {
        match raw {
            TestRaw::Event(event) => out.push(Signal::Event(event)),
            TestRaw::SwapChain => {
                self.generation += 1;
                let releases = self.control.releases.clone();
                out.push(Signal::SwapChain(SwapChain::new(
                    SWAP_CHAIN_HANDLE,
                    self.generation,
                    move || {
                        releases.fetch_add(1, Ordering::SeqCst);
                    },
                )));
            }
        }
    }

    fn set_display_facts(&mut self, _facts: &DisplayFacts) {
        self.record(TestCommand::DisplayFacts);
    }

    fn set_composition_size(&mut self, size: CompositionSize) {
        self.record(TestCommand::CompositionSize {
            width: size.width,
            height: size.height,
        });
    }

    fn output(&self) -> EngineOutput {
        EngineOutput {
            hardware_decoder: Some("test".into()),
            video: None,
        }
    }

    fn take_shutdown(&mut self) -> Box<dyn FnOnce() + Send> {
        if !std::mem::replace(&mut self.running, false) {
            return Box::new(|| {});
        }
        self.record(TestCommand::Shutdown);
        Box::new(free_slot)
    }
}

fn free_slot() {
    *lock(&SLOT) = false;
    SLOT_FREED.notify_all();
}

impl Drop for TestBackend {
    /// An engine dropped without a shutdown (its driver ended abnormally) frees the process's slot too.
    fn drop(&mut self) {
        if self.running {
            free_slot();
        }
    }
}
