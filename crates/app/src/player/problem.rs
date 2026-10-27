//! Why playback stopped and why the player could not open or act, flat and with open names, as every UI gets them (ADR 0019,
//! "Errors"). No variant carries a URL or a server's text. The bridge copies these one to one; the player's words for them are
//! [`super::words::playback_problem_text`] and [`super::words::player_problem_text`].

use current_model::PlaybackProblem as ModelProblem;

use super::PlayerError;

/// Why a player could not open or act.
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl From<&PlayerError> for PlayerProblem {
    fn from(error: &PlayerError) -> Self {
        match error {
            PlayerError::EngineUnavailable => Self::EngineUnavailable,
            PlayerError::Busy => Self::Busy,
            PlayerError::EngineFailed { kind } => Self::EngineFailed {
                kind: kind.as_str().to_owned(),
            },
            PlayerError::NotSignedIn => Self::NotSignedIn,
            PlayerError::Replaced => Self::Replaced,
            // Calls through the bridge always run on the core's runtime.
            PlayerError::NoRuntime | PlayerError::Closed => Self::Closed,
        }
    }
}

/// Why playback cannot start or go on (ADR 0019, "Errors"). Flat, with no server text and no URL.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Current could not start its local stream connection (the loopback relay, ADR 0020).
    LocalRelay,
}

impl From<&ModelProblem> for PlaybackProblem {
    fn from(problem: &ModelProblem) -> Self {
        match problem {
            ModelProblem::NotPlayable { reasons } => Self::NotPlayable {
                reasons: reasons.iter().map(|r| r.as_str().to_owned()).collect(),
            },
            ModelProblem::Engine { kind } => Self::Engine {
                kind: kind.as_str().to_owned(),
            },
            ModelProblem::Network => Self::Network,
            ModelProblem::GatewaySignInNeeded => Self::GatewaySignInNeeded,
            ModelProblem::SignedOut => Self::SignedOut,
            ModelProblem::Server { status } => Self::Server { status: *status },
            ModelProblem::NotFound => Self::NotFound,
            ModelProblem::PlaylistElsewhere => Self::PlaylistElsewhere,
            ModelProblem::LocalRelay => Self::LocalRelay,
        }
    }
}

#[cfg(test)]
mod tests {
    use current_model::{EngineErrorKind, RouteReason};

    use super::*;

    #[test]
    fn a_player_error_becomes_its_problem() {
        let cases = [
            (
                PlayerError::EngineUnavailable,
                PlayerProblem::EngineUnavailable,
            ),
            (PlayerError::Busy, PlayerProblem::Busy),
            (
                PlayerError::EngineFailed {
                    kind: EngineErrorKind::LOADING_FAILED,
                },
                PlayerProblem::EngineFailed {
                    kind: "loading_failed".into(),
                },
            ),
            (PlayerError::NotSignedIn, PlayerProblem::NotSignedIn),
            (PlayerError::NoRuntime, PlayerProblem::Closed),
            (PlayerError::Replaced, PlayerProblem::Replaced),
            (PlayerError::Closed, PlayerProblem::Closed),
        ];
        for (error, problem) in cases {
            assert_eq!(PlayerProblem::from(&error), problem, "{error:?}");
        }
    }

    #[test]
    fn a_playback_problem_keeps_its_open_names() {
        let problem = ModelProblem::NotPlayable {
            reasons: vec![RouteReason::from("a_reason_no_release_knows")],
        };
        assert_eq!(
            PlaybackProblem::from(&problem),
            PlaybackProblem::NotPlayable {
                reasons: vec!["a_reason_no_release_knows".into()]
            }
        );
        let engine = ModelProblem::Engine {
            kind: EngineErrorKind::ENGINE_GONE,
        };
        assert_eq!(
            PlaybackProblem::from(&engine),
            PlaybackProblem::Engine {
                kind: "engine_gone".into()
            }
        );
        assert_eq!(
            PlaybackProblem::from(&ModelProblem::Server { status: 500 }),
            PlaybackProblem::Server { status: 500 }
        );
    }
}
