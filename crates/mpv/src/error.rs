//! Typed errors: mpv's error codes (`mpv_error` in `client.h`) and what can go wrong starting the engine.

/// One of mpv's error codes. The names are `client.h`'s (client API 2.5, mpv 0.41.0), kept here so no FFI call is needed to name one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("mpv error {code} ({})", self.name())]
pub struct MpvError {
    code: i32,
}

impl MpvError {
    pub const EVENT_QUEUE_FULL: MpvError = MpvError { code: -1 };
    pub const OPTION_NOT_FOUND: MpvError = MpvError { code: -5 };
    pub const PROPERTY_UNAVAILABLE: MpvError = MpvError { code: -10 };
    pub const LOADING_FAILED: MpvError = MpvError { code: -13 };
    pub const UNKNOWN_FORMAT: MpvError = MpvError { code: -17 };

    /// The error for a negative status; `None` for success (zero or positive).
    pub fn from_status(status: i32) -> Option<MpvError> {
        (status < 0).then_some(MpvError { code: status })
    }

    pub fn code(self) -> i32 {
        self.code
    }

    /// The name `client.h` gives the code, lowercased without the prefix (`MPV_ERROR_LOADING_FAILED` is `loading-failed`).
    pub fn name(self) -> &'static str {
        match self.code {
            -1 => "event-queue-full",
            -2 => "nomem",
            -3 => "uninitialized",
            -4 => "invalid-parameter",
            -5 => "option-not-found",
            -6 => "option-format",
            -7 => "option-error",
            -8 => "property-not-found",
            -9 => "property-format",
            -10 => "property-unavailable",
            -11 => "property-error",
            -12 => "command",
            -13 => "loading-failed",
            -14 => "ao-init-failed",
            -15 => "vo-init-failed",
            -16 => "nothing-to-play",
            -17 => "unknown-format",
            -18 => "unsupported",
            -19 => "not-implemented",
            -20 => "generic",
            _ => "unknown",
        }
    }
}

/// Starting or driving the engine failed.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("libmpv could not create a player")]
    Create,
    #[error("libmpv's client API is {major}.{minor}; the engine is written for 2.x")]
    ApiVersion { major: u32, minor: u32 },
    /// An option, property or command failed. `what` is its name, never a value.
    #[error("{what}: {error}")]
    Mpv { what: &'static str, error: MpvError },
    #[error("a text value contains a NUL byte: {what}")]
    Nul { what: &'static str },
    /// The file carries gateway credentials, but this FFmpeg cannot refuse redirects (`max_redirects` needs FFmpeg 9.0 or later), so
    /// its headers could follow a redirect to another host (ADR 0019, choice 4).
    #[error("this FFmpeg cannot refuse redirects; not sending gateway credentials")]
    RedirectsNotRefused,
    /// An HLS or DASH playlist with gateway credentials, not marked as checked for same-origin URIs: FFmpeg would send the headers to
    /// any host the playlist names, and the cookies to any whose name ends in the stream's (`FileRequest::playlist_checked_same_origin`).
    #[error("a playlist with gateway credentials must be checked for same-origin URIs first")]
    UncheckedPlaylist,
    /// A cookie or header has a character that would change its meaning (a control character, `;` in a cookie, `:` in a header name).
    /// `what` names the field, never its value.
    #[error("invalid gateway credential: {what}")]
    InvalidCredential { what: &'static str },
    /// Another engine still exists in this process (one per process, `engine` module doc).
    #[error("another player engine is still running")]
    Busy,
    #[error("could not start the event thread: {0}")]
    Thread(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_maps_to_errors_and_names() {
        assert_eq!(MpvError::from_status(0), None);
        assert_eq!(MpvError::from_status(3), None);
        let e = MpvError::from_status(-13).unwrap();
        assert_eq!(e, MpvError::LOADING_FAILED);
        assert_eq!(e.to_string(), "mpv error -13 (loading-failed)");
        assert_eq!(MpvError::from_status(-99).unwrap().name(), "unknown");
    }
}
