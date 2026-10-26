//! The seam between the relay and the server: [`Upstream`] sends one request to the server's selected address and gives back its
//! answer with a body read chunk by chunk. `current-net`'s `ServerClient::send_stream` implements it in the app (ADR 0020, "Where it
//! lives"); tests use a fake.

use std::future::Future;
use std::pin::Pin;

use current_net::header::{self, HeaderMap, HeaderName};
use current_net::{NetError, Url, redact_query};
use hyper::body::Bytes;

/// The request headers a player's request passes upstream; every other header is dropped, `Cookie` and `Authorization` included
/// (ADR 0020, "Requests and answers").
pub const REQUEST_HEADERS: [HeaderName; 5] = [
    header::RANGE,
    header::IF_RANGE,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
    header::ACCEPT,
];

/// The response headers the player gets back, beside the status. `Set-Cookie` is never among them.
pub const RESPONSE_HEADERS: [HeaderName; 7] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_RANGE,
    header::ACCEPT_RANGES,
    header::ETAG,
    header::LAST_MODIFIED,
    header::CACHE_CONTROL,
];

/// The two methods the relay forwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMethod {
    Get,
    Head,
}

/// A request for the server, built by the relay from a player's request that passed every check. Debug shows header names only and
/// the query as the request log shows it.
#[derive(Clone)]
pub struct UpstreamRequest {
    pub method: StreamMethod,
    /// The path relative to the server's address, starting with `/` and inside the route's prefixes (`/Videos/{id}/...`), still
    /// percent-encoded as the player sent it. Never a host: the upstream URL is the selected address plus this path and the query.
    pub path: String,
    /// The query pairs, decoded, in order, without `ApiKey` or `api_key`: the server's token is added by the upstream's own
    /// credentials (ADR 0020 choice 4).
    pub query: Vec<(String, String)>,
    /// Only [`REQUEST_HEADERS`].
    pub headers: HeaderMap,
}

impl std::fmt::Debug for UpstreamRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &redact_query(&self.query))
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The server's answer. Debug shows the status and header names only.
pub struct UpstreamResponse {
    pub status: u16,
    /// Every header the server sent; the relay passes on only [`RESPONSE_HEADERS`].
    pub headers: HeaderMap,
    /// The base URL of the address the request went to, without credentials: playlist URIs are resolved against it plus the
    /// request's path. It may differ between requests of one route when the server roams to another address (ADR 0020 choice 6).
    pub base: Url,
    pub body: Box<dyn UpstreamBody>,
}

impl std::fmt::Debug for UpstreamResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamResponse")
            .field("status", &self.status)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A future the relay can hold across polls.
pub type UpstreamFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, NetError>> + Send + 'a>>;

/// A response body, read one chunk at a time. The relay asks for the next chunk only when the player's socket can take more, so a
/// player that stops reading stops the reads upstream (ADR 0020, "Streaming"). Dropping the body ends the upstream request.
pub trait UpstreamBody: Send {
    /// The next chunk, or `None` at the end of the body. An error ends the body: the player's connection is closed.
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>>;
}

/// Sends the relay's requests to one server.
///
/// The contract, as for `ServerClient::send` (current-net): the request goes to the address selected now, with that address's
/// gateway credentials and the service's token, and no redirect is followed. `Ok` for a 2xx or 304 answer; for any other status an
/// error typed like `send`'s: [`NetError::Auth`] for 401 and 403 (with the challenges and whether the answer was a page),
/// [`NetError::Redirected`] for a 3xx, [`NetError::Status`] otherwise; and [`NetError::Unreachable`], [`NetError::Timeout`] or
/// [`NetError::Tls`] when the network failed. The relay also treats an `Ok` with another status as the error it should have been.
pub trait Upstream: Send + Sync {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse>;
}
