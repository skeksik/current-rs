//! Playback logic (ADR 0019): the [`Engine`] and [`MediaServer`] traits, the route planner and the playback [`Controller`]. No HTTP
//! and no libmpv, and no service: `current-jellyfin`'s client answers [`MediaServer`] and `current-mpv`'s engine implements
//! [`Engine`], joined by the composition root (`current-app`).

mod clock;
mod controller;
mod engine;
mod engines;
pub mod planner;
mod quality;
pub mod route;
mod server;
mod status;
mod stream;

#[cfg(test)]
mod controller_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod two_engine_tests;

pub use clock::{Clock, SystemClock};
pub use controller::{
    AUTOPLAY_COUNTDOWN, Controller, NEXT_EPISODE_LEAD, NativeEngine, PROGRESS_INTERVAL,
    STATE_INTERVAL,
};
pub use engine::{EndReason, Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};
pub use engines::{DeviceRemux, DisplayKind, EngineProfile, Engines, PlatformRules};
pub use planner::{PlanNote, Planned, RouteInputs};
pub use quality::{
    Quality, QualityOption, QualityPreference, QualityTarget, listed_quality,
    preferred_quality_options, quality_options, resolution_name, snap_quality,
};
pub use route::{RouteChoice, RouteContext};
pub use server::{
    MediaServer, PlanRequest, ProgressReport, ServerAnswer, StopReport, TranscodePermissions,
};
pub use status::{AutoSkip, NextEpisode, PlaybackDetails, PlayerEvent, PlayerStatus, Track};
pub use stream::{EngineStream, StreamAuth, StreamUrl};

/// The playback types of `current-model` this crate's API uses, so callers need not name both crates.
pub use current_model::{
    EngineCapabilities, EngineErrorKind, EngineRole, GapKind, MediaSource, MediaStream,
    PlaybackPlan, PlaybackProblem, PlayerState, Route, RouteLabel, RouteReason, Segment,
    SegmentKind, ServerOffer, SubtitleChoice, TrackChoice, TrackGap, TranscodingFacts,
};
