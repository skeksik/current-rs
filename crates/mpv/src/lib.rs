//! The libmpv engine (ADR 0019 "`current-mpv`", ADR 0007): hand-written declarations for libmpv's client API, the player handle with
//! its event thread, the options at start and per file, the display facts mapping, the swap chain reference for the UI, the build's
//! capabilities and the redaction of mpv's log.
//!
//! The only crate allowed `unsafe` (CLAUDE.md, ADR 0002): it calls libmpv's C API and COM's `IUnknown`. All of it sits in `sys`, the
//! engine handle and the swap chain, each block commented with its invariant; everything else is safe Rust and tested without libmpv.
//! The engine itself needs the `libmpv` feature, which links libmpv (crate manifest).

// Without the `libmpv` feature the engine is not built, and the parts only it uses (the declarations in `sys`, the per-file command,
// the swap chain's constructor) are unused; they are still compiled and tested.
#![cfg_attr(not(feature = "libmpv"), allow(dead_code))]

mod caps;
mod error;
mod event;
mod node;
mod options;
mod redact;
mod swapchain;
mod sys;

#[cfg(feature = "libmpv")]
mod engine;

#[cfg(test)]
mod live_tests;

pub use caps::BuildCapabilities;
#[cfg(feature = "libmpv")]
pub use engine::{Engine, EventSink};
pub use error::{EngineError, MpvError};
pub use event::{EndReason, EngineEvent, LogLine, PropertyChange, Track, VideoParams};
pub use options::{
    CompositionSize, DisplayFacts, EngineConfig, FileRequest, LogLevel, Output, StartOption,
    StreamCookie, SubtitleFile, TrackChoice, display_options, start_options, video_sync_mode,
};
pub use redact::{REDACTED, Redactor};
pub use swapchain::SwapChain;

/// The User-Agent of mpv's requests: `Current/<version>`.
pub fn user_agent() -> String {
    format!("Current/{}", env!("CARGO_PKG_VERSION"))
}
