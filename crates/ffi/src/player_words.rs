//! The player's words across the bridge (ADR 0019): its times, shown and spoken, the route and its reasons, the plan's notes, track
//! names, the skip button, the next-episode card, the track menus' fixed items and its problems, all worded by `current-app`
//! (`player::words`), so every UI shows the same. The playback info panel's lines are [`crate::Player::playback_info_lines`]; this
//! holds the sizes the platform measures for them ([`PanelSizes`]).
//!
//! Times are in milliseconds, signed: a remaining time can come out below 0, which reads as 0.

use current_app::playback::PlanNote;
use current_model::RouteLabel;

use crate::Core;
use crate::player_records::{
    PlanNoteInfo, PlaybackProblem, PlayerProblem, RouteKind, RouteLabelKind, TrackInfo,
};

/// What the platform measures for the info panel's last line: the swap chain panel's size in physical pixels, the view's and the
/// content's sizes in physical pixels (device-independent size × the window's scale, so fractional), and the window's client size
/// in pixels. The content's and the window's are shown only when both their numbers are given.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PanelSizes {
    pub composition_width: u32,
    pub composition_height: u32,
    pub view_width: f64,
    pub view_height: f64,
    pub content_width: Option<f64>,
    pub content_height: Option<f64>,
    pub window_width: Option<i32>,
    pub window_height: Option<i32>,
}

impl From<&PanelSizes> for current_app::PanelSizes {
    fn from(panel: &PanelSizes) -> Self {
        Self {
            composition_width: panel.composition_width,
            composition_height: panel.composition_height,
            view_width: panel.view_width,
            view_height: panel.view_height,
            content: panel.content_width.zip(panel.content_height),
            window: panel.window_width.zip(panel.window_height),
        }
    }
}

impl From<&PlanNoteInfo> for PlanNote {
    fn from(note: &PlanNoteInfo) -> Self {
        match note {
            PlanNoteInfo::RemoteBitrateLimit { bits_per_second } => Self::RemoteBitrateLimit {
                limit: *bits_per_second,
            },
            PlanNoteInfo::TranscodingNotAllowed => Self::TranscodingNotAllowed,
            PlanNoteInfo::RemuxingNotAllowed => Self::RemuxingNotAllowed,
            PlanNoteInfo::RemoteSourceTranscodingForced => Self::RemoteSourceTranscodingForced,
        }
    }
}

#[uniffi::export]
impl Core {
    /// A time as the player shows it: "1:02:03" from an hour on, else "2:03". The hours are the whole hours in total: 25 h 0 min
    /// 30 s is "25:00:30".
    pub fn time_label(&self, ms: i64) -> String {
        current_app::time_label(ms)
    }

    /// A time as a screen reader should say it: "1 hour 2 minutes 3 seconds", "0 seconds".
    pub fn spoken_time(&self, ms: i64) -> String {
        current_app::spoken_time(ms)
    }

    /// The elapsed time's spoken name: "Time elapsed 12 minutes 3 seconds".
    pub fn elapsed_label(&self, ms: i64) -> String {
        current_app::elapsed_label(ms)
    }

    /// The remaining time's spoken name: "Time remaining 12 minutes 3 seconds".
    pub fn remaining_label(&self, ms: i64) -> String {
        current_app::remaining_label(ms)
    }

    /// The seek bar's spoken status: "1 minute of 1 hour 30 minutes", or the position alone without a duration.
    pub fn position_label(&self, ms: i64, duration_ms: Option<i64>) -> String {
        current_app::position_label(ms, duration_ms)
    }

    /// The route's label: "Direct play", "Remux", "Direct stream", "Transcode" (one engine; W3 brings ADR 0025's labels).
    pub fn route_name(&self, route: RouteKind) -> String {
        let label = match route {
            RouteKind::DirectPlay => RouteLabel::AppEngine,
            RouteKind::Remux => RouteLabel::ServerRemux,
            RouteKind::DirectStream => RouteLabel::ServerDirectStream,
            RouteKind::Transcode => RouteLabel::ServerTranscode,
        };
        current_app::route_name_one_engine(label).to_owned()
    }

    /// ADR 0025's route label for a player with a platform engine: "Native", "Remux on this device", "App engine", "Server remux",
    /// "Server direct stream", "Server transcode".
    pub fn route_label_name(&self, label: RouteLabelKind) -> String {
        current_app::route_name(label.into()).to_owned()
    }

    /// "How to play"'s choice as Settings and the player's menu name it: "Auto", "Native", "Remux on this device", "App engine",
    /// "Server remux", "Server transcode".
    pub fn how_to_play_name(&self, how: crate::HowToPlayKind) -> String {
        current_app::how_to_play_name(how.into()).to_owned()
    }

    /// Why the server streams rather than plays the file directly, in plain words; a reason no release knows is its own name.
    pub fn reason_text(&self, reason: String) -> String {
        current_app::reason_text(&reason).to_owned()
    }

    /// What the server's permissions say about the plan, its bitrate with the UI's decimal separator.
    pub fn plan_note_text(&self, note: PlanNoteInfo) -> String {
        current_app::plan_note_text(&(&note).into(), &self.app.number_format())
    }

    /// A track as the server names it, else by its language and codec ("eng · AC3"), else its number ("Track 3").
    pub fn track_name(&self, track: TrackInfo) -> String {
        current_app::track_name(
            track.label.as_deref(),
            track.language.as_deref(),
            track.codec.as_deref(),
            track.index,
        )
    }

    /// The skip button for a segment kind: "Skip intro", "Skip credits", ...; "Skip" for a kind no release knows.
    pub fn skip_label(&self, kind: String) -> String {
        current_app::skip_label(&kind).to_owned()
    }

    /// The next-episode card's line from its number label and name ([`crate::NextEpisodeInfo`]'s): "S1:E3 Name", or the name alone.
    pub fn episode_line(&self, number: String, name: String) -> String {
        current_app::episode_line(&number, &name)
    }

    /// The next-episode card's countdown: "Starts in 5 seconds".
    pub fn countdown_text(&self, seconds: u32) -> String {
        current_app::countdown_text(seconds)
    }

    /// The audio menu's only item when the item has no audio track: "No audio tracks".
    pub fn no_audio_tracks_label(&self) -> String {
        current_app::NO_AUDIO_TRACKS.to_owned()
    }

    /// The subtitles menu's first item: "Off".
    pub fn subtitles_off_label(&self) -> String {
        current_app::SUBTITLES_OFF.to_owned()
    }

    /// Why playback stopped, in plain words.
    pub fn playback_problem_text(&self, problem: PlaybackProblem) -> String {
        current_app::playback_problem_text(&(&problem).into())
    }

    /// Why the player couldn't open or act, in plain words.
    pub fn player_problem_text(&self, problem: PlayerProblem) -> String {
        current_app::player_problem_text(&(&problem).into()).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant of the bridge's copies, through `current-app`'s and back; the exhaustive matches stop the copies drifting: a
    /// variant added on one side only fails to compile here.
    #[test]
    fn the_problems_convert_one_to_one_both_ways() {
        let player = [
            PlayerProblem::EngineUnavailable,
            PlayerProblem::Busy,
            PlayerProblem::EngineFailed {
                kind: "loading_failed".into(),
            },
            PlayerProblem::NotSignedIn,
            PlayerProblem::Replaced,
            PlayerProblem::Closed,
        ];
        for problem in &player {
            // Listed above: one per variant.
            match problem {
                PlayerProblem::EngineUnavailable
                | PlayerProblem::Busy
                | PlayerProblem::EngineFailed { .. }
                | PlayerProblem::NotSignedIn
                | PlayerProblem::Replaced
                | PlayerProblem::Closed => {}
            }
            let core = current_app::PlayerProblem::from(problem);
            match (&core, problem) {
                (
                    current_app::PlayerProblem::EngineUnavailable,
                    PlayerProblem::EngineUnavailable,
                )
                | (current_app::PlayerProblem::Busy, PlayerProblem::Busy)
                | (current_app::PlayerProblem::NotSignedIn, PlayerProblem::NotSignedIn)
                | (current_app::PlayerProblem::Replaced, PlayerProblem::Replaced)
                | (current_app::PlayerProblem::Closed, PlayerProblem::Closed) => {}
                (
                    current_app::PlayerProblem::EngineFailed { kind: a },
                    PlayerProblem::EngineFailed { kind: b },
                ) => assert_eq!(a, b),
                (core, problem) => panic!("{problem:?} became {core:?}"),
            }
            assert_eq!(&PlayerProblem::from(&core), problem);
        }
        let playback = [
            PlaybackProblem::NotPlayable {
                reasons: vec!["a_reason_no_release_knows".into()],
            },
            PlaybackProblem::Engine {
                kind: "engine_gone".into(),
            },
            PlaybackProblem::Network,
            PlaybackProblem::GatewaySignInNeeded,
            PlaybackProblem::SignedOut,
            PlaybackProblem::Server { status: 503 },
            PlaybackProblem::NotFound,
            PlaybackProblem::PlaylistElsewhere,
            PlaybackProblem::LocalRelay,
        ];
        for problem in &playback {
            // Listed above: one per variant.
            match problem {
                PlaybackProblem::NotPlayable { .. }
                | PlaybackProblem::Engine { .. }
                | PlaybackProblem::Network
                | PlaybackProblem::GatewaySignInNeeded
                | PlaybackProblem::SignedOut
                | PlaybackProblem::Server { .. }
                | PlaybackProblem::NotFound
                | PlaybackProblem::PlaylistElsewhere
                | PlaybackProblem::LocalRelay => {}
            }
            let core = current_app::PlaybackProblem::from(problem);
            use current_app::PlaybackProblem as C;
            match (&core, problem) {
                (C::NotPlayable { reasons: a }, PlaybackProblem::NotPlayable { reasons: b }) => {
                    assert_eq!(a, b);
                }
                (C::Engine { kind: a }, PlaybackProblem::Engine { kind: b }) => assert_eq!(a, b),
                (C::Server { status: a }, PlaybackProblem::Server { status: b }) => {
                    assert_eq!(a, b);
                }
                (C::Network, PlaybackProblem::Network)
                | (C::GatewaySignInNeeded, PlaybackProblem::GatewaySignInNeeded)
                | (C::SignedOut, PlaybackProblem::SignedOut)
                | (C::NotFound, PlaybackProblem::NotFound)
                | (C::PlaylistElsewhere, PlaybackProblem::PlaylistElsewhere)
                | (C::LocalRelay, PlaybackProblem::LocalRelay) => {}
                (core, problem) => panic!("{problem:?} became {core:?}"),
            }
            assert_eq!(&PlaybackProblem::from(&core), problem);
        }
    }

    #[test]
    fn the_words_are_the_cores() {
        let core = Core::new().unwrap();
        core.set_decimal_separator(",".into());
        assert_eq!(core.time_label(3_723_000), "1:02:03");
        assert_eq!(core.time_label(-1), "0:00");
        assert_eq!(core.time_label(25 * 3_600_000 + 30_000), "25:00:30");
        assert_eq!(core.spoken_time(3_723_000), "1 hour 2 minutes 3 seconds");
        assert_eq!(core.elapsed_label(1_000), "Time elapsed 1 second");
        assert_eq!(core.remaining_label(-500), "Time remaining 0 seconds");
        assert_eq!(core.position_label(60_000, None), "1 minute");
        assert_eq!(core.route_name(RouteKind::DirectStream), "Direct stream");
        assert_eq!(
            core.reason_text("quality_limit".into()),
            "The quality you chose is below the file's."
        );
        assert_eq!(
            core.plan_note_text(PlanNoteInfo::RemoteBitrateLimit {
                bits_per_second: 1_500_000
            }),
            "The server limits streaming outside your network to 1,5 Mbit/s."
        );
        assert_eq!(core.skip_label("outro".into()), "Skip credits");
        assert_eq!(
            core.episode_line("S1:E3".into(), "Name".into()),
            "S1:E3 Name"
        );
        assert_eq!(core.countdown_text(1), "Starts in 1 second");
        assert_eq!(core.no_audio_tracks_label(), "No audio tracks");
        assert_eq!(core.subtitles_off_label(), "Off");
        assert_eq!(
            core.playback_problem_text(PlaybackProblem::Server { status: 500 }),
            "The server couldn't play this item (error 500)."
        );
        assert_eq!(
            core.player_problem_text(PlayerProblem::Busy),
            "Another video is still closing. Try again in a moment."
        );
    }

    #[test]
    fn the_panel_sizes_show_a_pair_only_when_both_are_given() {
        let sizes = PanelSizes {
            composition_width: 1,
            composition_height: 2,
            view_width: 3.0,
            view_height: 4.0,
            content_width: Some(5.0),
            content_height: None,
            window_width: Some(6),
            window_height: Some(7),
        };
        let core: current_app::PanelSizes = (&sizes).into();
        assert_eq!(core.content, None);
        assert_eq!(core.window, Some((6, 7)));
    }
}
