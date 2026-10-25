//! What the system media controls show (ADR 0019, "Media keys, the media flyout and full screen"): the item, its artwork, the playback
//! status, the timeline and the commands that work now. The UI copies it into `SystemMediaTransportControls` and forwards button
//! presses and position requests as the player's actions; previous is a restart (seek to 0).
//!
//! The timeline revision changes on every seek completion, state change and every [`TIMELINE_INTERVAL`] while playing, so the UI updates
//! the flyout's timeline when it changes (a stale position stays until the timeline is updated).

use std::sync::Arc;
use std::time::Duration;

use current_model::{ItemRef, ItemSummary, MediaKind, PlayerState};
use current_playback::PlayerStatus;

/// How often the timeline is renewed while playing.
pub const TIMELINE_INTERVAL: Duration = Duration::from_secs(5);

/// The playback status as the media controls name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MediaStatus {
    /// Nothing is playing: no item, ended or failed.
    #[default]
    Stopped,
    /// Preparing, loading, buffering or seeking.
    Changing,
    Playing,
    Paused,
}

/// The media controls' value. Artwork is encoded image bytes (or a BlurHash placeholder until the image loads); no URL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub item: Option<ItemRef>,
    pub kind: Option<MediaKind>,
    pub title: Option<String>,
    /// For an episode: the series' name and the season and episode numbers.
    pub series: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub artwork: Option<Arc<[u8]>>,
    pub status: MediaStatus,
    pub position: Duration,
    /// The timeline's end; the start is always zero.
    pub duration: Option<Duration>,
    /// The end of the range a seek may reach: the duration when known.
    pub seekable_end: Option<Duration>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_stop: bool,
    pub can_seek: bool,
    /// A next episode exists.
    pub can_next: bool,
    /// Previous restarts the item.
    pub can_previous: bool,
    pub timeline_revision: u64,
}

pub(crate) fn status(state: &PlayerState) -> MediaStatus {
    match state {
        PlayerState::Idle | PlayerState::Ended | PlayerState::Failed { .. } => MediaStatus::Stopped,
        PlayerState::Preparing
        | PlayerState::Loading
        | PlayerState::Buffering
        | PlayerState::Seeking => MediaStatus::Changing,
        PlayerState::Playing => MediaStatus::Playing,
        PlayerState::Paused => MediaStatus::Paused,
    }
}

/// The value for `status`, with the item's `summary` (when it loaded), its `artwork`, whether a next episode exists and the timeline
/// `revision`.
pub(crate) fn now_playing(
    status: &PlayerStatus,
    summary: Option<&ItemSummary>,
    artwork: Option<Arc<[u8]>>,
    has_next: bool,
    revision: u64,
) -> NowPlaying {
    let media = self::status(&status.state);
    let active = status.state.is_active();
    let episode = summary.and_then(|summary| summary.episode.as_ref());
    let has_item = status.item.is_some();
    NowPlaying {
        item: status.item.clone(),
        kind: summary.map(|summary| summary.kind.clone()),
        title: summary.map(|summary| summary.name.clone()),
        series: episode.and_then(|links| links.series_name.clone()),
        season: episode.and_then(|links| links.season_number),
        episode: episode.and_then(|links| links.number),
        artwork: if has_item { artwork } else { None },
        status: media,
        position: status.position,
        duration: status.duration,
        seekable_end: status.duration.filter(|_| active),
        can_play: has_item && status.state != PlayerState::Playing,
        can_pause: status.state == PlayerState::Playing,
        can_stop: has_item && media != MediaStatus::Stopped,
        can_seek: active && status.duration.is_some(),
        can_next: has_next,
        can_previous: active,
        timeline_revision: revision,
    }
}

#[cfg(test)]
mod tests {
    use current_model::{EpisodeLinks, ItemId, ServerKey, ServerRef, ServiceType, UserState};

    use super::*;

    #[test]
    fn the_commands_follow_the_state() {
        let item = ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new("1"),
        );
        let summary = ItemSummary {
            item: item.clone(),
            kind: MediaKind::EPISODE,
            name: "Example Episode".into(),
            year: None,
            runtime: None,
            artwork: Vec::new(),
            user: UserState::default(),
            episode: Some(EpisodeLinks {
                series: None,
                series_name: Some("Example Series".into()),
                season: None,
                season_name: None,
                number: Some(2),
                number_end: None,
                season_number: Some(1),
            }),
            child_count: None,
        };
        let mut status = PlayerStatus {
            state: PlayerState::Playing,
            item: Some(item),
            position: Duration::from_secs(30),
            duration: Some(Duration::from_secs(600)),
            ..PlayerStatus::default()
        };
        let playing = now_playing(&status, Some(&summary), None, true, 3);
        assert_eq!(playing.status, MediaStatus::Playing);
        assert_eq!(playing.title.as_deref(), Some("Example Episode"));
        assert_eq!(playing.series.as_deref(), Some("Example Series"));
        assert_eq!((playing.season, playing.episode), (Some(1), Some(2)));
        assert!(playing.can_pause && !playing.can_play && playing.can_seek && playing.can_next);
        assert_eq!(playing.seekable_end, Some(Duration::from_secs(600)));
        assert_eq!(playing.timeline_revision, 3);

        status.state = PlayerState::Paused;
        let paused = now_playing(&status, Some(&summary), None, false, 4);
        assert!(paused.can_play && !paused.can_pause && !paused.can_next);
        status.state = PlayerState::Seeking;
        assert_eq!(
            now_playing(&status, None, None, false, 5).status,
            MediaStatus::Changing
        );
        let idle = now_playing(&PlayerStatus::default(), None, None, false, 6);
        assert_eq!(idle.status, MediaStatus::Stopped);
        assert!(!idle.can_play && !idle.can_stop && !idle.can_previous);
    }
}
