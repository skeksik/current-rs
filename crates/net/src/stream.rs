//! Streaming requests for the loopback relay (ADR 0020): a response whose body is read chunk by chunk, with no deadline for the
//! whole body, no redirects, and only short lists of headers passed through either way.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use reqwest::Method;
use reqwest::header::{self, HeaderMap, HeaderName};
use tokio::time::Instant;

use crate::address::{AddressOutcome, AddressProvider, Selected};
use crate::client::{Request, Response, ServerClient, gateway_for, join};
use crate::error::{Location, NetError, is_html};
use crate::log::{LogEntry, Outcome, RequestLog, redact_query};
use crate::shape::{GatewayShape, classify};

/// The request headers [`ServerClient::send_stream`] passes on; every other header of the [`Request`] is dropped, `Cookie` and
/// `Authorization` included. The address's gateway credentials and [`RequestAuth`](crate::RequestAuth)'s are added as for any request.
pub const STREAM_REQUEST_HEADERS: [HeaderName; 5] = [
    header::RANGE,
    header::IF_RANGE,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
    header::ACCEPT,
];

/// The response headers a [`StreamResponse`] exposes. `Set-Cookie` is never among them: it updates the address's gateway cookies.
pub const STREAM_RESPONSE_HEADERS: [HeaderName; 7] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_RANGE,
    header::ACCEPT_RANGES,
    header::ETAG,
    header::LAST_MODIFIED,
    header::CACHE_CONTROL,
];

/// How long a stream waits for the next chunk of its body, unless the request sets its own ([`Request::read_timeout`]).
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a 401 or 403 answer's body is read to tell a sign-in page from a plain refusal.
const REFUSAL_BODY_LIMIT: usize = 4096;

/// Why a stream did not start, or stopped. Like [`NetError`], no variant carries a URL, a host or a header value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StreamError {
    /// A gateway or the server refused: 401, 403, a redirect (any 3xx but 304, never followed), or an HTML page where media was
    /// expected (a 2xx whose `Content-Type` is HTML). `shape` is [`classify`]'s answer for it; ADR 0017's 401 rule decides what it
    /// means.
    #[error("the server refused the stream ({status})")]
    Refused { status: u16, shape: GatewayShape },
    /// Any other status of 400 and up. The body is not read.
    #[error("the server answered with status {status}")]
    Status { status: u16 },
    /// No connection, or the connection was lost.
    #[error("the server could not be reached")]
    Unreachable,
    /// No headers within the request's time, or no chunk within the read timeout.
    #[error("the server did not answer in time")]
    Timeout,
    /// The secure connection failed.
    #[error("the secure connection to the server failed")]
    Tls,
    /// A method other than GET or HEAD, a body, or a path that does not fit the server's address.
    #[error("the request could not be built")]
    InvalidRequest,
}

impl StreamError {
    /// A transport error from `NetError`'s sorting of a reqwest error, which yields only these four.
    fn from_reqwest(error: reqwest::Error) -> Self {
        match NetError::from_reqwest(error) {
            NetError::Timeout => Self::Timeout,
            NetError::Tls => Self::Tls,
            NetError::InvalidRequest => Self::InvalidRequest,
            _ => Self::Unreachable,
        }
    }

    /// How the request log shows it.
    fn outcome(&self) -> Outcome {
        match self {
            // An HTML page with a success status: the status alone would read as success.
            Self::Refused { status, .. } if (200..300).contains(status) => {
                Outcome::Error("sign-in page")
            }
            Self::Refused { status, .. } | Self::Status { status } => Outcome::Status(*status),
            Self::Unreachable => Outcome::Error(NetError::Unreachable.kind()),
            Self::Timeout => Outcome::Error(NetError::Timeout.kind()),
            Self::Tls => Outcome::Error(NetError::Tls.kind()),
            Self::InvalidRequest => Outcome::Error(NetError::InvalidRequest.kind()),
        }
    }

    /// What it says about the address the request went to.
    fn address_outcome(&self) -> AddressOutcome {
        match self {
            Self::Unreachable => AddressOutcome::Unreachable,
            Self::Timeout => AddressOutcome::Timeout,
            Self::Tls => AddressOutcome::Tls,
            _ => AddressOutcome::Ok,
        }
    }
}

/// A stream whose headers have arrived: status 2xx or 304. Read its body with [`StreamResponse::next_chunk`]; dropping it closes
/// the connection to the server if the body was not read to its end. Its `Debug` output shows no header values.
pub struct StreamResponse {
    status: u16,
    headers: HeaderMap,
    body: reqwest::Response,
    read_timeout: Duration,
    failed: Option<StreamError>,
    /// For a failure while reading the body.
    report: BodyReport,
}

/// What a stream needs to log and report a failure after its headers: the entry as `send_stream` would have built it.
struct BodyReport {
    log: Arc<RequestLog>,
    entry: LogEntry,
    started: Instant,
    address: Arc<dyn AddressProvider>,
    selected: Selected,
}

impl std::fmt::Debug for StreamResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamResponse")
            .field("status", &self.status)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("read_timeout", &self.read_timeout)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl StreamResponse {
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The answer's headers that are on [`STREAM_RESPONSE_HEADERS`], and no others.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The next chunk of the body as the server sent it, or `None` at its end (at once for HEAD and 304). Waits at most the read
    /// timeout for it; the time only runs while this is awaited, so a caller that stops reading (a paused player) does not time out,
    /// and the server is slowed by TCP flow control instead. After an error, every later call returns the same error.
    ///
    /// A failure is logged (ADR 0006) and, if it says something about the address, reported to the address provider.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, StreamError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let error = match tokio::time::timeout(self.read_timeout, self.body.chunk()).await {
            Ok(Ok(chunk)) => return Ok(chunk),
            Ok(Err(error)) => StreamError::from_reqwest(error),
            Err(_) => StreamError::Timeout,
        };
        let report = &mut self.report;
        report.entry.at = SystemTime::now();
        report.entry.outcome = error.outcome();
        report.entry.duration = report.started.elapsed();
        push(&report.log, report.entry.clone());
        if error.address_outcome() != AddressOutcome::Ok {
            report
                .address
                .report(&report.selected, error.address_outcome());
        }
        self.failed = Some(error.clone());
        Err(error)
    }
}

impl ServerClient {
    /// Sends a GET or HEAD whose answer is streamed, for the loopback relay (ADR 0020). Like [`ServerClient::send`]: the selected
    /// address, its gateway credentials only, [`RequestAuth`](crate::RequestAuth)'s credentials, the plain-HTTP rule, `Set-Cookie`
    /// updating the address's gateway cookies, and one retry on a newly selected address when the first fails to connect or answer
    /// in time. Unlike it:
    /// - only [`STREAM_REQUEST_HEADERS`] of the request are sent, and only [`STREAM_RESPONSE_HEADERS`] come back;
    /// - no redirect is followed: one is [`StreamError::Refused`];
    /// - the request's time ([`ServerClient::budget`]) covers waiting for an address and for the answer's headers, and the body
    ///   has no deadline, only a read timeout per chunk ([`DEFAULT_READ_TIMEOUT`] or [`Request::read_timeout`]);
    /// - the request log gets an entry only for a failure: a status of 300 and up (304 aside), an HTML page, or a network error,
    ///   before or during the body. A success is not logged, so a long HLS stream does not push everything else out of the log.
    ///
    /// The request's body and `max_body` are not used: a request with a body is [`StreamError::InvalidRequest`].
    pub async fn send_stream(&self, request: Request) -> Result<StreamResponse, StreamError> {
        let started = Instant::now();
        if !(request.method == Method::GET || request.method == Method::HEAD)
            || request.body.is_some()
        {
            self.log_failure(&request, None, &StreamError::InvalidRequest, started);
            return Err(StreamError::InvalidRequest);
        }
        let deadline = started + self.budget(&request);
        let selected = match self.address.current() {
            Some(selected) => Some(selected),
            None => self.address.wait(deadline).await,
        };
        let Some(selected) = selected else {
            self.log_failure(&request, None, &StreamError::Unreachable, started);
            return Err(StreamError::Unreachable);
        };
        let first = self
            .stream_attempt(&request, &selected, deadline, false)
            .await;
        let Err(error) = &first else {
            return first;
        };
        if error.address_outcome() == AddressOutcome::Ok {
            return first;
        }
        // GET and HEAD may be repeated even if they reached the server (ADR 0014).
        match self.address.wait(deadline).await {
            Some(next) if next.url() != selected.url() && Instant::now() < deadline => {
                self.stream_attempt(&request, &next, deadline, true).await
            }
            _ => first,
        }
    }

    /// One attempt at `selected`, logged if it fails, with its outcome reported to the provider as `send`'s attempts are.
    async fn stream_attempt(
        &self,
        request: &Request,
        selected: &Selected,
        deadline: Instant,
        retry: bool,
    ) -> Result<StreamResponse, StreamError> {
        let started = Instant::now();
        let Some(remaining) = deadline
            .checked_duration_since(started)
            .filter(|remaining| !remaining.is_zero())
        else {
            // Nothing was tried, so no address is named or blamed.
            self.log_failure(request, None, &StreamError::Timeout, started);
            return Err(StreamError::Timeout);
        };
        let result = self
            .open_stream(request, selected, remaining, started)
            .await;
        let reported = match &result {
            Ok(_) => AddressOutcome::Ok,
            Err(error) => {
                self.log_failure(request, selected.position(), error, started);
                error.address_outcome()
            }
        };
        // A retry has only what is left of the time: its timeout blames the budget, not the address.
        if !(retry && reported == AddressOutcome::Timeout) {
            self.address.report(selected, reported);
        }
        result
    }

    /// Sends `request` to `selected` and waits up to `remaining` for the answer's headers.
    async fn open_stream(
        &self,
        request: &Request,
        selected: &Selected,
        remaining: Duration,
        started: Instant,
    ) -> Result<StreamResponse, StreamError> {
        let url = join(selected.url(), &request.path, &request.query)
            .map_err(|_| StreamError::InvalidRequest)?;
        let mut forwarded = HeaderMap::new();
        for name in &STREAM_REQUEST_HEADERS {
            for value in request.headers.get_all(name) {
                forwarded.append(name.clone(), value.clone());
            }
        }
        let (authorized_url, authorized_headers) = self.authorize(&url, &forwarded, selected);
        let http = self.stream_http().await.map_err(|_| StreamError::Tls)?;
        let send = http
            .request(request.method.clone(), authorized_url)
            .headers(authorized_headers)
            .send();
        let mut response = match tokio::time::timeout(remaining, send).await {
            Ok(result) => result.map_err(StreamError::from_reqwest)?,
            Err(_) => return Err(StreamError::Timeout),
        };
        let status = response.status().as_u16();
        if let Some(gateway) = gateway_for(selected, &url) {
            gateway.update(&url, response.headers(), SystemTime::now());
        }
        if (300..400).contains(&status) && status != 304 {
            return Err(StreamError::Refused {
                status,
                shape: classify(&Err(NetError::Redirected {
                    status,
                    location: None,
                })),
            });
        }
        if matches!(status, 401 | 403) {
            let body = refusal_body(&mut response, started + remaining).await;
            let error = NetError::from_status(status, response.headers(), &body);
            return Err(StreamError::Refused {
                status,
                shape: classify(&Err(error)),
            });
        }
        if status >= 400 {
            return Err(StreamError::Status { status });
        }
        if is_html(response.headers(), b"") {
            // Classified as `send`'s answers are: an HTML page where something else was expected.
            let page = Ok(Response {
                status,
                headers: response.headers().clone(),
                body: Vec::new(),
                url: Location(url),
                redirected: false,
            });
            return Err(StreamError::Refused {
                status,
                shape: classify(&page),
            });
        }
        let mut headers = HeaderMap::new();
        for name in &STREAM_RESPONSE_HEADERS {
            for value in response.headers().get_all(name) {
                headers.append(name.clone(), value.clone());
            }
        }
        Ok(StreamResponse {
            status,
            headers,
            body: response,
            read_timeout: request.read_timeout.unwrap_or(DEFAULT_READ_TIMEOUT),
            failed: None,
            report: BodyReport {
                log: self.log.clone(),
                entry: self.entry(
                    request,
                    selected.position(),
                    Outcome::Status(status),
                    started,
                ),
                started,
                address: self.address.clone(),
                selected: selected.clone(),
            },
        })
    }

    fn log_failure(
        &self,
        request: &Request,
        address: Option<u8>,
        error: &StreamError,
        started: Instant,
    ) {
        push(
            &self.log,
            self.entry(request, address, error.outcome(), started),
        );
    }

    /// An entry for `request` built from the request as the caller made it: no address, no credentials, no header.
    fn entry(
        &self,
        request: &Request,
        address: Option<u8>,
        outcome: Outcome,
        started: Instant,
    ) -> LogEntry {
        LogEntry {
            at: SystemTime::now(),
            server: self.server.clone(),
            address,
            method: request.method.to_string(),
            path: request.path.clone(),
            query: redact_query(&request.query),
            outcome,
            duration: started.elapsed(),
        }
    }
}

fn push(log: &RequestLog, entry: LogEntry) {
    tracing::debug!(target: "current_net", "{}", entry.to_line());
    log.push(entry);
}

/// The start of a refusal's body, enough to tell an HTML page; whatever arrived if reading fails or the request's time runs out.
async fn refusal_body(response: &mut reqwest::Response, deadline: Instant) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < REFUSAL_BODY_LIMIT {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_show_no_realm_and_log_by_status() {
        let error = StreamError::Refused {
            status: 401,
            shape: GatewayShape::Password {
                realm: Some("Private Realm Text".to_owned()),
            },
        };
        let text = format!("{error:?} {error}");
        assert!(!text.contains("Private Realm Text"), "{text}");
        assert_eq!(error.outcome(), Outcome::Status(401));
        let page = StreamError::Refused {
            status: 200,
            shape: GatewayShape::SignIn,
        };
        assert_eq!(page.outcome(), Outcome::Error("sign-in page"));
        assert_eq!(StreamError::Timeout.outcome(), Outcome::Error("timeout"));
    }
}
