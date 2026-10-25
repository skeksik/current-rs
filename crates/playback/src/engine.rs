//! The engine trait: what the controller asks of a player engine (libmpv in `current-mpv`, AVPlayer later) and what it hears back.
//!
//! One engine per player, owned by the controller. A new file (the next episode, a re-plan, the fallback) goes to the same engine
//! through [`Engine::load`], which replaces the file playing: nothing here makes a second engine, and `current-mpv` allows only one
//! libmpv core per process for now (owner, 2026-10-25).

use std::time::Duration;

use current_model::{EngineCapabilities, EngineErrorKind, SubtitleFormat};

use crate::stream::{StreamAuth, StreamUrl};

/// A track for the engine to show, by the server's stream index. The engine maps the index onto its own tracks (for libmpv through
/// `ff-index`; that it equals Jellyfin's index is checked on real files in task 5, ADR 0019 "Tracks").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EngineTrack {
    /// The engine's own choice.
    #[default]
    Auto,
    Off,
    /// The stream with this server index.
    Stream(u32),
}

/// One file to load: the URL, the credentials for it, where to start and which tracks to show from the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineFile {
    pub url: StreamUrl,
    pub auth: StreamAuth,
    /// For libmpv the per-file `start` option, for direct play and HLS alike (ADR 0019, "Start and resume").
    pub start: Duration,
    pub audio: EngineTrack,
    pub subtitle: EngineTrack,
}

/// An external subtitle the core fetched itself (ADR 0019, "External subtitles"), for the engine to add to the file playing (for
/// libmpv as `memory://` for text or `hex://` for binary formats), so the engine never fetches a subtitle.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalSubtitle {
    /// The server's stream index, so later selections of this index pick the added track.
    pub index: u32,
    pub format: Option<SubtitleFormat>,
    pub data: Vec<u8>,
    pub title: Option<String>,
    pub language: Option<String>,
    /// Show it at once.
    pub select: bool,
}

impl std::fmt::Debug for ExternalSubtitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalSubtitle")
            .field("index", &self.index)
            .field("format", &self.format)
            .field("bytes", &self.data.len())
            .field("title", &self.title)
            .field("language", &self.language)
            .field("select", &self.select)
            .finish()
    }
}

/// A player engine. Commands do not block (libmpv's are asynchronous) and failures after a command arrive as [`EngineEvent`]s;
/// [`Engine::load`] fails at once only when the engine cannot take a file at all.
pub trait Engine {
    /// What the engine plays, for the device profile.
    fn capabilities(&self) -> EngineCapabilities;
    /// Replaces the file playing, if any, with `file`.
    fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind>;
    fn add_subtitle(&mut self, subtitle: ExternalSubtitle);
    fn set_paused(&mut self, paused: bool);
    fn seek_to(&mut self, position: Duration);
    fn select_audio(&mut self, track: EngineTrack);
    fn select_subtitle(&mut self, track: EngineTrack);
    /// 0 to 100.
    fn set_volume(&mut self, volume: u8);
    fn set_muted(&mut self, muted: bool);
    /// Unloads the file and keeps the engine for the next one.
    fn stop(&mut self);
}

/// What an engine reports. The composition root turns the engine's own events into these (for libmpv on its event thread) and hands
/// them to the controller; the event thread calls no listener (ADR 0019, "Threads and event rate").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// The file is open (libmpv's `FILE_LOADED`).
    Loaded,
    /// Playback (re)started after loading or a seek (libmpv's `PLAYBACK_RESTART`).
    Restarted,
    Position(Duration),
    Duration(Duration),
    /// How far ahead of the position the engine holds data.
    Buffered(Duration),
    /// The engine paused or resumed, by the user or by itself.
    Paused(bool),
    /// Waiting for data (libmpv's `paused-for-cache`).
    Buffering(bool),
    /// The file ended. With libmpv's `keep-open=yes` the end of the file is `eof-reached`, which the composition root reports as
    /// [`EndReason::Eof`].
    Ended(EndReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    /// Stopped or replaced by a command; the controller ignores it, since it gave the command.
    Stopped,
    Error(EngineErrorKind),
}
