//! What a player engine must offer the composition root besides `current-playback`'s [`Engine`] trait: its own events turned into the
//! controller's, the swap chain for the UI, the display facts and composition size from the UI, what it reports about its output, and a
//! blocking shutdown.
//!
//! The engine's event thread only sends its raw events into a channel; the player's driver task hands each to
//! [`Backend::translate`], which owns the mapping state. So no listener is ever called from the engine's thread (ADR 0019, "Threads and
//! event rate").

use std::sync::{Mutex, PoisonError};

use current_playback::{Engine, EngineEvent};

pub use current_mpv::{CompositionSize, DisplayFacts};

/// A reference to the engine's swap chain for the UI (ADR 0007, ADR 0019 "Swap chain"): `handle` is the `IDXGISwapChain*` as an integer
/// for `SetSwapChain`, and `generation` grows with each new swap chain. Call [`SwapChain::release`] once `SetSwapChain` took its own
/// reference; dropping the last handle releases too, and releasing twice does nothing.
pub struct SwapChain {
    handle: u64,
    generation: u64,
    release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl SwapChain {
    pub(crate) fn new(
        handle: u64,
        generation: u64,
        release: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            handle,
            generation,
            release: Mutex::new(Some(Box::new(release))),
        }
    }

    pub fn handle(&self) -> u64 {
        self.handle
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Gives the core's reference back.
    pub fn release(&self) {
        let release = self
            .release
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(release) = release {
            release();
        }
    }

    pub fn is_released(&self) -> bool {
        self.release
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    }
}

impl Drop for SwapChain {
    fn drop(&mut self) {
        self.release();
    }
}

impl std::fmt::Debug for SwapChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwapChain")
            .field("generation", &self.generation)
            .field("released", &self.is_released())
            .finish_non_exhaustive()
    }
}

/// What the engine reports about its decoding and output, for playback information. No URL or path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineOutput {
    /// The hardware decoder in use (libmpv's `hwdec-current`); `None` for software decoding or before the first frame.
    pub hardware_decoder: Option<String>,
    pub video: Option<VideoOutput>,
}

/// The decoded video's parameters (libmpv's `video-params`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoOutput {
    pub width: u32,
    pub height: u32,
    pub pixel_format: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
}

/// One translated engine event.
pub(crate) enum Signal {
    Event(EngineEvent),
    SwapChain(SwapChain),
    /// The engine stopped working on its own: nothing more will play. Only libmpv's engine does.
    #[cfg_attr(not(feature = "libmpv"), allow(dead_code))]
    Gone,
}

/// A player engine as the player's driver uses it.
pub(crate) trait Backend: Engine + Send + 'static {
    /// What the engine's own thread sends.
    type Raw: Send + 'static;

    fn translate(&mut self, raw: Self::Raw, out: &mut Vec<Signal>);
    fn set_display_facts(&mut self, facts: &DisplayFacts);
    fn set_composition_size(&mut self, size: CompositionSize);
    fn output(&self) -> EngineOutput;
    /// Takes the engine out and returns what stops it and waits until it is gone. Blocking: the driver runs it on the blocking pool.
    /// The backend does nothing after this.
    fn take_shutdown(&mut self) -> Box<dyn FnOnce() + Send>;
}
