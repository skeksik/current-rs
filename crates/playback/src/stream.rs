//! How the engine reaches a stream: its URL and the secrets in it (ADR 0019, "Gateways, tokens and the player"; ADR 0020). Neither
//! has a `Display`, their `Debug` shows no URL and no value, and they never cross `current-ffi`.

use std::fmt;

/// A URL of a stream. From the server's answer, a URL on the server's address with the session token in it as `ApiKey` (ADR 0019,
/// choice 3); for the engine, that URL or one on the loopback relay that holds the route's token instead (ADR 0020). Opaque like
/// `current-jellyfin`'s `StreamUrl`, which the composition root converts into this without formatting it.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamUrl(String);

impl StreamUrl {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// The URL with its token. Only for the engine handoff (the engine passing it to libmpv) and for the composition root making the
    /// engine's URL from the server's: never logged, shown, stored or passed across `current-ffi`.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StreamUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamUrl(<redacted>)")
    }
}

/// The secret values a stream's URL holds (the session token of a direct URL, the route token of a relayed one), so the engine can
/// redact them from its log. The engine gets no gateway credential: a stream on an address with a gateway goes through the loopback
/// relay, which adds them upstream (ADR 0020).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StreamAuth {
    pub secrets: Vec<String>,
}

impl StreamAuth {
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }
}

impl fmt::Debug for StreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamAuth")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// What [`MediaServer::stream_url`](crate::MediaServer::stream_url) gives the engine for a plan's stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineStream {
    pub url: StreamUrl,
    pub auth: StreamAuth,
    /// The URL is on the loopback relay (ADR 0020), not on the server's address.
    pub relayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_no_secret() {
        let url = StreamUrl::new("https://media.example.com/Videos/1/stream?ApiKey=secret-token");
        let auth = StreamAuth {
            secrets: vec!["secret-token".into()],
        };
        let stream = EngineStream {
            url: url.clone(),
            auth: auth.clone(),
            relayed: false,
        };
        let shown = format!("{url:?} {auth:?} {url:#?} {auth:#?} {stream:?}");
        for secret in ["secret-token", "media.example.com"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert_eq!(
            url.expose(),
            "https://media.example.com/Videos/1/stream?ApiKey=secret-token"
        );
        assert!(StreamAuth::default().is_empty());
        assert!(!auth.is_empty());
    }
}
