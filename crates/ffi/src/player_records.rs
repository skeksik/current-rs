//! The player's records (ADR 0019, "What crosses `current-ffi`"): the state, the media controls' value, playback information and the
//! inputs (composition size, display facts, choices). Times are milliseconds. None of them has a URL, a path, a cookie or a header
//! value; `player_tests` checks every type in this file against a played item's URL, token and gateway values.

use std::time::Duration;

use current_app::playback::{NextEpisode, PlanNote, PlaybackDetails, PlayerStatus, Quality, Track};
use current_app::{EngineOutput, MediaStatus, NowPlaying, PlaybackInfo, PlayerEngine};
use current_model::{
    MediaSource, MediaStream, PlaybackProblem as ModelProblem, PlayerState, Route, RouteLabel,
    StreamAction, StreamProtocol, SubtitleChoice, TranscodingFacts,
};

use crate::Core;
use crate::browse::ItemRef;

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The panel's size in physical pixels (`ActualSize` × `CompositionScale`, ADR 0007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CompositionSize {
    pub width: u32,
    pub height: u32,
}

impl From<CompositionSize> for current_app::CompositionSize {
    fn from(size: CompositionSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

/// What the monitor the window is on reports (ADR 0007).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct DisplayFacts {
    /// The monitor is in HDR mode (advanced colour).
    pub hdr: bool,
    pub peak_nits: f32,
    pub sdr_white_nits: f32,
    pub refresh_hz: f64,
}

impl From<DisplayFacts> for current_app::DisplayFacts {
    fn from(facts: DisplayFacts) -> Self {
        Self {
            hdr: facts.hdr,
            peak_nits: facts.peak_nits,
            sdr_white_nits: facts.sdr_white_nits,
            refresh_hz: facts.refresh_hz,
        }
    }
}

/// The engine this build of the core has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlayerEngineKind {
    /// None: no player opens (`CoreError::Player` with `EngineUnavailable`).
    Unavailable,
    Libmpv,
    /// The test engine, which opens nothing: test builds only.
    Test,
}

impl From<PlayerEngine> for PlayerEngineKind {
    fn from(engine: PlayerEngine) -> Self {
        match engine {
            PlayerEngine::Unavailable => Self::Unavailable,
            PlayerEngine::Libmpv => Self::Libmpv,
            PlayerEngine::Test => Self::Test,
        }
    }
}

/// Why a player could not open or act.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlayerProblem {
    /// This build has no player engine.
    EngineUnavailable,
    /// Another player is still open (one per process for now).
    Busy,
    /// The engine could not start.
    EngineFailed { kind: String },
    /// The item's server is not signed in.
    NotSignedIn,
    /// A later play or stop replaced this action before it was done: not a failure; the state shows what plays now.
    Replaced,
    /// The player is closed.
    Closed,
}

impl From<&current_app::PlayerError> for PlayerProblem {
    fn from(error: &current_app::PlayerError) -> Self {
        (&current_app::PlayerProblem::from(error)).into()
    }
}

// One to one with `current-app`'s problems, both ways (`player_words` tests every variant).
impl From<&current_app::PlayerProblem> for PlayerProblem {
    fn from(problem: &current_app::PlayerProblem) -> Self {
        use current_app::PlayerProblem as P;
        match problem {
            P::EngineUnavailable => Self::EngineUnavailable,
            P::Busy => Self::Busy,
            P::EngineFailed { kind } => Self::EngineFailed { kind: kind.clone() },
            P::NotSignedIn => Self::NotSignedIn,
            P::Replaced => Self::Replaced,
            P::Closed => Self::Closed,
        }
    }
}

impl From<&PlayerProblem> for current_app::PlayerProblem {
    fn from(problem: &PlayerProblem) -> Self {
        use PlayerProblem as P;
        match problem {
            P::EngineUnavailable => Self::EngineUnavailable,
            P::Busy => Self::Busy,
            P::EngineFailed { kind } => Self::EngineFailed { kind: kind.clone() },
            P::NotSignedIn => Self::NotSignedIn,
            P::Replaced => Self::Replaced,
            P::Closed => Self::Closed,
        }
    }
}

/// Why playback cannot start or go on (ADR 0019, "Errors"). Flat, with no server text and no URL.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlaybackProblem {
    /// The server offers no way to play it, with the server's reasons (open names, `route_reason` values).
    NotPlayable {
        reasons: Vec<String>,
    },
    /// The engine failed: `kind` is an open name (`loading_failed`, `unsupported_format`, `decoder_failed`,
    /// `gateway_playlist_unchecked`, `engine_gone`, or one a later version adds).
    Engine {
        kind: String,
    },
    Network,
    /// A gateway in front of the server wants the user to sign in to it again (ADR 0017).
    GatewaySignInNeeded,
    /// The server ended the session: sign in again.
    SignedOut,
    Server {
        status: u16,
    },
    NotFound,
    /// The server's HLS playlist names another address or another item, or could not be read, so the local stream connection
    /// refused it (ADR 0020).
    PlaylistElsewhere,
    /// Current could not start its local stream connection (the loopback relay, ADR 0020). Shown as "Current could not start its
    /// local stream connection".
    LocalRelay,
}

impl From<&ModelProblem> for PlaybackProblem {
    fn from(problem: &ModelProblem) -> Self {
        (&current_app::PlaybackProblem::from(problem)).into()
    }
}

impl From<&current_app::PlaybackProblem> for PlaybackProblem {
    fn from(problem: &current_app::PlaybackProblem) -> Self {
        use current_app::PlaybackProblem as P;
        match problem {
            P::NotPlayable { reasons } => Self::NotPlayable {
                reasons: reasons.clone(),
            },
            P::Engine { kind } => Self::Engine { kind: kind.clone() },
            P::Network => Self::Network,
            P::GatewaySignInNeeded => Self::GatewaySignInNeeded,
            P::SignedOut => Self::SignedOut,
            P::Server { status } => Self::Server { status: *status },
            P::NotFound => Self::NotFound,
            P::PlaylistElsewhere => Self::PlaylistElsewhere,
            P::LocalRelay => Self::LocalRelay,
        }
    }
}

impl From<&PlaybackProblem> for current_app::PlaybackProblem {
    fn from(problem: &PlaybackProblem) -> Self {
        use PlaybackProblem as P;
        match problem {
            P::NotPlayable { reasons } => Self::NotPlayable {
                reasons: reasons.clone(),
            },
            P::Engine { kind } => Self::Engine { kind: kind.clone() },
            P::Network => Self::Network,
            P::GatewaySignInNeeded => Self::GatewaySignInNeeded,
            P::SignedOut => Self::SignedOut,
            P::Server { status } => Self::Server { status: *status },
            P::NotFound => Self::NotFound,
            P::PlaylistElsewhere => Self::PlaylistElsewhere,
            P::LocalRelay => Self::LocalRelay,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlayerStateKind {
    Idle,
    /// Planning: asking the server how to play.
    Preparing,
    Loading,
    Playing,
    Paused,
    Buffering,
    Seeking,
    Ended,
    /// See the snapshot's `problem`.
    Failed,
}

/// How the item reaches the engine, as the UI names it (ADR 0019, "Routes").
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RouteKind {
    DirectPlay,
    /// The server copies both streams into a new container.
    Remux,
    /// The server copies the video and converts the audio.
    DirectStream,
    Transcode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StreamProtocolKind {
    Hls,
    Progressive,
}

/// The route: its label and, for a server stream, the stream's protocol and container and what is copied.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RouteInfo {
    pub kind: RouteKind,
    pub protocol: Option<StreamProtocolKind>,
    pub container: Option<String>,
    pub video_copied: bool,
    pub audio_copied: bool,
}

impl From<&Route> for RouteInfo {
    fn from(route: &Route) -> Self {
        // One engine until W3 (ADR 0025): every local route is direct play, a server stream is named by what it copies.
        let kind = match route.label() {
            RouteLabel::Native | RouteLabel::DeviceRemux | RouteLabel::AppEngine => {
                RouteKind::DirectPlay
            }
            RouteLabel::ServerRemux => RouteKind::Remux,
            RouteLabel::ServerDirectStream => RouteKind::DirectStream,
            RouteLabel::ServerTranscode => RouteKind::Transcode,
        };
        match route {
            Route::Native | Route::DeviceRemux | Route::AppEngine => Self {
                kind,
                protocol: None,
                container: None,
                video_copied: true,
                audio_copied: true,
            },
            Route::ServerStream {
                protocol,
                container,
                video,
                audio,
                ..
            } => Self {
                kind,
                protocol: Some(match protocol {
                    StreamProtocol::Hls => StreamProtocolKind::Hls,
                    StreamProtocol::Progressive => StreamProtocolKind::Progressive,
                }),
                container: container.clone(),
                video_copied: *video == StreamAction::Copy,
                audio_copied: *audio == StreamAction::Copy,
            },
        }
    }
}

/// An audio or subtitle track for the menus, by the server's index.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TrackInfo {
    pub index: u32,
    /// The server's label; `None` when it has none.
    pub label: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub is_external: bool,
}

impl From<&Track> for TrackInfo {
    fn from(track: &Track) -> Self {
        Self {
            index: track.index,
            label: track.label.clone(),
            language: track.language.clone(),
            codec: track.codec.as_ref().map(|codec| codec.as_str().to_owned()),
            is_default: track.is_default,
            is_forced: track.is_forced,
            is_hearing_impaired: track.is_hearing_impaired,
            is_external: track.is_external,
        }
    }
}

/// A subtitle choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SubtitleSelection {
    /// The server's default for the user.
    Default,
    Off,
    Stream {
        index: u32,
    },
}

impl From<SubtitleChoice> for SubtitleSelection {
    fn from(choice: SubtitleChoice) -> Self {
        match choice {
            SubtitleChoice::Default => Self::Default,
            SubtitleChoice::Off => Self::Off,
            SubtitleChoice::Stream(index) => Self::Stream { index },
        }
    }
}

impl From<SubtitleSelection> for SubtitleChoice {
    fn from(selection: SubtitleSelection) -> Self {
        match selection {
            SubtitleSelection::Default => Self::Default,
            SubtitleSelection::Off => Self::Off,
            SubtitleSelection::Stream { index } => Self::Stream(index),
        }
    }
}

/// The most a stream may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum QualityChoice {
    /// The server's ceiling: only its own limits apply.
    Maximum,
    Bitrate {
        bits_per_second: u32,
    },
}

impl From<Quality> for QualityChoice {
    fn from(quality: Quality) -> Self {
        match quality {
            Quality::Maximum => Self::Maximum,
            Quality::Bitrate(bits_per_second) => Self::Bitrate { bits_per_second },
        }
    }
}

impl From<QualityChoice> for Quality {
    /// A bitrate of zero is no limit the server could meet: read as "Maximum", as the settings file reads it.
    fn from(choice: QualityChoice) -> Self {
        match choice {
            QualityChoice::Bitrate { bits_per_second } if bits_per_second > 0 => {
                Self::Bitrate(bits_per_second)
            }
            QualityChoice::Bitrate { .. } | QualityChoice::Maximum => Self::Maximum,
        }
    }
}

/// The steps of the quality menu, highest first, after "Maximum".
#[uniffi::export]
pub fn quality_steps() -> Vec<QualityChoice> {
    Quality::STEPS
        .iter()
        .map(|quality| (*quality).into())
        .collect()
}

/// A media segment (an intro, the credits, ...), for its skip button.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SegmentInfo {
    /// An open name: `intro`, `outro`, `recap`, `preview`, `commercial`, or the server's own.
    pub kind: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The "Next episode" card.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NextEpisodeInfo {
    pub item: ItemRef,
    pub name: String,
    pub series: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// The core's label for the numbers: `S1:E2` or empty.
    pub number: String,
    /// Whole seconds until it starts by itself, while the countdown runs.
    pub countdown: Option<u32>,
}

impl From<&NextEpisode> for NextEpisodeInfo {
    fn from(next: &NextEpisode) -> Self {
        let links = next.item.episode.as_ref();
        Self {
            item: (&next.item.item).into(),
            name: next.item.name.clone(),
            series: links.and_then(|links| links.series_name.clone()),
            season: links.and_then(|links| links.season_number),
            episode: links.and_then(|links| links.number),
            number: links.map(|links| links.number_label()).unwrap_or_default(),
            countdown: next.countdown,
        }
    }
}

/// The player's state: published at most 4 times a second for the position, at once on any other change.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PlayerStateSnapshot {
    pub state: PlayerStateKind,
    /// Set when `state` is `Failed`.
    pub problem: Option<PlaybackProblem>,
    pub item: Option<ItemRef>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    /// How far ahead of the position the engine holds data.
    pub buffered_ms: Option<u64>,
    pub audio_tracks: Vec<TrackInfo>,
    pub subtitle_tracks: Vec<TrackInfo>,
    /// The audio stream playing, by server index.
    pub audio: Option<u32>,
    /// The subtitle shown: never `Default` once playing.
    pub subtitle: SubtitleSelection,
    pub route: Option<RouteInfo>,
    /// The segment under the position.
    pub segment: Option<SegmentInfo>,
    pub next: Option<NextEpisodeInfo>,
    /// The quality in use: the entry of the item's quality menu it plays at (the highest not above the preferred quality), or the
    /// user's pick for it; the preferred quality while no item is playing. The menu ticks it.
    pub quality: QualityChoice,
    pub volume: u8,
    pub muted: bool,
}

impl From<&PlayerStatus> for PlayerStateSnapshot {
    fn from(status: &PlayerStatus) -> Self {
        let (state, problem) = match &status.state {
            PlayerState::Idle => (PlayerStateKind::Idle, None),
            PlayerState::Preparing => (PlayerStateKind::Preparing, None),
            PlayerState::Loading => (PlayerStateKind::Loading, None),
            PlayerState::Playing => (PlayerStateKind::Playing, None),
            PlayerState::Paused => (PlayerStateKind::Paused, None),
            PlayerState::Buffering => (PlayerStateKind::Buffering, None),
            PlayerState::Seeking => (PlayerStateKind::Seeking, None),
            PlayerState::Ended => (PlayerStateKind::Ended, None),
            PlayerState::Failed { problem } => (PlayerStateKind::Failed, Some(problem.into())),
        };
        Self {
            state,
            problem,
            item: status.item.as_ref().map(Into::into),
            position_ms: millis(status.position),
            duration_ms: status.duration.map(millis),
            buffered_ms: status.buffered.map(millis),
            audio_tracks: status.audio_tracks.iter().map(Into::into).collect(),
            subtitle_tracks: status.subtitle_tracks.iter().map(Into::into).collect(),
            audio: status.tracks.audio,
            subtitle: status.tracks.subtitle.into(),
            route: status.route.as_ref().map(Into::into),
            segment: status.segment.as_ref().map(|segment| SegmentInfo {
                kind: segment.kind.as_str().to_owned(),
                start_ms: millis(segment.start),
                end_ms: millis(segment.end),
            }),
            next: status.next.as_ref().map(Into::into),
            quality: status.quality.into(),
            volume: status.volume,
            muted: status.muted,
        }
    }
}

/// The playback status as the system media controls name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MediaStatusKind {
    Stopped,
    Changing,
    Playing,
    Paused,
}

/// What the system media controls show (ADR 0019): copy it into `SystemMediaTransportControls` and its display updater (type video),
/// and update the timeline when `timeline_revision` changes. Previous is a restart (seek to 0).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NowPlayingSnapshot {
    pub item: Option<ItemRef>,
    /// An open name: `movie`, `episode`, ...
    pub kind: Option<String>,
    pub title: Option<String>,
    pub series: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// What shows under the title for an episode: `Series · S1:E2`.
    pub subtitle: Option<String>,
    /// Encoded image bytes: a placeholder first, then the server's image.
    pub artwork: Option<Vec<u8>>,
    pub status: MediaStatusKind,
    pub position_ms: u64,
    /// The timeline's end; it starts at zero.
    pub duration_ms: Option<u64>,
    pub seekable_end_ms: Option<u64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_stop: bool,
    pub can_seek: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub timeline_revision: u64,
}

impl From<&NowPlaying> for NowPlayingSnapshot {
    fn from(now: &NowPlaying) -> Self {
        Self {
            item: now.item.as_ref().map(Into::into),
            kind: now.kind.as_ref().map(|kind| kind.as_str().to_owned()),
            title: now.title.clone(),
            series: now.series.clone(),
            season: now.season,
            episode: now.episode,
            subtitle: now.subtitle.clone(),
            artwork: now.artwork.as_ref().map(|bytes| bytes.to_vec()),
            status: match now.status {
                MediaStatus::Stopped => MediaStatusKind::Stopped,
                MediaStatus::Changing => MediaStatusKind::Changing,
                MediaStatus::Playing => MediaStatusKind::Playing,
                MediaStatus::Paused => MediaStatusKind::Paused,
            },
            position_ms: millis(now.position),
            duration_ms: now.duration.map(millis),
            seekable_end_ms: now.seekable_end.map(millis),
            can_play: now.can_play,
            can_pause: now.can_pause,
            can_stop: now.can_stop,
            can_seek: now.can_seek,
            can_next: now.can_next,
            can_previous: now.can_previous,
            timeline_revision: now.timeline_revision,
        }
    }
}

/// One stream of the source, for playback information.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct StreamInfo {
    pub index: u32,
    /// An open name: `video`, `audio`, `subtitle`, ...
    pub kind: String,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub language: Option<String>,
    pub label: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub frame_rate: Option<f32>,
    /// The server's range type (`SDR`, `HDR10`, `DOVI`, ...).
    pub range_type: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub is_external: bool,
}

impl From<&MediaStream> for StreamInfo {
    fn from(stream: &MediaStream) -> Self {
        let video = stream.video.as_ref();
        let audio = stream.audio.as_ref();
        Self {
            index: stream.index,
            kind: stream.kind.as_str().to_owned(),
            codec: stream.codec.as_ref().map(|codec| codec.as_str().to_owned()),
            profile: stream.profile.clone(),
            language: stream.language.clone(),
            label: stream
                .display_title
                .clone()
                .or_else(|| stream.title.clone()),
            width: video.and_then(|video| video.width),
            height: video.and_then(|video| video.height),
            bit_depth: video.and_then(|video| video.bit_depth),
            frame_rate: video.and_then(|video| video.frame_rate),
            range_type: video
                .and_then(|video| video.range_type.as_ref())
                .map(|range| range.as_str().to_owned()),
            channels: audio.and_then(|audio| audio.channels),
            channel_layout: audio.and_then(|audio| audio.layout.clone()),
            is_external: stream.is_external,
        }
    }
}

/// The source played (a version of the item), for playback information. No path.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct SourceInfo {
    pub name: Option<String>,
    pub container: Option<String>,
    pub size: Option<u64>,
    pub bitrate: Option<u32>,
    pub duration_ms: Option<u64>,
    pub streams: Vec<StreamInfo>,
}

impl From<&MediaSource> for SourceInfo {
    fn from(source: &MediaSource) -> Self {
        Self {
            name: source.name.clone(),
            container: source.container.clone(),
            size: source.size,
            bitrate: source.bitrate,
            duration_ms: source.duration.map(millis),
            streams: source.streams.iter().map(Into::into).collect(),
        }
    }
}

/// What explains a plan, from the user's permissions on the server.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlanNoteInfo {
    RemoteBitrateLimit { bits_per_second: u32 },
    TranscodingNotAllowed,
    RemuxingNotAllowed,
    RemoteSourceTranscodingForced,
}

impl From<&PlanNote> for PlanNoteInfo {
    fn from(note: &PlanNote) -> Self {
        match note {
            PlanNote::RemoteBitrateLimit { limit } => Self::RemoteBitrateLimit {
                bits_per_second: *limit,
            },
            PlanNote::TranscodingNotAllowed => Self::TranscodingNotAllowed,
            PlanNote::RemuxingNotAllowed => Self::RemuxingNotAllowed,
            PlanNote::RemoteSourceTranscodingForced => Self::RemoteSourceTranscodingForced,
        }
    }
}

/// What the server reports about its transcode.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TranscodingInfo {
    pub video_direct: bool,
    pub audio_direct: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub bitrate: Option<u32>,
    pub frame_rate: Option<f32>,
    /// Percent done.
    pub completion: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub audio_channels: Option<u32>,
    pub hardware_acceleration: Option<String>,
    pub reasons: Vec<String>,
}

impl From<&TranscodingFacts> for TranscodingInfo {
    fn from(facts: &TranscodingFacts) -> Self {
        Self {
            video_direct: facts.video_direct,
            audio_direct: facts.audio_direct,
            video_codec: facts.video_codec.as_ref().map(|c| c.as_str().to_owned()),
            audio_codec: facts.audio_codec.as_ref().map(|c| c.as_str().to_owned()),
            container: facts.container.clone(),
            bitrate: facts.bitrate,
            frame_rate: facts.frame_rate,
            completion: facts.completion,
            width: facts.width,
            height: facts.height,
            audio_channels: facts.audio_channels,
            hardware_acceleration: facts.hardware_acceleration.clone(),
            reasons: facts
                .reasons
                .iter()
                .map(|r| r.as_str().to_owned())
                .collect(),
        }
    }
}

/// What the engine reports about its decoding and output.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EngineOutputInfo {
    pub hardware_decoder: Option<String>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub pixel_format: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
}

impl From<&EngineOutput> for EngineOutputInfo {
    fn from(output: &EngineOutput) -> Self {
        let video = output.video.as_ref();
        Self {
            hardware_decoder: output.hardware_decoder.clone(),
            video_width: video.map(|video| video.width),
            video_height: video.map(|video| video.height),
            pixel_format: video.and_then(|video| video.pixel_format.clone()),
            primaries: video.and_then(|video| video.primaries.clone()),
            transfer: video.and_then(|video| video.transfer.clone()),
        }
    }
}

/// The playback information view: route, reasons, the source, what explains the plan, the server's transcoding facts and the
/// engine's output. No URL.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PlaybackInfoSnapshot {
    pub route: Option<RouteInfo>,
    /// Open names: the server's reasons and Current's own (`quality_limit`, `fallback_after_engine_error`, ...).
    pub reasons: Vec<String>,
    pub source: Option<SourceInfo>,
    pub notes: Vec<PlanNoteInfo>,
    pub transcoding: Option<TranscodingInfo>,
    pub engine: EngineOutputInfo,
}

impl From<&PlaybackInfo> for PlaybackInfoSnapshot {
    fn from(info: &PlaybackInfo) -> Self {
        let details: Option<&PlaybackDetails> = info.details.as_ref();
        let plan = details.and_then(|details| details.plan.as_ref());
        Self {
            route: plan.map(|plan| (&plan.route).into()),
            reasons: plan
                .map(|plan| plan.reasons.iter().map(|r| r.as_str().to_owned()).collect())
                .unwrap_or_default(),
            source: plan.map(|plan| (&plan.source).into()),
            notes: details
                .map(|details| details.notes.iter().map(Into::into).collect())
                .unwrap_or_default(),
            transcoding: details
                .and_then(|details| details.transcoding.as_ref())
                .map(Into::into),
            engine: (&info.engine).into(),
        }
    }
}

/// Settings → Playback (ADR 0019, choice 8).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PlaybackSettingsSnapshot {
    /// "Preferred quality": the most each item plays at, on local and internet addresses alike ("Maximum" by default).
    pub preferred_quality: QualityChoice,
    /// The segment kinds skipped automatically, by open name; empty when off (the default).
    pub auto_skip: Vec<String>,
    /// Whether intros and credits (`intro` and `outro`) are skipped: what the Settings switch shows.
    pub skips_intros_and_credits: bool,
    /// "Show resolution in quality menu": whether the menu shows each choice's resolution (on by default).
    pub shows_quality_resolution: bool,
    /// "Show all quality options": whether the menu lists every step below the original's bitrate (off by default).
    pub shows_all_qualities: bool,
}

impl PlaybackSettingsSnapshot {
    pub(crate) fn new(
        preferred_quality: QualityChoice,
        auto_skip: Vec<String>,
        shows_quality_resolution: bool,
        shows_all_qualities: bool,
    ) -> Self {
        let has = |kind: &str| auto_skip.iter().any(|k| k == kind);
        Self {
            preferred_quality,
            skips_intros_and_credits: has("intro") && has("outro"),
            auto_skip,
            shows_quality_resolution,
            shows_all_qualities,
        }
    }
}

/// One entry of the quality menu, as the core names it: the choice, the resolution the server would send it at (`4K`, `1080p`...;
/// none when not known) and its bitrate (the original's own for `Maximum`; none when not known).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QualityOptionInfo {
    pub quality: QualityChoice,
    pub resolution: Option<String>,
    pub bitrate: Option<u32>,
}

impl From<&current_app::playback::QualityOption> for QualityOptionInfo {
    fn from(option: &current_app::playback::QualityOption) -> Self {
        Self {
            quality: option.quality.into(),
            resolution: option.resolution.map(str::to_owned),
            bitrate: option.bitrate,
        }
    }
}

impl From<&QualityOptionInfo> for current_app::QualityEntry {
    fn from(option: &QualityOptionInfo) -> Self {
        Self {
            quality: option.quality.into(),
            resolution: option.resolution.clone(),
            bitrate: option.bitrate,
        }
    }
}

/// One line of the quality menu: its entry, its label ("Original · 4K · 26.1 Mbit/s") and whether it is the quality in use (its
/// radio item checked).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QualityMenuEntry {
    pub option: QualityOptionInfo,
    pub label: String,
    pub checked: bool,
}

#[uniffi::export]
impl Core {
    /// The UI's decimal separator ("." or ",", from the user's region), for the labels with decimals: a platform fact passed once
    /// at start. "." until then; an empty one changes nothing.
    pub fn set_decimal_separator(&self, separator: String) {
        self.app.set_decimal_separator(&separator);
    }

    /// A bitrate as the player shows it: "40 Mbit/s", "1.5 Mbit/s" (with the UI's decimal separator), "720 kbit/s" below 1 Mbit/s.
    pub fn bitrate_label(&self, bits_per_second: u32) -> String {
        current_app::bitrate_label(bits_per_second, &self.app.number_format())
    }

    /// A quality entry's label: "Original · 1080p · 5.2 Mbit/s", "720p · 4 Mbit/s", without the resolution unless `with_resolution`
    /// (Settings' "Show resolution in quality menu"); the parts not known are left out.
    pub fn quality_label(&self, option: QualityOptionInfo, with_resolution: bool) -> String {
        current_app::quality_label(
            &(&option).into(),
            with_resolution,
            &self.app.number_format(),
        )
    }

    /// The quality menu while playing at `current`: `options` as [`crate::Player::quality_options`] named them for the
    /// version playing, or `None` before then for "Original" and the common steps. A limit below or between the entries is listed
    /// as its own entry in bitrate order ("Original" stays first); the entry in use is checked.
    pub fn quality_menu(
        &self,
        options: Option<Vec<QualityOptionInfo>>,
        current: QualityChoice,
        with_resolution: bool,
    ) -> Vec<QualityMenuEntry> {
        let entries = options.map(|options| options.iter().map(Into::into).collect());
        current_app::quality_menu(
            entries,
            current.into(),
            with_resolution,
            &self.app.number_format(),
        )
        .into_iter()
        .map(|item| QualityMenuEntry {
            option: QualityOptionInfo {
                quality: item.entry.quality.into(),
                resolution: item.entry.resolution,
                bitrate: item.entry.bitrate,
            },
            label: item.label,
            checked: item.checked,
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quality_menu_is_labelled_with_the_uis_decimal_separator() {
        let core = Core::new().unwrap();
        let menu = core.quality_menu(
            Some(vec![QualityOptionInfo {
                quality: QualityChoice::Maximum,
                resolution: Some("4K".into()),
                bitrate: Some(26_100_000),
            }]),
            QualityChoice::Bitrate {
                bits_per_second: 10_000_000,
            },
            true,
        );
        let labels: Vec<(&str, bool)> = menu
            .iter()
            .map(|entry| (entry.label.as_str(), entry.checked))
            .collect();
        assert_eq!(
            labels,
            [("Original · 4K · 26.1 Mbit/s", false), ("10 Mbit/s", true)]
        );
        core.set_decimal_separator(",".into());
        assert_eq!(core.bitrate_label(1_500_000), "1,5 Mbit/s");
        let fallback = core.quality_menu(None, QualityChoice::Maximum, true);
        assert_eq!(fallback.len(), 6);
        assert!(fallback[0].checked);
        assert_eq!(fallback[0].label, "Original");
    }
}
