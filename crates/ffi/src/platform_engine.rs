//! A platform's own player across the bridge (ADR 0025, "How a platform engine plugs into the core"): [`PlatformEngine`] is a
//! UniFFI foreign trait the UI implements (Windows: Media Foundation in C#, W4; Apple: AVPlayer in Swift, M3), and
//! [`Core::open_player_with_platform_engine`] opens a player with it beside libmpv. The engine answers through the
//! [`PlatformEngineEvents`] object it is handed.
//!
//! Threads: the core calls the engine from its player task. Each method must return quickly and never wait for the UI thread (the
//! platform hands the call to its dispatcher and returns). [`PlatformEngineEvents::send`] may be called from any thread and never
//! blocks.
//!
//! A URL crosses the bridge here for the first time: always the loopback relay's, holding the route's token only, never the session
//! token (ADR 0025, "URLs across the bridge"). The platform must not log it.
//!
//! Errors: a method returns `Err` for a failure the platform knows ([`PlatformEngineError::Failed`] with an engine error kind) and
//! UniFFI turns an exception or panic in the platform's code into [`PlatformEngineError::Unexpected`]
//! (`From<UnexpectedUniFFICallbackError>`, as UniFFI's manual recommends for foreign traits). Neither brings the core down: a failed
//! load is a failed route, and the player tries the next one; a failed command is dropped.

use std::sync::Arc;
use std::time::Duration;

use current_app::playback::{EndReason, EngineEvent, EngineTrack};
use current_app::{CodecAnswer, CodecKind, PlatformError, PlatformEvents, PlatformSubtitle};
use current_model::EngineErrorKind;

use crate::player::Player;
use crate::player_records::{CompositionSize, DisplayFacts};
use crate::{Core, CoreError};

/// What a platform engine call failed with.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PlatformEngineError {
    /// A failure the platform knows, by an engine error kind's name (`loading_failed`, `unsupported_format`, `decoder_failed`, or the
    /// platform's own: open values).
    #[error("the platform engine failed: {kind}")]
    Failed { kind: String },
    /// The platform's code threw or panicked.
    #[error("the platform engine failed unexpectedly")]
    Unexpected,
}

impl From<uniffi::UnexpectedUniFFICallbackError> for PlatformEngineError {
    fn from(_: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Unexpected
    }
}

impl From<PlatformEngineError> for PlatformError {
    fn from(error: PlatformEngineError) -> Self {
        match error {
            PlatformEngineError::Failed { kind } => Self::Failed(EngineErrorKind::new(kind)),
            PlatformEngineError::Unexpected => Self::Unexpected,
        }
    }
}

/// A track for the engine to show, by the server's stream index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlatformTrack {
    /// The engine's own choice.
    Auto,
    Off,
    Stream {
        index: u32,
    },
}

impl From<EngineTrack> for PlatformTrack {
    fn from(track: EngineTrack) -> Self {
        match track {
            EngineTrack::Auto => Self::Auto,
            EngineTrack::Off => Self::Off,
            EngineTrack::Stream(index) => Self::Stream { index },
        }
    }
}

/// One file to load. `url` is on the loopback relay (`http://127.0.0.1:<port>/<route token>/...`) and holds no session token; never
/// log, store or show it. Its `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PlatformFile {
    /// The load's number: every event about this file carries it ([`PlatformEngineEvents::send`]).
    pub generation: u64,
    pub url: String,
    pub start_ms: u64,
    pub audio: PlatformTrack,
    pub subtitle: PlatformTrack,
    /// The URL is an HLS playlist (Windows: open it with `AdaptiveMediaSource`).
    pub hls: bool,
}

impl std::fmt::Debug for PlatformFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformFile")
            .field("generation", &self.generation)
            .field("start_ms", &self.start_ms)
            .field("audio", &self.audio)
            .field("subtitle", &self.subtitle)
            .field("hls", &self.hls)
            .finish_non_exhaustive()
    }
}

fn platform_file(file: current_app::PlatformFile) -> PlatformFile {
    PlatformFile {
        generation: file.generation,
        // Moved as it is: never formatted, logged or shown.
        url: file.url.expose().to_owned(),
        start_ms: u64::try_from(file.start.as_millis()).unwrap_or(u64::MAX),
        audio: file.audio.into(),
        subtitle: file.subtitle.into(),
        hls: file.hls,
    }
}

/// A subtitle the core fetched, for the engine to show over the video (ADR 0025, "Subtitles on the native route").
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PlatformSubtitleFile {
    /// The server's stream index: later selections of it pick this subtitle.
    pub index: u32,
    /// The format by the server's name (`subrip`, `pgssub`, ...), if known.
    pub format: Option<String>,
    pub data: Vec<u8>,
    pub title: Option<String>,
    pub language: Option<String>,
    /// Show it at once.
    pub select: bool,
}

/// Why a file ended.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlatformEndReason {
    Eof,
    /// Stopped or replaced by the core's own command.
    Stopped,
    /// By an engine error kind's name.
    Error {
        kind: String,
    },
}

/// What a platform engine reports, about the load whose generation it is sent with.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum PlatformEvent {
    /// The file opened.
    Loaded,
    /// Playback (re)started after opening or a seek: the first frame shows.
    Restarted,
    Position {
        ms: u64,
    },
    Duration {
        ms: u64,
    },
    /// How far ahead of the position the engine holds data.
    Buffered {
        ms: u64,
    },
    Paused {
        paused: bool,
    },
    Buffering {
        buffering: bool,
    },
    Ended {
        reason: PlatformEndReason,
    },
    /// A selected track cannot be played although the file opened (Windows: its decoder status is not fully supported, or it raised
    /// `OpenFailed`; W1). The core moves the file to the next route.
    TrackFailed {
        kind: String,
    },
    /// Whether rates other than 1 work with the tracks playing (Windows: `IsSupportedPlaybackRateRange`). Report it after every load
    /// and on every track change.
    RatesSupported {
        supported: bool,
    },
}

impl From<PlatformEvent> for EngineEvent {
    fn from(event: PlatformEvent) -> Self {
        let ms = Duration::from_millis;
        match event {
            PlatformEvent::Loaded => Self::Loaded,
            PlatformEvent::Restarted => Self::Restarted,
            PlatformEvent::Position { ms: at } => Self::Position(ms(at)),
            PlatformEvent::Duration { ms: length } => Self::Duration(ms(length)),
            PlatformEvent::Buffered { ms: ahead } => Self::Buffered(ms(ahead)),
            PlatformEvent::Paused { paused } => Self::Paused(paused),
            PlatformEvent::Buffering { buffering } => Self::Buffering(buffering),
            PlatformEvent::Ended { reason } => Self::Ended(match reason {
                PlatformEndReason::Eof => EndReason::Eof,
                PlatformEndReason::Stopped => EndReason::Stopped,
                PlatformEndReason::Error { kind } => EndReason::Error(EngineErrorKind::new(kind)),
            }),
            PlatformEvent::TrackFailed { kind } => Self::TrackFailed(EngineErrorKind::new(kind)),
            PlatformEvent::RatesSupported { supported } => Self::RatesSupported(supported),
        }
    }
}

/// Where a platform engine sends its events: with the generation of the load they are about. The core drops an event of an older
/// load, or of any load after a stop. Any thread; never blocks.
#[derive(Debug, uniffi::Object)]
pub struct PlatformEngineEvents {
    events: PlatformEvents,
}

#[uniffi::export]
impl PlatformEngineEvents {
    pub fn send(&self, generation: u64, event: PlatformEvent) {
        self.events.send(generation, event.into());
    }
}

/// A platform's own player, implemented by the UI (ADR 0025). See the module's notes on threads, URLs and errors.
#[uniffi::export(with_foreign)]
pub trait PlatformEngine: Send + Sync {
    /// Called once, when the player opens, before anything else.
    fn attach(&self, events: Arc<PlatformEngineEvents>) -> Result<(), PlatformEngineError>;
    /// Replaces the file playing, if any.
    fn load(&self, file: PlatformFile) -> Result<(), PlatformEngineError>;
    fn add_subtitle(&self, subtitle: PlatformSubtitleFile) -> Result<(), PlatformEngineError>;
    fn set_paused(&self, paused: bool) -> Result<(), PlatformEngineError>;
    fn seek_to(&self, position_ms: u64) -> Result<(), PlatformEngineError>;
    fn select_audio(&self, track: PlatformTrack) -> Result<(), PlatformEngineError>;
    fn select_subtitle(&self, track: PlatformTrack) -> Result<(), PlatformEngineError>;
    /// 0 to 100.
    fn set_volume(&self, volume: u8) -> Result<(), PlatformEngineError>;
    fn set_muted(&self, muted: bool) -> Result<(), PlatformEngineError>;
    /// The playback rate, 1 for normal; within 0.01 to 100.
    fn set_speed(&self, speed: f64) -> Result<(), PlatformEngineError>;
    /// Unloads the file. Events about it after this are dropped.
    fn stop(&self) -> Result<(), PlatformEngineError>;
}

/// The foreign engine as `current-app` takes it.
struct Foreign(Arc<dyn PlatformEngine>);

impl current_app::PlatformEngine for Foreign {
    fn attach(&self, events: PlatformEvents) -> Result<(), PlatformError> {
        Ok(self.0.attach(Arc::new(PlatformEngineEvents { events }))?)
    }

    fn load(&self, file: current_app::PlatformFile) -> Result<(), PlatformError> {
        Ok(self.0.load(platform_file(file))?)
    }

    fn add_subtitle(&self, subtitle: PlatformSubtitle) -> Result<(), PlatformError> {
        Ok(self.0.add_subtitle(PlatformSubtitleFile {
            index: subtitle.index,
            format: subtitle.format.map(|format| format.as_str().to_owned()),
            data: subtitle.data,
            title: subtitle.title,
            language: subtitle.language,
            select: subtitle.select,
        })?)
    }

    fn set_paused(&self, paused: bool) -> Result<(), PlatformError> {
        Ok(self.0.set_paused(paused)?)
    }

    fn seek_to(&self, position: Duration) -> Result<(), PlatformError> {
        let ms = u64::try_from(position.as_millis()).unwrap_or(u64::MAX);
        Ok(self.0.seek_to(ms)?)
    }

    fn select_audio(&self, track: EngineTrack) -> Result<(), PlatformError> {
        Ok(self.0.select_audio(track.into())?)
    }

    fn select_subtitle(&self, track: EngineTrack) -> Result<(), PlatformError> {
        Ok(self.0.select_subtitle(track.into())?)
    }

    fn set_volume(&self, volume: u8) -> Result<(), PlatformError> {
        Ok(self.0.set_volume(volume)?)
    }

    fn set_muted(&self, muted: bool) -> Result<(), PlatformError> {
        Ok(self.0.set_muted(muted)?)
    }

    fn set_speed(&self, speed: f64) -> Result<(), PlatformError> {
        Ok(self.0.set_speed(speed)?)
    }

    fn stop(&self) -> Result<(), PlatformError> {
        Ok(self.0.stop()?)
    }
}

/// Which list a decoder is looked for in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CodecQueryKind {
    Video,
    Audio,
}

/// A decoder the UI is asked to look for on Windows: `CodecQuery.FindAllAsync(kind, CodecCategory.Decoder, subtype)` with the
/// value of the `Windows.Media.Core.CodecSubtypes` property named `subtype` (for example `VideoFormatHevc`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CodecQueryAsk {
    pub kind: CodecQueryKind,
    pub subtype: String,
}

/// The UI's answer: how many decoders `CodecQuery` returned for `subtype` (0 when the Store extension is missing).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CodecQueryAnswer {
    pub subtype: String,
    pub decoders: u32,
}

/// What the core asks the Windows UI to look for before it opens a player with Windows' own player (ADR 0025; the table is the
/// core's, the UI only answers).
#[uniffi::export]
pub fn windows_codec_queries() -> Vec<CodecQueryAsk> {
    current_app::windows_codec_queries()
        .into_iter()
        .map(|query| CodecQueryAsk {
            kind: match query.kind {
                CodecKind::Video => CodecQueryKind::Video,
                CodecKind::Audio => CodecQueryKind::Audio,
            },
            subtype: query.subtype,
        })
        .collect()
}

#[uniffi::export]
impl Core {
    /// Opens a player with the platform's own player beside this build's engine (ADR 0025): `engine` plays what Windows decodes,
    /// with `answers` to [`windows_codec_queries`] saying what that is; libmpv plays the rest, then the server. The routes, the
    /// info panel's words (ADR 0025's) and the fallback between engines are the core's. [`Core::open_player`] stays the player with
    /// libmpv alone.
    ///
    /// # Errors
    /// As [`Core::open_player`]; [`CoreError::Player`] with `EngineFailed` when the engine's `attach` failed.
    pub async fn open_player_with_platform_engine(
        &self,
        size: CompositionSize,
        display: DisplayFacts,
        engine: Arc<dyn PlatformEngine>,
        answers: Vec<CodecQueryAnswer>,
    ) -> Result<Arc<Player>, CoreError> {
        let app = self.app.clone();
        let answers: Vec<CodecAnswer> = answers
            .into_iter()
            .map(|answer| CodecAnswer {
                subtype: answer.subtype,
                decoders: answer.decoders,
            })
            .collect();
        let engine: Arc<dyn current_app::PlatformEngine> = Arc::new(Foreign(engine));
        let player = self
            .runtime
            .spawn(async move {
                app.open_player_with_platform_engine(size.into(), display.into(), engine, &answers)
                    .await
            })
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| crate::player::player_error(&error))?;
        Ok(self.player(player))
    }
}
