//! The seam between the relay and the server: [`Upstream`] sends one request to the server's selected address and gives back its
//! answer with a body read chunk by chunk. `current-net`'s `ServerClient::send_stream` implements it in the app (ADR 0020, "Where it
//! lives"); tests use a fake.

use std::future::Future;
use std::pin::Pin;

use current_net::header::{self, HeaderMap, HeaderName};
use current_net::{GatewayShape, Url, redact_query};
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
    /// The base URL of the address the request went to (or, if the upstream cannot tell, the address selected now), without
    /// credentials: playlist URIs are resolved against it plus the request's path, and an absolute URI must be on its origin. It may
    /// differ between requests of one route when the server roams to another address (ADR 0020 choice 6).
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

/// Why an upstream request or body failed. It matches `current-net`'s `StreamError` (ADR 0020 task 1), so the app's adapter maps
/// it one to one. No variant holds a URL, a header or the server's text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamError {
    /// A refusal: 401, 403, a redirect (3xx other than 304), or an HTML page where media was expected (2xx `text/html`). `shape` is
    /// `current-net`'s classification of it (ADR 0017).
    #[error("the server or a gateway refused the stream ({status})")]
    Refused { status: u16, shape: GatewayShape },
    /// Any other status of 400 or more. Carries no header or body: a 416's `Content-Range` is not passed on.
    #[error("the server answered with status {status}")]
    Status { status: u16 },
    #[error("the server could not be reached")]
    Unreachable,
    #[error("the server did not answer in time")]
    Timeout,
    #[error("the secure connection to the server failed")]
    Tls,
    /// The request could not be built for the address in use: a bug, not a server problem.
    #[error("the request could not be built")]
    InvalidRequest,
}

/// A future the relay can hold across polls.
pub type UpstreamFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, UpstreamError>> + Send + 'a>>;

/// A response body, read one chunk at a time. The relay asks for the next chunk only when the player's socket can take more, so a
/// player that stops reading stops the reads upstream (ADR 0020, "Streaming"). Dropping the body ends the upstream request.
pub trait UpstreamBody: Send {
    /// The next chunk, or `None` at the end of the body. An error ends the body: the player's connection is closed.
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>>;
}

/// Sends the relay's requests to one server.
///
/// The contract, as for `current-net`'s `ServerClient::send_stream`: the request goes to the address selected now, with that
/// address's gateway credentials and the service's token, and no redirect is followed. `Ok` for a 2xx (other than an HTML page) or
/// 304 answer; every other answer is an [`UpstreamError`]. The relay also treats an `Ok` with another status as the error it should
/// have been.
pub trait Upstream: Send + Sync {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse>;
}
