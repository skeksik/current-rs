//! Items as browsing shows them: summaries for grids and rows, details for a detail page, libraries, people (ADR 0018).

use std::time::Duration;

use crate::artwork::Artwork;
use crate::identity::{ItemRef, MediaKind};
use crate::media::MediaSummary;
use crate::open::open_value;

/// What the signed-in user has done with an item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserState {
    pub played: bool,
    /// Where playback stopped, if the item can be resumed.
    pub position: Option<Duration>,
    /// How much was played, 0.0 to 1.0, if the server says.
    pub progress: Option<f32>,
    pub favourite: bool,
    /// How many items inside are unplayed: series, seasons and folders.
    pub unplayed_count: Option<u32>,
    pub play_count: u32,
}

/// Where an episode sits: its series and season.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpisodeLinks {
    pub series: Option<ItemRef>,
    pub series_name: Option<String>,
    pub season: Option<ItemRef>,
    pub season_name: Option<String>,
    /// The episode's number in its season.
    pub number: Option<u32>,
    /// The last episode's number when one file holds several.
    pub number_end: Option<u32>,
    pub season_number: Option<u32>,
}

/// What a grid tile or a row shows of an item.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemSummary {
    pub item: ItemRef,
    pub kind: MediaKind,
    pub name: String,
    pub year: Option<i32>,
    pub runtime: Option<Duration>,
    /// At most one image per image type, so each layout can pick primary, thumb or backdrop.
    pub artwork: Vec<Artwork>,
    pub user: UserState,
    /// Set for episodes.
    pub episode: Option<EpisodeLinks>,
    /// How many items the server grouped into this one: the new episodes of a series in "latest".
    pub child_count: Option<u32>,
}

open_value! {
    /// What a library holds. Open: each service crate maps its server's library types to these where the meaning matches and passes
    /// any other type through unchanged.
    CollectionKind {
        MOVIES = "movies",
        TV_SHOWS = "tv_shows",
        MUSIC = "music",
        MUSIC_VIDEOS = "music_videos",
        HOME_VIDEOS = "home_videos",
        BOX_SETS = "box_sets",
        BOOKS = "books",
        PHOTOS = "photos",
        PLAYLISTS = "playlists",
    }
}

/// A library (a Jellyfin user view), in the order the server returns them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub item: ItemRef,
    pub name: String,
    /// `None` for a library of mixed content, which the server gives no type.
    pub collection: Option<CollectionKind>,
    pub artwork: Vec<Artwork>,
}

open_value! {
    /// A person's part in an item. Open: unknown parts pass through unchanged.
    PersonKind {
        ACTOR = "actor",
        DIRECTOR = "director",
        WRITER = "writer",
        PRODUCER = "producer",
        GUEST_STAR = "guest_star",
        COMPOSER = "composer",
    }
}

/// Someone in an item's cast or crew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub person: ItemRef,
    pub name: String,
    /// The character played, for an actor.
    pub role: Option<String>,
    pub kind: PersonKind,
    pub image: Option<Artwork>,
}

/// A day in the calendar, as a server writes it. The model has no date type and takes no dependency for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarDate {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
}

impl CalendarDate {
    /// The date, or `None` if the month or day cannot exist (February 30th included).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(year) => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days)
            .contains(&day)
            .then_some(Self { year, month, day })
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// A named point in an item's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chapter {
    pub start: Duration,
    pub name: Option<String>,
}

/// What a detail page shows of an item.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDetails {
    pub summary: ItemSummary,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub taglines: Vec<String>,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub people: Vec<Person>,
    /// The age rating as the server writes it.
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub premiere: Option<CalendarDate>,
    pub end: Option<CalendarDate>,
    /// A series' state ("Continuing", "Ended"), as the server writes it.
    pub status: Option<String>,
    /// Every backdrop, in the server's order.
    pub backdrops: Vec<Artwork>,
    pub logo: Option<Artwork>,
    pub chapters: Vec<Chapter>,
    /// The default version's media, for items that play.
    pub media: Option<MediaSummary>,
}

/// What a Play button starts (ADR 0019, task 5): the item that plays, which for a series is its next episode, and where. The details
/// screen owns the choice of resuming or starting over (ADR 0019, "Start and resume"): it offers both when [`PlayTarget::resumes`].
#[derive(Clone, Debug, PartialEq)]
pub struct PlayTarget {
    pub item: ItemSummary,
    /// The resume point, or zero.
    pub start: Duration,
}

impl PlayTarget {
    /// Kinds that play by themselves: a film, an episode, a video, a music video. A series plays through its next episode; other
    /// kinds (audio, folders, collections) have no player yet.
    pub fn plays(kind: &MediaKind) -> bool {
        [
            MediaKind::MOVIE,
            MediaKind::EPISODE,
            MediaKind::VIDEO,
            MediaKind::MUSIC_VIDEO,
        ]
        .contains(kind)
    }

    /// Play for `item` itself: from its resume point unless it is played (a played item with a position left over starts again),
    /// else from the start. `None` for a kind that does not play by itself.
    pub fn for_item(item: &ItemSummary) -> Option<Self> {
        if !Self::plays(&item.kind) {
            return None;
        }
        let start = match item.user.position {
            Some(position) if !item.user.played => position,
            _ => Duration::ZERO,
        };
        Some(Self {
            item: item.clone(),
            start,
        })
    }

    /// Whether Play resumes: the button says "Resume", and "Play from the beginning" goes with it.
    pub fn resumes(&self) -> bool {
        !self.start.is_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ItemId, ServerKey, ServerRef, ServiceType};

    fn summary(kind: MediaKind, user: UserState) -> ItemSummary {
        ItemSummary {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                ItemId::new("42"),
            ),
            kind,
            name: "Example".to_owned(),
            year: None,
            runtime: None,
            artwork: Vec::new(),
            user,
            episode: None,
            child_count: None,
        }
    }

    #[test]
    fn play_resumes_from_the_position_unless_played() {
        let started = UserState {
            position: Some(Duration::from_secs(600)),
            ..UserState::default()
        };
        let target = PlayTarget::for_item(&summary(MediaKind::MOVIE, started.clone())).unwrap();
        assert_eq!(target.start, Duration::from_secs(600));
        assert!(target.resumes());

        let played = UserState {
            played: true,
            ..started
        };
        let target = PlayTarget::for_item(&summary(MediaKind::EPISODE, played)).unwrap();
        assert_eq!(target.start, Duration::ZERO);
        assert!(!target.resumes());

        let fresh = PlayTarget::for_item(&summary(MediaKind::VIDEO, UserState::default())).unwrap();
        assert!(!fresh.resumes());
    }

    #[test]
    fn only_kinds_that_play_by_themselves_get_a_target() {
        for kind in [
            MediaKind::SERIES,
            MediaKind::SEASON,
            MediaKind::AUDIO,
            MediaKind::BOX_SET,
            MediaKind::new("SomeFutureKind"),
        ] {
            assert!(PlayTarget::for_item(&summary(kind, UserState::default())).is_none());
        }
        assert!(
            PlayTarget::for_item(&summary(MediaKind::MUSIC_VIDEO, UserState::default())).is_some()
        );
    }

    #[test]
    fn dates_that_cannot_exist_are_refused() {
        assert!(CalendarDate::new(2024, 2, 29).is_some());
        assert!(CalendarDate::new(2023, 2, 29).is_none());
        assert!(CalendarDate::new(1900, 2, 29).is_none());
        assert!(CalendarDate::new(2000, 2, 29).is_some());
        assert!(CalendarDate::new(2024, 4, 31).is_none());
        assert!(CalendarDate::new(2024, 13, 1).is_none());
        assert!(CalendarDate::new(2024, 1, 0).is_none());
        assert_eq!(
            CalendarDate::new(2010, 7, 16),
            Some(CalendarDate {
                year: 2010,
                month: 7,
                day: 16
            })
        );
    }

    #[test]
    fn dates_order_by_day() {
        let earlier = CalendarDate::new(2009, 12, 31);
        let later = CalendarDate::new(2010, 1, 1);
        assert!(earlier < later);
    }

    #[test]
    fn unknown_collection_kinds_pass_through() {
        let kind = CollectionKind::new("SomeFutureLibrary");
        assert!(!kind.is_known());
        assert_eq!(kind.as_str(), "SomeFutureLibrary");
        assert!(CollectionKind::from("tv_shows").is_known());
        assert!(!PersonKind::new("Lyricist").is_known());
    }
}
