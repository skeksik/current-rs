//! Errors of the settings store.
//!
//! No variant carries a path or an OS error's text: the data folder's path holds the user's account name on every desktop platform,
//! and these errors reach the UI and bug reports. The kind of I/O error is enough to tell the user what to do.

use std::io;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// Reading or writing the data folder failed. `op` says which step.
    #[error("could not {op} the settings ({kind})")]
    Io {
        op: &'static str,
        kind: io::ErrorKind,
    },
    /// The settings file was written by a newer version of Current. It is kept as it is; this version does not save over it.
    #[error(
        "the settings were saved by a newer version of Current (format {found}); they are not changed"
    )]
    NewerVersion { found: u64 },
    /// The operating system's random number generator failed.
    #[error("the system random number generator failed")]
    Random,
}

impl StoreError {
    pub(crate) fn io(op: &'static str) -> impl FnOnce(io::Error) -> Self {
        move |error| Self::Io {
            op,
            kind: error.kind(),
        }
    }
}
