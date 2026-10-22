//! The Jellyfin client. So far: server setup, which turns what a user types into a base URL, probes it for a Jellyfin server, tells
//! servers apart by their id and says whether plain HTTP needs the user's confirmation (ADR 0012), and finds servers on the local
//! network (ADR 0016).
//!
//! Built on `current-net`'s [`ServerClient`](current_net::ServerClient). Wire types stay private; the public API speaks
//! `current-model`.

mod address;
mod discovery;
mod probe;
mod wire;

pub use address::{AddressError, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, parse_address};
pub use discovery::{
    Candidate, DISCOVERY_PORT, DiscoveryConfig, DiscoveryError, MAX_ADDRESSES_PER_SERVER,
    MAX_REPLY_BYTES, MAX_SERVERS, discover,
};
pub use probe::{FoundServer, JellyfinIdentity, MINIMUM_VERSION, ProbeError, Prober, same_server};
