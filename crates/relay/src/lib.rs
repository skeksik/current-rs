//! The loopback relay (ADR 0020): a small HTTP/1.1 server bound to 127.0.0.1 that forwards a player's GET and HEAD requests to the
//! server through an [`Upstream`] (in the app, the server's `ServerClient`, which adds the selected address's gateway credentials and
//! the session token) and streams the answers back. The player gets `http://127.0.0.1:<port>/<route>/...` and holds no credential.
//!
//! - A [`Relay`] listens only while a [`Route`] exists; each start gets a new port from the OS.
//! - A route is 128 random bits, base64url, as the first path segment. It maps to one upstream, the path prefixes of the item playing
//!   and a [`RelayEvents`] sink. Dropping it aborts its open connections.
//! - Refused before anything goes upstream: a wrong token or a path outside the prefixes (404), a `Host` other than
//!   `127.0.0.1:<port>`, an absolute-form target or a request body (400), any `Origin` (403), methods other than GET and HEAD (405).
//! - Only `current-net`'s `STREAM_REQUEST_HEADERS` go upstream and only its `STREAM_RESPONSE_HEADERS` come back; HLS playlists are rewritten so that every URI
//!   points back at the route, without `ApiKey`, or the playlist fails.
//! - It knows no service: whatever implements [`Upstream`] can be relayed.
//!
//! No Debug, Display or error of this crate shows a URL, a token or a credential.

mod body;
mod events;
mod path;
mod playlist;
mod relay;
mod serve;
mod token;
mod upstream;

pub use events::{RelayEvent, RelayEvents};
pub use hyper::body::Bytes;
pub use playlist::{MAX_PLAYLIST_BYTES, MAX_REWRITTEN_BYTES};
pub use relay::{FIRST_REQUEST_TIMEOUT, MAX_CONNECTIONS, Relay, Route, RouteConfig};
pub use upstream::{
    StreamMethod, Upstream, UpstreamBody, UpstreamFuture, UpstreamRequest, UpstreamResponse,
};

/// Why a route could not be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// No port could be opened on 127.0.0.1 (`PlaybackProblem::LocalRelay`, ADR 0020).
    #[error("the local stream connection could not be started")]
    Listen,
    /// The OS gave no random bytes for the route's token.
    #[error("the local stream connection could not be secured")]
    Random,
    /// A path prefix is not a clean absolute path ending in `/`: a bug in the caller.
    #[error("the stream's path is not one the local stream connection can serve")]
    InvalidPrefix,
}
