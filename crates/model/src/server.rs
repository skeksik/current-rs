//! What a server says about itself before sign-in, and the addresses it is reached at.

use crate::identity::ServerId;

/// What a server tells anyone who asks, without signing in. Values are as the server sent them: the model parses none of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicServerInfo {
    /// The server's own id: the same on every address (see [`ServerId`]).
    pub id: ServerId,
    /// The name its administrator gave it. Shown to the user; it can change at any time, so it never identifies the server.
    pub name: String,
    /// The server's version as it reports it, e.g. `12.2.0`.
    pub version: String,
    /// The server product's name as it reports it, e.g. `Jellyfin Server`. Open: forks may report another name.
    pub product: String,
    /// The address the server believes it is reached at, if it said. A hint only: never used without probing it.
    pub local_address: Option<String>,
    /// Whether the server's administrator finished its first-run setup.
    pub setup_completed: bool,
}

/// One address a server is reached at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerAddress {
    /// The server's base URL, including any path prefix it is served under (no trailing slash).
    pub url: String,
    /// Whether the user agreed to use this address over plain HTTP where that needs their confirmation (ADR 0012). Kept with the
    /// address so that they are asked once, not on every connection.
    pub plain_http_confirmed: bool,
}
