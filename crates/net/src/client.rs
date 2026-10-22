//! One HTTP client per configured server.

use std::sync::{Arc, Once};
use std::time::{Duration, Instant, SystemTime};

use current_model::ServerRef;
use reqwest::header::HeaderMap;
use reqwest::{Method, redirect};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::NetError;
use crate::log::{LogEntry, Outcome, RequestLog, redact_query};

/// Where a server is right now. The seam for roaming between a server's addresses (M1): the client asks before every request, so a
/// provider can switch addresses between requests. [`FixedAddress`] is the only provider until then.
pub trait AddressProvider: Send + Sync {
    /// The server's base address, including any path prefix it is served under.
    fn current(&self) -> Url;
}

/// A server with one address.
#[derive(Clone, Debug)]
pub struct FixedAddress(pub Url);

impl AddressProvider for FixedAddress {
    fn current(&self) -> Url {
        self.0.clone()
    }
}

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

/// A request relative to the server's address.
#[derive(Clone, Debug)]
pub struct Request {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Option<Duration>,
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
}

/// A successful (2xx) response.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, NetError> {
        serde_json::from_slice(&self.body).map_err(|error| NetError::from_decode(&error))
    }
}

/// The HTTP client for one server.
pub struct ServerClient {
    server: ServerRef,
    address: Arc<dyn AddressProvider>,
    auth: Option<Arc<dyn RequestAuth>>,
    log: Arc<RequestLog>,
    timeout: Duration,
    http: reqwest::Client,
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
            // Credentials are headers the service crates add; following a redirect could carry them to another host. A redirect is
            // reported as a status instead (ADR 0006).
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
            http,
        })
    }

    pub fn server(&self) -> &ServerRef {
        &self.server
    }

    /// Sends a request. A status outside 2xx is an error carrying the server's message.
    pub async fn send(&self, request: Request) -> Result<Response, NetError> {
        let started = Instant::now();
        let mut entry = LogEntry {
            at: SystemTime::now(),
            server: self.server.clone(),
            method: request.method.to_string(),
            path: request.path.clone(),
            query: redact_query(&request.query),
            outcome: Outcome::Error(NetError::InvalidRequest.kind()),
            duration: Duration::ZERO,
        };
        let result = self.execute(request).await;
        entry.duration = started.elapsed();
        entry.outcome = match &result {
            Ok(response) => Outcome::Status(response.status),
            Err(NetError::Auth { status, .. } | NetError::Status { status, .. }) => {
                Outcome::Status(*status)
            }
            Err(error) => Outcome::Error(error.kind()),
        };
        tracing::debug!(target: "current_net", "{}", entry.to_line());
        self.log.push(entry);
        result
    }

    /// Sends a request and decodes its JSON body.
    pub async fn send_json<T: DeserializeOwned>(&self, request: Request) -> Result<T, NetError> {
        self.send(request).await?.json()
    }

    async fn execute(&self, request: Request) -> Result<Response, NetError> {
        let Request {
            method,
            path,
            mut query,
            mut headers,
            body,
            timeout,
        } = request;
        if let Some(auth) = &self.auth {
            auth.apply(&mut headers, &mut query);
        }
        let url = join(&self.address.current(), &path, &query)?;
        let mut builder = self
            .http
            .request(method, url)
            .headers(headers)
            .timeout(timeout.unwrap_or(self.timeout));
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let response = builder.send().await.map_err(NetError::from_reqwest)?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(NetError::from_reqwest)?
            .to_vec();
        if !(200..300).contains(&status) {
            return Err(NetError::from_status(status, &body));
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// The base address with `path` appended to its own path and `query` as its query.
fn join(base: &Url, path: &str, query: &[(String, String)]) -> Result<Url, NetError> {
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
    use super::*;

    fn url(text: &str) -> Url {
        match Url::parse(text) {
            Ok(url) => url,
            Err(error) => panic!("{error}"),
        }
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
