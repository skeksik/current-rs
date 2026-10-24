//! What a server says about itself before sign-in, and the addresses it is reached at.

use crate::identity::ServerId;
use crate::open::open_value;

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
    /// The address's own id within its server, given when the address is saved (`None` until then). Its gateway secrets are
    /// stored under it (ADR 0017), so it stays when the URL changes or the addresses are reordered.
    pub id: Option<AddressId>,
    /// The kinds of gateway credential this address has in the keyring (ADR 0017), never a value. Empty: none.
    pub gateway: Vec<GatewayKind>,
}

impl ServerAddress {
    /// An address not saved yet: no id and no gateway credential.
    pub fn new(url: impl Into<String>, plain_http_confirmed: bool) -> Self {
        Self {
            url: url.into(),
            plain_http_confirmed,
            id: None,
            gateway: Vec::new(),
        }
    }
}

/// The id of a saved address within its server. Given out by [`SavedServer::add_address`](crate::SavedServer::add_address) and
/// never given to another address of the same server, so a secret left behind by a removed address never reaches a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AddressId(u64);

impl AddressId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for AddressId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

open_value! {
    /// A kind of gateway credential an address has (ADR 0017). Open: a kind a newer version stored passes through unchanged.
    GatewayKind {
        /// An HTTP Basic user name and password.
        BASIC = "basic",
        /// Cookies from a web sign-in.
        COOKIES = "cookies",
        /// Extra headers, such as a service token.
        HEADERS = "headers",
    }
}
