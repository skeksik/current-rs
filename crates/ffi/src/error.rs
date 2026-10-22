//! The one error type every export returns (ADR 0004). It becomes `CoreException` with one nested class per variant in C#, and a
//! Swift `Error` enum.

/// Why a call into the core failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    /// The call's `CancelToken` was cancelled before it finished.
    #[error("cancelled")]
    Cancelled,
    /// The core's runtime could not start or lost the task doing the work.
    #[error("runtime failure: {message}")]
    Runtime { message: String },
}
