//! One typed error for the whole surface. UniFFI turns it into a C# exception class and a Swift `Error` enum.
//! (The real crates derive `Display` with thiserror; the spike avoids the extra dependency.)

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum BridgeError {
    /// The caller cancelled the operation through its `CancelToken`.
    Cancelled,
    InvalidInput {
        reason: String,
    },
    /// The core's runtime could not start or lost a task.
    Runtime {
        message: String,
    },
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::InvalidInput { reason } => write!(f, "invalid input: {reason}"),
            Self::Runtime { message } => write!(f, "runtime error: {message}"),
        }
    }
}

impl std::error::Error for BridgeError {}
