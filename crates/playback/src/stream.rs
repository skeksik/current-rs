//! What the engine needs to open a stream: its URL and the address's gateway credentials (ADR 0019, "Gateways, tokens and the
//! player"). Both hold secrets: neither has a `Display`, their `Debug` shows no URL and no value, and they never cross `current-ffi`.

use std::fmt;

/// The URL the engine plays, with the session token in it as `ApiKey` (ADR 0019, choice 3). Opaque like `current-jellyfin`'s
/// `StreamUrl`, which the composition root converts into this without formatting it.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamUrl(String);

impl StreamUrl {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// The URL with its token. Only for the engine handoff (the engine passing it to libmpv): never logged, shown, stored or
    /// passed across `current-ffi`.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StreamUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamUrl(<redacted>)")
    }
}

/// A gateway cookie of the address the stream is on (ADR 0017). The engine scopes it to the stream's host (for libmpv, FFmpeg's
/// `cookies` option per file, ADR 0019 choice 5).
#[derive(Clone, PartialEq, Eq)]
pub struct StreamCookie {
    pub name: String,
    pub value: String,
    pub path: String,
    /// Sent only over https.
    pub secure: bool,
}

impl fmt::Debug for StreamCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("path", &self.path)
            .field("secure", &self.secure)
            .finish()
    }
}

/// The gateway credentials a stream needs: cookies, Basic and extra headers, and the secret values its URL holds (the session token)
/// so the engine can redact them from its log. Empty for an address without a gateway.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StreamAuth {
    pub cookies: Vec<StreamCookie>,
    /// Header name and value (for libmpv, FFmpeg's `headers` option per file).
    pub headers: Vec<(String, String)>,
    /// Values to redact from engine log lines; the engine adds the cookie and header values itself.
    pub secrets: Vec<String>,
}

impl StreamAuth {
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty() && self.headers.is_empty() && self.secrets.is_empty()
    }
}

impl fmt::Debug for StreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamAuth")
            .field("cookies", &self.cookies)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_no_secret() {
        let url = StreamUrl::new("https://media.example.com/Videos/1/stream?ApiKey=secret-token");
        let auth = StreamAuth {
            cookies: vec![StreamCookie {
                name: "gateway".into(),
                value: "cookie-value".into(),
                path: "/".into(),
                secure: true,
            }],
            headers: vec![("Authorization".into(), "Basic header-value".into())],
            secrets: vec!["secret-token".into()],
        };
        let shown = format!("{url:?} {auth:?} {url:#?} {auth:#?}");
        for secret in ["secret-token", "cookie-value", "header-value", "media.example.com"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert!(shown.contains("Authorization"));
        assert_eq!(
            url.expose(),
            "https://media.example.com/Videos/1/stream?ApiKey=secret-token"
        );
        assert!(StreamAuth::default().is_empty());
        assert!(!auth.is_empty());
    }
}
