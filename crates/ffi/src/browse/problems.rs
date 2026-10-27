//! Why a browse request or an image failed, as the UI acts on it: flat copies of the core's errors with no address, no path and no
//! server message (ADR 0006).

use current_jellyfin::AuthError;

use crate::Core;
use crate::problems::{GatewayProblem, NetProblem};

/// Why a browse request (a page, a row, a detail page, a played or favourite change) failed.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BrowseProblem {
    /// The session ended (a sign-out, a removal, a new sign-in): a new `Browse` is needed.
    NotSignedIn,
    /// The server revoked the token: sign in again.
    SessionRevoked,
    /// The server knows the user but refuses the request.
    Refused,
    /// The server's answer is being checked: try again in a moment.
    Unconfirmed,
    /// The session goes on, but the server refused this to the user: the item or its library is gone, or hidden from the user
    /// (ADR 0015 as amended 2026-10-25).
    NotAvailable,
    /// An authentication gateway in front of the server answered instead (ADR 0017): sign in to it again.
    Gateway {
        answer: GatewayProblem,
    },
    Network {
        problem: NetProblem,
    },
}

/// Why an image could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ImageProblem {
    /// The request failed as a browse request does.
    Request { problem: BrowseProblem },
    /// The server answered something that is not a JPEG or PNG image.
    NotAnImage,
    /// The request cannot be sent: an id or image type that does not fit in a URL path.
    Invalid,
    /// The BlurHash is not valid.
    InvalidBlurHash,
    /// The box has no area: a width or height of 0.
    EmptyBox,
    /// The fetch ended without an answer.
    Stopped,
}

impl From<&AuthError> for BrowseProblem {
    fn from(error: &AuthError) -> Self {
        current_app::BrowseProblem::from(error).into()
    }
}

// One to one, both ways: every match is exhaustive, so a variant added on one side and not the other fails to compile.

impl From<current_app::BrowseProblem> for BrowseProblem {
    fn from(problem: current_app::BrowseProblem) -> Self {
        use current_app::BrowseProblem as P;
        match problem {
            P::NotSignedIn => Self::NotSignedIn,
            P::SessionRevoked => Self::SessionRevoked,
            P::Refused => Self::Refused,
            P::Unconfirmed => Self::Unconfirmed,
            P::NotAvailable => Self::NotAvailable,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
            P::Network { problem } => Self::Network {
                problem: problem.into(),
            },
        }
    }
}

impl From<BrowseProblem> for current_app::BrowseProblem {
    fn from(problem: BrowseProblem) -> Self {
        use BrowseProblem as P;
        match problem {
            P::NotSignedIn => Self::NotSignedIn,
            P::SessionRevoked => Self::SessionRevoked,
            P::Refused => Self::Refused,
            P::Unconfirmed => Self::Unconfirmed,
            P::NotAvailable => Self::NotAvailable,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
            P::Network { problem } => Self::Network {
                problem: problem.into(),
            },
        }
    }
}

#[uniffi::export]
impl Core {
    /// Why a browse request failed, in plain words.
    pub fn browse_problem_text(&self, problem: BrowseProblem) -> String {
        current_app::browse_problem_text(&problem.into(), self.app.platform())
    }
}

impl From<&current_app::ImageError> for ImageProblem {
    fn from(error: &current_app::ImageError) -> Self {
        use current_jellyfin::ImageError as Fetch;
        match error {
            current_app::ImageError::Fetch(Fetch::Request(error)) => Self::Request {
                problem: error.into(),
            },
            current_app::ImageError::Fetch(Fetch::NotAnImage) => Self::NotAnImage,
            current_app::ImageError::Fetch(Fetch::Invalid) => Self::Invalid,
            current_app::ImageError::InvalidBlurHash => Self::InvalidBlurHash,
            current_app::ImageError::EmptyBox => Self::EmptyBox,
            current_app::ImageError::Stopped => Self::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_browse_problem_converts_both_ways() {
        for problem in [
            BrowseProblem::NotSignedIn,
            BrowseProblem::SessionRevoked,
            BrowseProblem::Refused,
            BrowseProblem::Unconfirmed,
            BrowseProblem::NotAvailable,
            BrowseProblem::Gateway {
                answer: GatewayProblem::SignIn,
            },
            BrowseProblem::Network {
                problem: NetProblem::Timeout,
            },
        ] {
            let back: BrowseProblem = current_app::BrowseProblem::from(problem.clone()).into();
            assert_eq!(back, problem);
        }
    }
}
