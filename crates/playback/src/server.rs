//! The media server trait: what a server must answer for playback. `current-jellyfin`'s client implements it in the composition root
//! (task 4); Stash scenes answer it later. Errors are already [`PlaybackProblem`]s: the implementation maps its server's errors (ADR
//! 0019, "Errors").

use std::future::Future;
use std::time::Duration;

use current_model::{
    EngineCapabilities, ItemRef, ItemSummary, MediaSource, MediaStream, PlaybackPreferences,
    PlaybackProblem, Route, RouteReason, Segment, TrackChoice, TranscodingFacts,
};

use crate::quality::Quality;
use crate::stream::{EngineStream, StreamUrl};

/// What a plan asks the server for (for Jellyfin, one `POST /Items/{id}/PlaybackInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRequest {
    /// The version the user picked; `None` for the server's first.
    pub media_source: Option<String>,
    pub start: Duration,
    pub tracks: TrackChoice,
    /// The implementation sends its server's ceiling for [`Quality::Maximum`], never leaving the bitrate to the server's default.
    pub quality: Quality,
    /// False only after a fallback (ADR 0019, choice 9).
    pub direct_play: bool,
}

/// The server's answer for the chosen source: the source, the route it offers and why, and the stream's URL on the server.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerAnswer {
    pub play_session_id: String,
    pub source: MediaSource,
    /// Before start, whether a server stream copies is predicted from the reasons; [`TranscodingFacts`] tell after start.
    pub route: Route,
    /// The server's reasons; empty for direct play.
    pub reasons: Vec<RouteReason>,
    /// On the server's address, with the session token; the engine gets [`MediaServer::stream_url`]'s.
    pub url: StreamUrl,
}

/// What the server lets the user do, read only to explain an outcome: the server enforces it (ADR 0019, "Permissions").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TranscodePermissions {
    pub video_transcoding: bool,
    pub remuxing: bool,
    pub force_remote_source_transcoding: bool,
    /// Bits per second, for addresses the server counts as remote.
    pub remote_bitrate_limit: Option<u32>,
}

impl Default for TranscodePermissions {
    /// What a server allows unless its administrator changes it.
    fn default() -> Self {
        Self {
            video_transcoding: true,
            remuxing: true,
            force_remote_source_transcoding: false,
            remote_bitrate_limit: None,
        }
    }
}

/// A report for the start and the progress of a playback (for Jellyfin `Sessions/Playing` and `/Progress`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressReport {
    pub item: ItemRef,
    pub media_source: String,
    pub play_session_id: String,
    pub position: Duration,
    /// The audio stream playing, by server index.
    pub audio: Option<u32>,
    /// The subtitle shown, by server index; `None` for none.
    pub subtitle: Option<u32>,
    pub paused: bool,
    pub muted: bool,
    /// 0 to 100.
    pub volume: u8,
    pub can_seek: bool,
    /// Gives the play method: direct play or transcode.
    pub route: Route,
}

/// A report that a playback stopped (for Jellyfin `Sessions/Playing/Stopped`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopReport {
    pub item: ItemRef,
    pub media_source: String,
    pub play_session_id: String,
    pub position: Duration,
    /// Playback ended with an error.
    pub failed: bool,
}

/// What a server answers for playback. The futures are `Send`, so the composition root can run the controller on a worker.
pub trait MediaServer {
    /// How to play `item` with an engine that has `capabilities`. `NotPlayable` when the server offers no route.
    fn playback_info(
        &self,
        item: &ItemRef,
        request: &PlanRequest,
        capabilities: &EngineCapabilities,
    ) -> impl Future<Output = Result<ServerAnswer, PlaybackProblem>> + Send;

    fn report_start(
        &self,
        report: &ProgressReport,
    ) -> impl Future<Output = Result<(), PlaybackProblem>> + Send;

    /// Also keeps the server's transcode alive.
    fn report_progress(
        &self,
        report: &ProgressReport,
    ) -> impl Future<Output = Result<(), PlaybackProblem>> + Send;

    /// Best effort, with a short timeout.
    fn report_stopped(
        &self,
        report: &StopReport,
    ) -> impl Future<Output = Result<(), PlaybackProblem>> + Send;

    /// The item's media segments; asked only when the source has segments.
    fn segments(
        &self,
        item: &ItemRef,
    ) -> impl Future<Output = Result<Vec<Segment>, PlaybackProblem>> + Send;

    /// The episode after `item`, if any; asked only for episodes.
    fn next_episode(
        &self,
        item: &ItemRef,
    ) -> impl Future<Output = Result<Option<ItemSummary>, PlaybackProblem>> + Send;

    /// The user's playback preferences (next-episode autoplay among them).
    fn preferences(
        &self,
    ) -> impl Future<Output = Result<PlaybackPreferences, PlaybackProblem>> + Send;

    fn permissions(
        &self,
    ) -> impl Future<Output = Result<TranscodePermissions, PlaybackProblem>> + Send;

    /// The server's facts about its transcode for this playback, if one runs.
    fn transcoding_facts(
        &self,
    ) -> impl Future<Output = Result<Option<TranscodingFacts>, PlaybackProblem>> + Send;

    /// An external subtitle's file, fetched by the core (with the address's credentials; a file on another host with none).
    fn external_subtitle(
        &self,
        item: &ItemRef,
        source: &MediaSource,
        stream: &MediaStream,
    ) -> impl Future<Output = Result<Vec<u8>, PlaybackProblem>> + Send;

    /// The URL the engine opens for `url`, the stream of a plan for `item` (ADR 0020): `url` itself on an address without gateway
    /// credentials, else a URL on the loopback relay that reaches only `item`'s streams and holds no credential (the relay adds the
    /// address's credentials and the session token upstream). Replaces the stream given before, whose relay route is removed.
    /// [`PlaybackProblem::LocalRelay`] when the relay cannot start (there is no fallback that hands the engine credentials),
    /// [`PlaybackProblem::Network`] when `url` is not on the address in use any more.
    fn stream_url(&self, item: &ItemRef, url: &StreamUrl) -> Result<EngineStream, PlaybackProblem>;

    /// The stream [`MediaServer::stream_url`] gave is no longer played (a stop, another item, a failure, the end): its relay route,
    /// if any, is removed, which closes the engine's open connections to it.
    fn end_stream(&self);

    /// Whether the address in use is a local one, for the remembered quality.
    fn is_local_address(&self) -> bool;
}
