//! Errors a request can end with, sorted by what the UI does about them (ADR 0006).
//!
//! No variant carries a URL, a host, a header or a library error's text: those can hold the server's address or a secret, and the
//! UI shows these errors and users paste them into bug reports.

use std::error::Error as _;

use reqwest::header::HeaderMap;

use crate::challenge::{AuthChallenge, challenges};

/// Longest server message kept, in characters.
const MESSAGE_LIMIT: usize = 300;

/// Debug output leaves out the server's message (see the `Debug` impl), so `tracing::debug!(?error)` stays safe to log.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
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
    /// 401 or 403: the user must sign in again or lacks the right, or a gateway in front of the server wants a sign-in (ADR 0017).
    /// `challenges` are the answer's `WWW-Authenticate` challenges (Debug shows their schemes only); `page` says whether the answer
    /// was an HTML page.
    #[error("the server refused access ({status})")]
    Auth {
        status: u16,
        message: Option<String>,
        challenges: Vec<AuthChallenge>,
        page: bool,
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

impl std::fmt::Debug for NetError {
    /// Like a derived Debug, with the server's message shown only as present or absent: it is for the UI, never for logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = |message: &Option<String>| {
            if message.is_some() {
                "[omitted]"
            } else {
                "None"
            }
        };
        match self {
            Self::Unreachable => f.write_str("Unreachable"),
            Self::Timeout => f.write_str("Timeout"),
            Self::Tls => f.write_str("Tls"),
            Self::Auth {
                status,
                message: text,
                challenges,
                page,
            } => f
                .debug_struct("Auth")
                .field("status", status)
                .field("message", &format_args!("{}", message(text)))
                .field("challenges", challenges)
                .field("page", page)
                .finish(),
            Self::Status {
                status,
                message: text,
            } => f
                .debug_struct("Status")
                .field("status", status)
                .field("message", &format_args!("{}", message(text)))
                .finish(),
            Self::Redirected { status, location } => f
                .debug_struct("Redirected")
                .field("status", status)
                .field("location", location)
                .finish(),
            Self::Decode { line, column } => f
                .debug_struct("Decode")
                .field("line", line)
                .field("column", column)
                .finish(),
            Self::InvalidRequest => f.write_str("InvalidRequest"),
        }
    }
}

impl NetError {
    /// The error for a non-success status, its headers and its body.
    pub(crate) fn from_status(status: u16, headers: &HeaderMap, body: &[u8]) -> Self {
        let message = server_message(body);
        match status {
            401 | 403 => Self::Auth {
                status,
                message,
                challenges: challenges(headers),
                page: is_html(headers, body),
            },
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

/// An address the server pointed to (a redirect's target, or where a response came from). Its Debug output hides it; read it with
/// [`Location::url`].
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

/// Whether an answer is an HTML page: its content type says so, or its body starts with `<`.
pub(crate) fn is_html(headers: &HeaderMap, body: &[u8]) -> bool {
    let declared = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let essence = value.split(';').next().unwrap_or("").trim();
            essence.eq_ignore_ascii_case("text/html")
                || essence.eq_ignore_ascii_case("application/xhtml+xml")
        });
    declared || body.trim_ascii_start().starts_with(b"<")
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
    use reqwest::header::{CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE};

    use super::*;
    use crate::challenge::AuthScheme;

    fn from_status(status: u16, body: &[u8]) -> NetError {
        NetError::from_status(status, &HeaderMap::new(), body)
    }

    #[test]
    fn auth_statuses_are_auth_errors() {
        assert_eq!(
            from_status(401, b""),
            NetError::Auth {
                status: 401,
                message: None,
                challenges: Vec::new(),
                page: false,
            }
        );
        assert!(matches!(
            from_status(403, b""),
            NetError::Auth { status: 403, .. }
        ));
        assert!(matches!(
            from_status(404, b""),
            NetError::Status { status: 404, .. }
        ));
    }

    #[test]
    fn auth_errors_carry_challenges_and_whether_they_were_a_page() {
        let mut headers = HeaderMap::new();
        headers.insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static(r#"Basic realm="Private Realm Text""#),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        let error = NetError::from_status(401, &headers, b"sign in");
        let NetError::Auth {
            challenges, page, ..
        } = &error
        else {
            panic!("{error:?}")
        };
        assert_eq!(challenges[0].scheme, AuthScheme::BASIC);
        assert_eq!(challenges[0].realm.as_deref(), Some("Private Realm Text"));
        assert!(*page);
        let text = format!("{error:?} {error}");
        assert!(!text.contains("Private Realm Text"), "{text}");
        assert_eq!(
            format!("{error:?}"),
            "Auth { status: 401, message: [omitted], challenges: [basic], page: true }"
        );
        assert!(matches!(
            from_status(403, b"  <!doctype html><p>no</p>"),
            NetError::Auth { page: true, .. }
        ));
    }

    #[test]
    fn server_messages_come_from_known_fields() {
        let message = |body: &[u8]| match from_status(500, body) {
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
    fn debug_leaves_out_the_server_message() {
        let error = from_status(500, br#"{"message":"Some Library Title is missing"}"#);
        assert_eq!(
            format!("{error:?}"),
            "Status { status: 500, message: [omitted] }"
        );
        assert_eq!(
            format!("{:?}", from_status(401, b"")),
            "Auth { status: 401, message: None, challenges: [], page: false }"
        );
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
