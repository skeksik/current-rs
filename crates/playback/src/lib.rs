//! Playback logic (ADR 0019): the [`Engine`] and [`MediaServer`] traits, the route planner and the playback [`Controller`]. No HTTP
//! and no libmpv, and no service: `current-jellyfin`'s client answers [`MediaServer`] and `current-mpv`'s engine implements
//! [`Engine`], joined by the composition root (`current-app`).

mod clock;
mod controller;
mod engine;
pub mod planner;
mod quality;
mod server;
mod status;
mod stream;

#[cfg(test)]
mod controller_tests;
#[cfg(test)]
mod test_support;

pub use clock::{Clock, SystemClock};
pub use controller::{
    AUTOPLAY_COUNTDOWN, Controller, NEXT_EPISODE_LEAD, PROGRESS_INTERVAL, STATE_INTERVAL,
};
pub use engine::{EndReason, Engine, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};
pub use planner::{PlanNote, Planned};
pub use quality::{
    Quality, QualityOption, QualityPreference, preferred_quality_options, quality_options,
    resolution_name, snap_quality,
};
pub use server::{
    MediaServer, PlanRequest, ProgressReport, ServerAnswer, StopReport, TranscodePermissions,
};
pub use status::{AutoSkip, NextEpisode, PlaybackDetails, PlayerEvent, PlayerStatus, Track};
pub use stream::{EngineStream, StreamAuth, StreamUrl};

/// The playback types of `current-model` this crate's API uses, so callers need not name both crates.
pub use current_model::{
    EngineCapabilities, EngineErrorKind, MediaSource, MediaStream, PlaybackPlan, PlaybackProblem,
    PlayerState, Route, RouteLabel, RouteReason, Segment, SegmentKind, SubtitleChoice, TrackChoice,
    TranscodingFacts,
};
