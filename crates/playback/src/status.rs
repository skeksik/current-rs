//! What the controller publishes: the player's state for the UI, the "stopped" event for the rest of the app, the remembered quality
//! and the playback information. No URL, token, cookie or header value is in any of these.

use std::collections::BTreeSet;
use std::time::Duration;

use current_model::{
    Codec, ItemRef, ItemSummary, MediaStream, PlaybackPlan, PlayerState, Route, Segment,
    SegmentKind, TrackChoice, TranscodingFacts,
};

use crate::planner::PlanNote;
use crate::quality::Quality;

/// The automatic segment skipping setting (ADR 0019, decided choice 8): the segment kinds the controller skips itself. Off by
/// default. Kinds are open values, so later kinds (recap, preview) can join without a migration; the composition root stores it
/// (task 4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AutoSkip {
    kinds: BTreeSet<SegmentKind>,
}

impl AutoSkip {
    pub fn off() -> Self {
        Self::default()
    }

    /// The setting the Settings page switches on: intros and credits (the `Intro` and `Outro` segments).
    pub fn intros_and_credits() -> Self {
        Self::new([SegmentKind::INTRO, SegmentKind::OUTRO])
    }

    pub fn new(kinds: impl IntoIterator<Item = SegmentKind>) -> Self {
        Self {
            kinds: kinds.into_iter().collect(),
        }
    }

    pub fn skips(&self, kind: &SegmentKind) -> bool {
        self.kinds.contains(kind)
    }

    pub fn is_off(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn kinds(&self) -> impl Iterator<Item = &SegmentKind> {
        self.kinds.iter()
    }
}

/// An audio or subtitle track for the menus, by the server's index.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub index: u32,
    /// The server's label; `None` when it has none, and the UI names the track itself.
    pub label: Option<String>,
    pub language: Option<String>,
    pub codec: Option<Codec>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub is_external: bool,
}

impl Track {
    pub(crate) fn from_stream(stream: &MediaStream) -> Self {
        Self {
            index: stream.index,
            label: stream
                .display_title
                .clone()
                .or_else(|| stream.title.clone()),
            language: stream.language.clone(),
            codec: stream.codec.clone(),
            is_default: stream.is_default,
            is_forced: stream.is_forced,
            is_hearing_impaired: stream.is_hearing_impaired,
            is_external: stream.is_external,
        }
    }
}

/// The "Next episode" card.
#[derive(Clone, Debug, PartialEq)]
pub struct NextEpisode {
    pub item: ItemSummary,
    /// Whole seconds until it starts by itself, while the autoplay countdown runs.
    pub countdown: Option<u32>,
}

/// The player's state for the UI: one value, published at most 4 times a second for the position and at once on any other change
/// (ADR 0019, "Threads and event rate").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerStatus {
    pub state: PlayerState,
    pub item: Option<ItemRef>,
    pub position: Duration,
    pub duration: Option<Duration>,
    /// How far ahead of the position the engine holds data.
    pub buffered: Option<Duration>,
    pub audio_tracks: Vec<Track>,
    pub subtitle_tracks: Vec<Track>,
    /// The tracks playing, resolved: no `Default` subtitle.
    pub tracks: TrackChoice,
    pub route: Option<Route>,
    /// The segment under the position, for its skip button.
    pub segment: Option<Segment>,
    pub next: Option<NextEpisode>,
    /// The quality limit for the address in use.
    pub quality: Quality,
    pub volume: u8,
    pub muted: bool,
}

/// What the controller tells the composition root, drained with [`Controller::take_events`](crate::Controller::take_events).
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    State(PlayerStatus),
    /// Playback of an item ended: stopped by the user, played to the end, replaced by another item, or failed. Once per item played,
    /// after the server was told. For lists that show progress (ADR 0018).
    Stopped {
        item: ItemRef,
        position: Duration,
        failed: bool,
    },
    /// The user picked a quality; the composition root saves it for that kind of address.
    QualityChanged { local: bool, quality: Quality },
}

/// What the playback information view shows: the plan (route, reasons, source), what explains it, and the server's facts about its
/// transcode. No URL.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackDetails {
    pub plan: Option<PlaybackPlan>,
    pub notes: Vec<PlanNote>,
    pub transcoding: Option<TranscodingFacts>,
}
