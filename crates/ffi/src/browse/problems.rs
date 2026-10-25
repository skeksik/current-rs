//! Why a browse request or an image failed, as the UI acts on it: flat copies of the core's errors with no address, no path and no
//! server message (ADR 0006).

use current_jellyfin::AuthError;

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
        match error {
            AuthError::NotSignedIn => Self::NotSignedIn,
            AuthError::SessionRevoked => Self::SessionRevoked,
            // The others are answers to a sign-in, which a browse request never gets: a refusal is the closest.
            AuthError::Refused
            | AuthError::WrongCredentials
            | AuthError::QuickConnectDisabled
            | AuthError::QuickConnectExpired
            | AuthError::EmbyHeaderIgnored => Self::Refused,
            AuthError::Unconfirmed => Self::Unconfirmed,
            AuthError::Gateway(answer) => Self::Gateway {
                answer: answer.into(),
            },
            AuthError::Network(error) => Self::Network {
                problem: error.into(),
            },
        }
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
