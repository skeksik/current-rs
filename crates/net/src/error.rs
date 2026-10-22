//! Errors a request can end with, sorted by what the UI does about them (ADR 0006).
//!
//! No variant carries a URL, a host, a header or a library error's text: those can hold the server's address or a secret, and the
//! UI shows these errors and users paste them into bug reports.

use std::error::Error as _;

/// Longest server message kept, in characters.
const MESSAGE_LIMIT: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    /// No connection: the device is offline, the name did not resolve, the server refused or dropped the connection.
    #[error("the server could not be reached")]
    Unreachable,
    /// The server did not answer in time.
    #[error("the server did not answer in time")]
    Timeout,
    /// The secure connection failed: an untrusted or mismatched certificate, or a failed handshake.
    #[error("the secure connection to the server failed")]
    Tls,
    /// 401 or 403: the user must sign in again or lacks the right.
    #[error("the server refused access ({status})")]
    Auth {
        status: u16,
        message: Option<String>,
    },
    /// Any other non-success status, with the server's own message when it sent one.
    #[error("the server answered with status {status}")]
    Status {
        status: u16,
        message: Option<String>,
    },
    /// A redirect that was not followed: to another host or port, from https to http, or too many in a row. `location` is where the
    /// server points, for server setup to show; it is redacted from Debug and never part of Display.
    #[error("the server redirected elsewhere ({status})")]
    Redirected {
        status: u16,
        location: Option<Location>,
    },
    /// The response did not have the expected shape. Only the position is kept: a decoder's message can quote the response.
    #[error("the server's response could not be read (line {line}, column {column})")]
    Decode { line: usize, column: usize },
    /// The request could not be built: a path that does not fit the server's address, or a body that would not encode. A bug in the
    /// calling crate, not a server problem.
    #[error("the request could not be built")]
    InvalidRequest,
}

impl NetError {
    /// The error for a non-success status and its body.
    pub(crate) fn from_status(status: u16, body: &[u8]) -> Self {
        let message = server_message(body);
        match status {
            401 | 403 => Self::Auth { status, message },
            _ => Self::Status { status, message },
        }
    }

    pub(crate) fn from_decode(error: &serde_json::Error) -> Self {
        Self::Decode {
            line: error.line(),
            column: error.column(),
        }
    }

    /// Sorts a transport error. Only the error's kind is used, never its text.
    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        // Drop the URL first so it cannot leak through a later change to this function.
        let error = error.without_url();
        if is_tls(&error) {
            Self::Tls
        } else if error.is_timeout() {
            Self::Timeout
        } else if error.is_builder() {
            Self::InvalidRequest
        } else {
            // Connect, DNS, a connection lost mid-request or mid-body.
            Self::Unreachable
        }
    }

    /// A short name for the request log.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Auth { .. } => "auth",
            Self::Status { .. } => "status",
            Self::Redirected { .. } => "redirected",
            Self::Decode { .. } => "decode",
            Self::InvalidRequest => "invalid request",
        }
    }
}

/// Where a redirect pointed. Holds an address, so its Debug output hides it; read it with [`Location::url`].
#[derive(Clone, PartialEq, Eq)]
pub struct Location(pub(crate) url::Url);

impl Location {
    pub fn url(&self) -> &url::Url {
        &self.0
    }
}

impl std::fmt::Debug for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Location([redacted])")
    }
}

/// Whether a rustls error is somewhere in the error's sources. hyper-rustls wraps it in `io::Error`s, whose `source()` skips the
/// wrapped error itself, so those are opened with `get_ref()`.
fn is_tls(error: &reqwest::Error) -> bool {
    let mut source = error.source();
    while let Some(current) = source {
        let mut inner: &(dyn std::error::Error + 'static) = current;
        loop {
            if inner.is::<rustls::Error>() {
                return true;
            }
            match inner
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
            {
                Some(wrapped) => inner = wrapped,
                None => break,
            }
        }
        source = current.source();
    }
    false
}

/// The server's own explanation from an error body: a `message`, `detail`, `title` or `error` string field of a JSON object, or a
/// short plain-text body. Shown to the user; never written to the request log.
fn server_message(body: &[u8]) -> Option<String> {
    let text = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(fields)) => ["message", "detail", "title", "error"]
            .iter()
            .find_map(|key| {
                fields
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })?,
        Ok(serde_json::Value::String(text)) => text,
        Ok(_) => return None,
        Err(_) => {
            let text = std::str::from_utf8(body).ok()?;
            // An HTML error page is not a message.
            if text.trim_start().starts_with('<') {
                return None;
            }
            text.to_owned()
        }
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(MESSAGE_LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_statuses_are_auth_errors() {
        assert_eq!(
            NetError::from_status(401, b""),
            NetError::Auth {
                status: 401,
                message: None
            }
        );
        assert!(matches!(
            NetError::from_status(403, b""),
            NetError::Auth { status: 403, .. }
        ));
        assert!(matches!(
            NetError::from_status(404, b""),
            NetError::Status { status: 404, .. }
        ));
    }

    #[test]
    fn server_messages_come_from_known_fields() {
        let message = |body: &[u8]| match NetError::from_status(500, body) {
            NetError::Status { message, .. } => message,
            _ => None,
        };
        assert_eq!(
            message(br#"{"message":"Request failed"}"#).as_deref(),
            Some("Request failed")
        );
        assert_eq!(
            message(br#"{"title":"Not Found","detail":"No such item"}"#).as_deref(),
            Some("No such item")
        );
        assert_eq!(
            message(b"  Plain text error \n").as_deref(),
            Some("Plain text error")
        );
        assert_eq!(message(br#"{"other":1}"#), None);
        assert_eq!(message(b"<html><body>Bad Gateway</body></html>"), None);
        assert_eq!(message(b""), None);
        assert_eq!(message(&[0xff, 0xfe]), None);
    }

    #[test]
    fn long_messages_are_cut() {
        let long = "é".repeat(MESSAGE_LIMIT + 50);
        let Some(message) = server_message(long.as_bytes()) else {
            panic!("no message")
        };
        assert_eq!(message.chars().count(), MESSAGE_LIMIT + 1);
        assert!(message.ends_with('…'));
    }

    #[test]
    fn decode_errors_keep_only_the_position() {
        let Err(error) = serde_json::from_str::<u32>(r#""Some Library Title""#) else {
            panic!("decoded")
        };
        let error = NetError::from_decode(&error);
        assert!(matches!(error, NetError::Decode { line: 1, .. }));
        assert!(!format!("{error} {error:?}").contains("Title"));
    }
}
