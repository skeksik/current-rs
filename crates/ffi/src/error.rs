//! The one error type every export returns (ADR 0004). It becomes `CoreException` with one nested class per variant in C#, and a
//! Swift `Error` enum.

use crate::browse::{BrowseProblem, ImageProblem};
use crate::player_records::PlayerProblem;
use crate::problems::{GatewayInputProblem, StoreProblem};

/// Why a call into the core failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    /// The call's `CancelToken` was cancelled before it finished.
    #[error("cancelled")]
    Cancelled,
    /// The core's runtime could not start or lost the task doing the work.
    #[error("runtime failure: {message}")]
    Runtime { message: String },
    /// The settings in the data folder could not be read or written.
    #[error("the settings could not be opened")]
    Store { problem: StoreProblem },
    /// The flow is not in the state the call needs, e.g. signing in before setup is `Ready`.
    #[error("the flow is not ready for this")]
    NotReady,
    /// No saved server matches.
    #[error("no such server is saved")]
    UnknownServer,
    /// The saved server has no address to reach it at.
    #[error("the server has no saved address")]
    NoAddress,
    /// The server's address cannot take requests. Setup never saves such an address, so this points to a bug.
    #[error("the server's address cannot be used")]
    InvalidAddress,
    /// The gateway credentials handed over were not used (ADR 0017): nothing was sent.
    #[error("the gateway credentials cannot be used")]
    Gateway { problem: GatewayInputProblem },
    /// The server is not a Jellyfin server that is signed in with a live session: browsing needs one (ADR 0018).
    #[error("not signed in to this server")]
    NotSignedIn,
    /// A browse request failed (ADR 0018).
    #[error("the request failed")]
    Browse { problem: BrowseProblem },
    /// An image could not be loaded (ADR 0018).
    #[error("the image could not be loaded")]
    Image { problem: ImageProblem },
    /// The player could not open or act (ADR 0019).
    #[error("the player could not do this")]
    Player { problem: PlayerProblem },
}
