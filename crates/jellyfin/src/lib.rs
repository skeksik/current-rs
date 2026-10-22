//! The Jellyfin client. Server setup turns what a user types into a base URL, probes it for a Jellyfin server, tells servers apart
//! by their id and says whether plain HTTP needs the user's confirmation (ADR 0012). Discovery finds servers on the local network
//! (ADR 0016). Sign-in covers the authorization header, password and Quick Connect sign-in, the current user and sign out
//! (ADR 0015).
//!
//! Built on `current-net`'s [`ServerClient`](current_net::ServerClient). Wire types stay private; the public API speaks
//! `current-model`.

mod address;
mod auth;
mod client;
mod discovery;
mod probe;
mod wire;

pub use address::{AddressError, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, parse_address};
pub use auth::{
    AccessToken, CLIENT_NAME, ClientIdentity, QuickConnectSecret, authorization_header,
    device_name, platform_name,
};
pub use client::{
    AuthError, JellyfinClient, QUICK_CONNECT_INTERVAL, QUICK_CONNECT_LIFETIME, QuickConnectCode,
    RevokedHook, SignedIn,
};
pub use discovery::{
    Candidate, DISCOVERY_PORT, DiscoveryConfig, DiscoveryError, MAX_ADDRESSES_PER_SERVER,
    MAX_REPLY_BYTES, MAX_SERVERS, discover,
};
pub use probe::{FoundServer, JellyfinIdentity, MINIMUM_VERSION, ProbeError, Prober, same_server};
