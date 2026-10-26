//! One HTTP client per configured server.

use std::sync::{Arc, Once};
use std::time::{Duration, SystemTime};

use current_model::ServerRef;
use reqwest::header::HeaderMap;
use reqwest::{Method, redirect};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use url::Url;

use crate::address::{AddressOutcome, AddressProvider, Selected};
use crate::error::{Location, NetError};
use crate::gateway::Gateway;
use crate::log::{LogEntry, Outcome, RequestLog, redact_query};
use crate::scope::transport;

/// Adds credentials to a request; implemented by the service crates. It runs after the request log entry is built, so nothing it adds
/// can reach the log.
pub trait RequestAuth: Send + Sync {
    fn apply(&self, headers: &mut HeaderMap, query: &mut Vec<(String, String)>);
}

/// Client settings. Every request has a timeout: [`ClientConfig::timeout`] unless the request sets its own.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// The whole request, from connecting to the end of the body.
    pub timeout: Duration,
    /// Connecting, including the TLS handshake.
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

/// A request relative to the server's address. Its `Debug` output shows no body, no header values and only the query values the
/// request log shows, since bodies, headers and queries can carry passwords and tokens.
#[derive(Clone)]
pub struct Request {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Option<Vec<u8>>,
    timeout: Option<Duration>,
    max_body: Option<usize>,
    pub(crate) read_timeout: Option<Duration>,
}

impl Request {
    /// A request for `path`, relative to the server's address. Paths are logged as they are: they must never hold a secret.
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            headers: HeaderMap::new(),
            body: None,
            timeout: None,
            max_body: None,
            read_timeout: None,
        }
    }

    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }

    /// Adds a query pair. Its value is redacted in the log unless the key is known to be harmless.
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    /// Adds a header. Headers are never logged.
    pub fn header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Sets a JSON body.
    pub fn json(mut self, body: &impl Serialize) -> Result<Self, NetError> {
        let bytes = serde_json::to_vec(body).map_err(|_| NetError::InvalidRequest)?;
        self.headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        self.body = Some(bytes);
        Ok(self)
    }

    /// Overrides the client's timeout for this request.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Limits the response's body to `bytes`: the body is read in chunks and a longer one ends the request with
    /// [`NetError::TooLarge`], whatever its status, without reading the rest.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = Some(bytes);
        self
    }

    /// For [`ServerClient::send_stream`] only: the longest wait for the next chunk of the body, instead of
    /// [`DEFAULT_READ_TIMEOUT`](crate::DEFAULT_READ_TIMEOUT). [`ServerClient::send`] ignores it.
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = Some(timeout);
        self
    }
}

/// A successful (2xx) response. Its `Debug` output shows no body and no header values: a sign-in's body holds a token.
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    /// Where the response came from: the request's address after any followed redirects, without the credentials `RequestAuth`
    /// adds. Server setup reads it to adopt an http to https upgrade or find a base path. Redacted from Debug and never logged.
    pub url: Location,
    /// Whether a redirect was followed on the way: gateway classification (ADR 0017) treats the final answer like any other.
    pub redirected: bool,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &redact_query(&self.query))
            .field("headers", &HeaderNames(&self.headers))
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .field("timeout", &self.timeout)
            .field("read_timeout", &self.read_timeout)
            .finish()
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &HeaderNames(&self.headers))
            .field("body_bytes", &self.body.len())
            .field("url", &self.url)
            .field("redirected", &self.redirected)
            .finish()
    }
}

/// A header map's names only, for `Debug`.
struct HeaderNames<'a>(&'a HeaderMap);

impl std::fmt::Debug for HeaderNames<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Response {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, NetError> {
        serde_json::from_slice(&self.body).map_err(|error| NetError::from_decode(&error))
    }
}

/// The HTTP client for one server.
pub struct ServerClient {
    pub(crate) server: ServerRef,
    pub(crate) address: Arc<dyn AddressProvider>,
    auth: Option<Arc<dyn RequestAuth>>,
    pub(crate) log: Arc<RequestLog>,
    timeout: Duration,
    connect_timeout: Duration,
    http: reqwest::Client,
    /// For [`ServerClient::send_stream`]: no total timeout. Built on the first stream, since most clients never stream.
    stream_http: tokio::sync::OnceCell<reqwest::Client>,
}

impl std::fmt::Debug for ServerClient {
    /// Names the server only: no address, no credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerClient")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl ServerClient {
    /// A client for `server`, logging to `log` (shared by every server's client).
    pub fn new(
        server: ServerRef,
        address: Arc<dyn AddressProvider>,
        auth: Option<Arc<dyn RequestAuth>>,
        log: Arc<RequestLog>,
        config: &ClientConfig,
    ) -> Result<Self, NetError> {
        install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            // Redirects are followed in `execute`, which re-applies the credentials on every hop (ADR 0006).
            .redirect(redirect::Policy::none())
            .build()
            // Building fails only when the TLS backend cannot start.
            .map_err(|_| NetError::Tls)?;
        Ok(Self {
            server,
            address,
            auth,
            log,
            timeout: config.timeout,
            connect_timeout: config.connect_timeout,
            http,
            stream_http: tokio::sync::OnceCell::new(),
        })
    }

    /// The client for streams: like [`ServerClient::new`]'s, without the timeout for the whole request (a stream's body has none;
    /// `send_stream` times its headers and each chunk itself).
    pub(crate) async fn stream_http(&self) -> Result<&reqwest::Client, NetError> {
        self.stream_http
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .connect_timeout(self.connect_timeout)
                    // Streams follow no redirect (ADR 0020).
                    .redirect(redirect::Policy::none())
                    // One connection per stream: on HTTP/2, paused streams sharing a connection hold its flow-control window and
                    // can starve another stream into a timeout, and the relay keeps no more than a chunk per connection (ADR 0020).
                    .http1_only()
                    // As mpv's requests do (`current_mpv::user_agent`); the workspace shares one version.
                    .user_agent(concat!("Current/", env!("CARGO_PKG_VERSION")))
                    .build()
                    // Building fails only when the TLS backend cannot start.
                    .map_err(|_| NetError::Tls)
            })
            .await
    }

    pub fn server(&self) -> &ServerRef {
        &self.server
    }

    /// How long `request` may take in all: its own timeout, else the client's.
    pub fn budget(&self, request: &Request) -> Duration {
        request.timeout.unwrap_or(self.timeout)
    }

    /// Sends a request. A status outside 2xx is an error carrying the server's message.
    ///
    /// When the address in use fails (unreachable, a timeout or TLS) the provider is told, and if it picks another address within the
    /// request's deadline the request is sent there once more: any method if nothing was sent, only idempotent methods (RFC 9110
    /// §9.2.2) if it may have been (ADR 0014). Each attempt is logged.
    pub async fn send(&self, request: Request) -> Result<Response, NetError> {
        let deadline = Instant::now() + request.timeout.unwrap_or(self.timeout);
        let selected = match self.address.current() {
            Some(selected) => Some(selected),
            None => self.address.wait(deadline).await,
        };
        let Some(selected) = selected else {
            self.log(
                &request,
                None,
                Outcome::Error(NetError::Unreachable.kind()),
                Instant::now(),
            );
            return Err(NetError::Unreachable);
        };
        let (result, sent) = self.attempt(&request, &selected, deadline, false).await;
        let Err(error) = &result else {
            return result;
        };
        if outcome_of(error) == AddressOutcome::Ok || (sent && !is_idempotent(&request.method)) {
            return result;
        }
        match self.address.wait(deadline).await {
            // Only with time left: an attempt that cannot start says nothing about the new address.
            Some(next) if next.url() != selected.url() && Instant::now() < deadline => {
                self.attempt(&request, &next, deadline, true).await.0
            }
            _ => result,
        }
    }

    /// Sends `request` to `selected` once, logs it and reports the outcome to the provider. The flag says whether the request may
    /// have reached a server: false only when it failed while connecting.
    ///
    /// An attempt that never started because the deadline had passed is not reported, and neither is a retry's timeout: the retry
    /// has only what is left of the deadline, so a timeout there blames the request's budget rather than the address.
    async fn attempt(
        &self,
        request: &Request,
        selected: &Selected,
        deadline: Instant,
        retry: bool,
    ) -> (Result<Response, NetError>, bool) {
        let started = Instant::now();
        let (result, sent, expired) = match self.execute(request.clone(), selected, deadline).await
        {
            Ok(response) => (Ok(response), true, false),
            Err(failure) => (Err(failure.error), failure.sent, failure.expired),
        };
        let outcome = match &result {
            Ok(response) => Outcome::Status(response.status),
            Err(NetError::Auth { status, .. } | NetError::Status { status, .. }) => {
                Outcome::Status(*status)
            }
            Err(error) => Outcome::Error(error.kind()),
        };
        // An attempt that never started went to no address.
        let address = if expired { None } else { selected.position() };
        self.log(request, address, outcome, started);
        let reported = match &result {
            Ok(_) => AddressOutcome::Ok,
            Err(error) => outcome_of(error),
        };
        if !expired && !(retry && reported == AddressOutcome::Timeout) {
            self.address.report(selected, reported);
        }
        (result, sent)
    }

    /// Adds an entry for `request` to the log, built from the request as the caller made it: no address, no credentials.
    fn log(&self, request: &Request, address: Option<u8>, outcome: Outcome, started: Instant) {
        let entry = LogEntry {
            at: SystemTime::now(),
            server: self.server.clone(),
            address,
            method: request.method.to_string(),
            path: request.path.clone(),
            query: redact_query(&request.query),
            outcome,
            duration: started.elapsed(),
        };
        tracing::debug!(target: "current_net", "{}", entry.to_line());
        self.log.push(entry);
    }

    /// The address the client would use now, if one is selected. For identity probes, which need to know which address they check.
    pub fn current_address(&self) -> Option<Selected> {
        self.address.current()
    }

    /// Sends a request and decodes its JSON body.
    pub async fn send_json<T: DeserializeOwned>(&self, request: Request) -> Result<T, NetError> {
        self.send(request).await?.json()
    }

    /// Fresh copies of `url` and `headers` with the credentials added: the service's, then the selected address's gateway
    /// credentials (ADR 0017). Runs on every hop, so a followed redirect keeps them, and cookies are matched against each hop's URL.
    /// Gateway credentials follow the token's transport rule: https, plain HTTP to the local network, or plain HTTP the user
    /// confirmed for the address (ADR 0012, ADR 0014).
    pub(crate) fn authorize(
        &self,
        url: &Url,
        headers: &HeaderMap,
        selected: &Selected,
    ) -> (Url, HeaderMap) {
        let (mut url, mut headers) = (url.clone(), headers.clone());
        if let Some(auth) = &self.auth {
            let mut query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            auth.apply(&mut headers, &mut query);
            url.set_query(None);
            if !query.is_empty() {
                url.query_pairs_mut().extend_pairs(&query);
            }
        }
        if let Some(gateway) = gateway_for(selected, &url) {
            gateway.apply(&url, &mut headers, SystemTime::now());
        }
        (url, headers)
    }

    /// Sends `request` to `selected`, following redirects on the same host, until `deadline` (one deadline for the whole request,
    /// redirects included). Every answer's `Set-Cookie` updates the address's gateway cookies.
    async fn execute(
        &self,
        request: Request,
        selected: &Selected,
        deadline: Instant,
    ) -> Result<Response, Failure> {
        let Request {
            mut method,
            path,
            query,
            mut headers,
            mut body,
            timeout: _,
            max_body,
            read_timeout: _,
        } = request;
        let original = join(selected.url(), &path, &query).map_err(Failure::unsent)?;
        let mut url = original.clone();
        let mut hops = 0;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or(Failure {
                    error: NetError::Timeout,
                    sent: hops > 0,
                    expired: hops == 0,
                })?;
            let (authorized_url, authorized_headers) = self.authorize(&url, &headers, selected);
            let mut builder = self
                .http
                .request(method.clone(), authorized_url)
                .headers(authorized_headers)
                .timeout(remaining);
            if let Some(body) = &body {
                builder = builder.body(body.clone());
            }
            // reqwest reports a failure in its connector (DNS, TCP and the TLS handshake) as a connect error: hyper-util 0.1.21
            // `client/legacy/client.rs` wraps the connector's error as `Connect`, which `is_connect()` looks for, and reqwest 0.13.5
            // `connect.rs` does the rustls handshake inside that connector. Nothing was sent then, unless an earlier hop was.
            let response = builder.send().await.map_err(|error| Failure {
                sent: hops > 0 || !error.is_connect(),
                error: NetError::from_reqwest(error),
                expired: false,
            })?;
            let status = response.status().as_u16();
            let response_headers = response.headers().clone();
            if let Some(gateway) = gateway_for(selected, &url) {
                gateway.update(&url, &response_headers, SystemTime::now());
            }

            if let Some(next) = redirect_target(status, &response_headers, &url) {
                if hops >= MAX_REDIRECTS || !may_follow(&original, &next) {
                    return Err(Failure {
                        error: NetError::Redirected {
                            status,
                            location: Some(Location(next)),
                        },
                        sent: true,
                        expired: false,
                    });
                }
                // As browsers do: 303 always becomes GET, and so do 301 and 302 for anything but GET and HEAD; 307 and 308 keep the
                // method and body.
                if (status == 303 && method != Method::HEAD)
                    || (matches!(status, 301 | 302)
                        && method != Method::GET
                        && method != Method::HEAD)
                {
                    method = Method::GET;
                    body = None;
                    headers.remove(reqwest::header::CONTENT_TYPE);
                }
                hops += 1;
                url = next;
                continue;
            }

            let body = read_body(response, max_body)
                .await
                .map_err(|error| Failure {
                    error,
                    sent: true,
                    expired: false,
                })?;
            if !(200..300).contains(&status) {
                return Err(Failure {
                    error: NetError::from_status(status, &response_headers, &body),
                    sent: true,
                    expired: false,
                });
            }
            return Ok(Response {
                status,
                headers: response_headers,
                body,
                url: Location(url),
                redirected: hops > 0,
            });
        }
    }
}

/// The response's body, at most `limit` bytes when there is a limit.
async fn read_body(
    mut response: reqwest::Response,
    limit: Option<usize>,
) -> Result<Vec<u8>, NetError> {
    let Some(limit) = limit else {
        return Ok(response
            .bytes()
            .await
            .map_err(NetError::from_reqwest)?
            .to_vec());
    };
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(NetError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(NetError::from_reqwest)? {
        if body.len() + chunk.len() > limit {
            return Err(NetError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The selected address's gateway, if its credentials may go to `url` (a hop on that address): https, plain HTTP to the local
/// network, or plain HTTP the user confirmed for the address. The same rule decides whether an answer from `url` may update them.
pub(crate) fn gateway_for<'a>(selected: &'a Selected, url: &Url) -> Option<&'a Arc<Gateway>> {
    selected
        .gateway()
        .filter(|_| !transport(url).needs_confirmation() || selected.plain_http_confirmed())
}

/// A failed attempt, and whether the request may have reached a server.
struct Failure {
    error: NetError,
    sent: bool,
    /// The deadline had passed before the attempt started: nothing was tried.
    expired: bool,
}

impl Failure {
    fn unsent(error: NetError) -> Self {
        Self {
            error,
            sent: false,
            expired: false,
        }
    }
}

/// What a request's error says about the address it went to.
fn outcome_of(error: &NetError) -> AddressOutcome {
    match error {
        NetError::Unreachable => AddressOutcome::Unreachable,
        NetError::Timeout => AddressOutcome::Timeout,
        NetError::Tls => AddressOutcome::Tls,
        _ => AddressOutcome::Ok,
    }
}

/// Methods that may be repeated after a failure once the request may have been sent (RFC 9110 §9.2.2, as listed in ADR 0014). Never
/// POST or PATCH.
fn is_idempotent(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
    ]
    .contains(method)
}

/// Where a redirect response points, resolved against the request's URL. `None` for any other response, including a 3xx without a
/// usable `Location`.
fn redirect_target(status: u16, headers: &HeaderMap, url: &Url) -> Option<Url> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = headers.get(reqwest::header::LOCATION)?.to_str().ok()?;
    url.join(location).ok()
}

/// How many redirects one request follows.
const MAX_REDIRECTS: usize = 5;

/// Whether a redirect from the request's original address may be followed: only to the same host, on the same scheme and port or as
/// an upgrade from http to https. Credentials therefore never reach another host (ADR 0006).
fn may_follow(from: &Url, to: &Url) -> bool {
    if from.host() != to.host() {
        return false;
    }
    match (from.scheme(), to.scheme()) {
        ("http", "https") => true,
        (a, b) if a == b => from.port_or_known_default() == to.port_or_known_default(),
        _ => false,
    }
}

/// The base address with `path` appended to its own path and `query` as its query.
pub(crate) fn join(base: &Url, path: &str, query: &[(String, String)]) -> Result<Url, NetError> {
    if base.cannot_be_a_base() || !matches!(base.scheme(), "http" | "https") {
        return Err(NetError::InvalidRequest);
    }
    let mut url = base.clone();
    url.set_path(&format!(
        "{}/{}",
        base.path().trim_end_matches('/'),
        path.trim_start_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

/// Selects ring as rustls's crypto provider for the process (ADR 0006). reqwest uses the process default when it builds a client.
fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Fails only if the app already chose a provider, which then stays in use.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use current_model::{ServerKey, ServiceType};
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn a_request_debug_shows_no_body_header_value_or_secret_query_value() {
        let request = Request::post("/Users/AuthenticateByName")
            .query("limit", "5")
            .query("secret", "qc-secret-value")
            .header(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_static("MediaBrowser Token=\"token-value\""),
            )
            .json(&serde_json::json!({"Username": "Example User", "Pw": "pa55-word-value"}))
            .unwrap();
        let text = format!("{request:?}");
        for secret in [
            "pa55-word-value",
            "Example User",
            "token-value",
            "qc-secret-value",
            "MediaBrowser",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(text.contains("/Users/AuthenticateByName"), "{text}");
        assert!(text.contains("\"limit\", \"5\""), "{text}");
        assert!(text.contains("authorization"), "{text}");
        assert!(text.contains("body_bytes: Some("), "{text}");
    }

    #[test]
    fn a_response_debug_shows_no_body_header_value_or_address() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_static("session=cookie-value"),
        );
        let response = Response {
            status: 200,
            headers,
            body: br#"{"AccessToken":"token-value"}"#.to_vec(),
            url: Location(url("http://192.168.1.10:8096/Users/AuthenticateByName")),
            redirected: false,
        };
        let text = format!("{response:?}");
        for secret in ["token-value", "cookie-value", "192.168.1.10"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(text.contains("status: 200"), "{text}");
        assert!(text.contains("set-cookie"), "{text}");
        assert!(text.contains("body_bytes: 29"), "{text}");
    }

    /// Serves `first` until a request fails, then offers `second` (as a provider that just re-raced would), and records reports.
    struct Switching {
        first: Selected,
        second: Selected,
        reports: Mutex<Vec<(Option<u8>, AddressOutcome)>>,
    }

    impl AddressProvider for Switching {
        fn current(&self) -> Option<Selected> {
            Some(self.first.clone())
        }

        fn wait(
            &self,
            _deadline: Instant,
        ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
            Box::pin(std::future::ready(Some(self.second.clone())))
        }

        fn report(&self, selected: &Selected, outcome: AddressOutcome) {
            self.reports
                .lock()
                .unwrap()
                .push((selected.position(), outcome));
        }
    }

    /// A server that accepts connections, counts them and never answers.
    async fn silent() -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        (url(&format!("http://127.0.0.1:{port}")), accepted)
    }

    fn switching_client(first: Url, second: Url) -> (ServerClient, Arc<Switching>) {
        let provider = Arc::new(Switching {
            first: Selected::new(first, Some(1), 1),
            second: Selected::new(second, Some(2), 2),
            reports: Mutex::new(Vec::new()),
        });
        let client = ServerClient::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            provider.clone(),
            None,
            Arc::new(RequestLog::default()),
            &ClientConfig::default(),
        )
        .unwrap();
        (client, provider)
    }

    #[tokio::test]
    async fn no_retry_once_the_deadline_is_gone() {
        let (first, _) = silent().await;
        let (second, second_accepted) = silent().await;
        let (client, provider) = switching_client(first, second);
        let request = Request::get("/Items").timeout(Duration::from_millis(300));
        assert_eq!(
            client.send(request).await.map(|_| ()),
            Err(NetError::Timeout)
        );
        // The new address was neither contacted nor blamed.
        assert_eq!(
            *provider.reports.lock().unwrap(),
            [(Some(1), AddressOutcome::Timeout)]
        );
        assert_eq!(second_accepted.load(Ordering::SeqCst), 0);
        // Nothing was sent to address 2, so no log line names it.
        assert!(
            !client.log.to_text().contains("address 2"),
            "{}",
            client.log.to_text()
        );
    }

    #[tokio::test]
    async fn a_retry_that_runs_out_of_time_does_not_blame_the_new_address() {
        // A port whose listener is gone: connections are refused. Another test could take the port meanwhile, which is unlikely.
        // (A socket bound but not listening does not refuse everywhere: on the macOS runner connections to one hung.)
        let refused = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        let (second, second_accepted) = silent().await;
        let (client, provider) =
            switching_client(url(&format!("http://127.0.0.1:{refused}")), second);
        let request = Request::get("/Items").timeout(Duration::from_secs(5));
        assert_eq!(
            client.send(request).await.map(|_| ()),
            Err(NetError::Timeout)
        );
        assert_eq!(second_accepted.load(Ordering::SeqCst), 1);
        assert_eq!(
            *provider.reports.lock().unwrap(),
            [(Some(1), AddressOutcome::Unreachable)]
        );
    }

    fn url(text: &str) -> Url {
        match Url::parse(text) {
            Ok(url) => url,
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn gateway_credentials_follow_the_token_transport_rule() {
        let gateway = Arc::new(Gateway::new(crate::gateway::GatewayCredentials::default()));
        let allowed = |address: &str, confirmed: bool, hop: &str| {
            let selected =
                Selected::new(url(address), None, 0).with_gateway(Some(gateway.clone()), confirmed);
            gateway_for(&selected, &url(hop)).is_some()
        };
        assert!(allowed(
            "https://media.example.com/",
            false,
            "https://media.example.com/a"
        ));
        assert!(allowed(
            "http://192.168.1.10:8096/",
            false,
            "http://192.168.1.10:8096/a"
        ));
        assert!(!allowed(
            "http://media.example.com/",
            false,
            "http://media.example.com/a"
        ));
        assert!(allowed(
            "http://media.example.com/",
            true,
            "http://media.example.com/a"
        ));
        // An upgrade hop is https: allowed even before a confirmation.
        assert!(allowed(
            "http://media.example.com/",
            false,
            "https://media.example.com/a"
        ));
        let none = Selected::new(url("https://media.example.com/"), None, 0);
        assert!(gateway_for(&none, &url("https://media.example.com/")).is_none());
    }

    #[test]
    fn redirects_stay_on_the_same_host() {
        let follows = |from: &str, to: &str| may_follow(&url(from), &url(to));
        assert!(follows(
            "http://192.168.1.10:8096/a",
            "http://192.168.1.10:8096/a/"
        ));
        assert!(follows(
            "http://media.example.com/a",
            "https://media.example.com/a"
        ));
        assert!(follows(
            "http://media.example.com:8096/a",
            "https://media.example.com/a"
        ));
        assert!(follows(
            "https://media.example.com/a",
            "https://media.example.com:443/b"
        ));
        assert!(!follows(
            "https://media.example.com/a",
            "http://media.example.com/a"
        ));
        assert!(!follows(
            "http://media.example.com/a",
            "http://other.example.com/a"
        ));
        assert!(!follows(
            "http://192.168.1.10:8096/a",
            "http://192.168.1.10:8097/a"
        ));
        assert!(!follows("http://192.168.1.10/a", "http://127.0.0.1/a"));
    }

    #[test]
    fn paths_join_under_the_base_path() {
        let query = vec![("a".to_owned(), "1 2".to_owned())];
        let joined = |base: &str, path: &str| join(&url(base), path, &query).map(String::from);
        assert_eq!(
            joined("http://192.168.1.10:8096", "/Items").as_deref(),
            Ok("http://192.168.1.10:8096/Items?a=1+2")
        );
        assert_eq!(
            joined("https://media.example.com/jellyfin/", "Items/1").as_deref(),
            Ok("https://media.example.com/jellyfin/Items/1?a=1+2")
        );
        assert_eq!(
            joined("https://media.example.com/jellyfin?x=1#f", "/Items").as_deref(),
            Ok("https://media.example.com/jellyfin/Items?a=1+2")
        );
        assert_eq!(
            joined("mailto:someone@example.com", "/Items"),
            Err(NetError::InvalidRequest)
        );
    }
}
