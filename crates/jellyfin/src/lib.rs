//! The Jellyfin client. So far: server setup, which turns what a user types into a base URL, probes it for a Jellyfin server, tells
//! servers apart by their id and says whether plain HTTP needs the user's confirmation (ADR 0012).
//!
//! Built on `current-net`'s [`ServerClient`](current_net::ServerClient). Wire types stay private; the public API speaks
//! `current-model`.

mod address;
mod probe;
mod wire;

pub use address::{AddressError, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, parse_address};
pub use probe::{FoundServer, MINIMUM_VERSION, ProbeError, Prober, same_server};
