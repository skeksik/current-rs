//! Probing an address: is a Jellyfin server there, which one, and can Current use it (ADR 0012).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use current_model::{PublicServerInfo, ServerAddress, ServerId, ServerRef};
use current_net::{
    ClientConfig, FixedAddress, IdentityCheck, IdentityProbe, Location, NetError, Request,
    RequestLog, ServerClient, Transport, Url,
};

use crate::address::{AddressError, parse_address};
use crate::wire;

/// The unauthenticated endpoint that identifies a Jellyfin server.
const INFO_PATH: &str = "/System/Info/Public";

/// The oldest server version Current accepts (ADR 0002: the latest releases only; Jellyfin 12.0 broke parts of the 10.11 API).
pub const MINIMUM_VERSION: &str = "12.0.0";
const MINIMUM: [u64; 3] = [12, 0, 0];

/// Why a probe found no usable server. Neither Display nor Debug holds an address: a redirect's target is a [`Location`], which hides
/// it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error(transparent)]
    InvalidAddress(#[from] AddressError),
    /// Nothing answered: offline, the name did not resolve, or nothing listens there.
    #[error("the server could not be reached")]
    Unreachable,
    #[error("the server did not answer in time")]
    Timeout,
    /// The secure connection failed, typically an untrusted (for example self-signed) or mismatched certificate.
    #[error("the secure connection to the server failed")]
    Tls,
    /// Something answered, but not a Jellyfin server.
    #[error("this is not a Jellyfin server")]
    NotJellyfin,
    /// 503: the server is starting, or it does not accept connections from this network (remote access off or an IP filter). Jellyfin
    /// answers both the same way, so they cannot be told apart.
    #[error("the server is starting or does not accept connections from this network")]
    Unavailable,
    /// The server points elsewhere (another host or port, or from https to http) and the redirect was not followed (ADR 0006).
    /// `location` is where it points, for the UI to offer.
    #[error("the server redirects to another address")]
    Redirected {
        status: u16,
        location: Option<Location>,
    },
    /// Any other unexpected status.
    #[error("the server answered with status {status}")]
    Status { status: u16 },
    /// A Jellyfin server older than [`MINIMUM_VERSION`], or whose version Current cannot read.
    #[error(
        "this server's version ({version}) is not supported: Jellyfin {MINIMUM_VERSION} or newer is needed"
    )]
    TooOld { version: String },
    /// The server's first-run setup is not finished. Jellyfin also reports this while it is still starting.
    #[error("the server's setup is not finished yet")]
    SetupIncomplete,
    /// The address leads to another server than the one expected.
    #[error("this address leads to a different server")]
    DifferentServer,
}

impl ProbeError {
    /// Whether a Jellyfin server answered: trying other candidates cannot do better.
    fn is_conclusive(&self) -> bool {
        matches!(self, Self::TooOld { .. } | Self::SetupIncomplete)
    }

    /// How much an error tells the user, when several candidates failed: the most telling one is reported.
    fn rank(&self) -> u8 {
        match self {
            Self::InvalidAddress(_) | Self::DifferentServer => 0,
            Self::Unreachable => 1,
            Self::Timeout => 2,
            Self::NotJellyfin | Self::Status { .. } => 3,
            Self::Tls => 4,
            Self::Redirected { .. } => 5,
            Self::Unavailable => 6,
            Self::TooOld { .. } | Self::SetupIncomplete => 7,
        }
    }
}

impl From<NetError> for ProbeError {
    fn from(error: NetError) -> Self {
        match error {
            NetError::Unreachable => Self::Unreachable,
            NetError::Timeout => Self::Timeout,
            NetError::Tls => Self::Tls,
            NetError::Status { status: 503, .. } => Self::Unavailable,
            NetError::Status { status: 404, .. } | NetError::Decode { .. } => Self::NotJellyfin,
            NetError::Auth { status, .. } | NetError::Status { status, .. } => {
                Self::Status { status }
            }
            NetError::Redirected { status, location } => Self::Redirected { status, location },
            NetError::InvalidRequest => Self::InvalidAddress(AddressError::Invalid),
        }
    }
}

/// A Jellyfin server Current can use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundServer {
    pub info: PublicServerInfo,
    /// The base URL it answered on: the candidate tried, upgraded to https or moved to a base path when the server redirected there.
    pub address: Url,
    /// How the connection is protected; [`Transport::PlainPublic`] needs the user's confirmation before the address is used.
    pub transport: Transport,
    /// An https candidate failed its TLS handshake before this plain http one answered: the server may offer https with a
    /// certificate this device does not trust. The UI says so rather than silently settling for http.
    pub https_failed_tls: bool,
}

impl FoundServer {
    /// Whether the user must confirm plain HTTP before the address is used.
    pub fn needs_confirmation(&self) -> bool {
        self.transport.needs_confirmation()
    }

    /// The address as the model keeps it, with whether the user confirmed plain HTTP to it.
    pub fn server_address(&self, plain_http_confirmed: bool) -> ServerAddress {
        ServerAddress {
            url: String::from(self.address.clone())
                .trim_end_matches('/')
                .to_owned(),
            plain_http_confirmed,
        }
    }
}

/// Whether two answers came from the same server: the same server id, whatever the address.
pub fn same_server(a: &PublicServerInfo, b: &PublicServerInfo) -> bool {
    a.id == b.id
}

/// Probes addresses for Jellyfin servers. Its requests go to the shared request log under the `ServerRef` it is given.
#[derive(Debug)]
pub struct Prober {
    server: ServerRef,
    log: Arc<RequestLog>,
    config: ClientConfig,
}

impl Prober {
    /// A prober with short timeouts (5 s to connect, 10 s per request), since a dead candidate delays the next one.
    pub fn new(server: ServerRef, log: Arc<RequestLog>) -> Self {
        Self::with_config(
            server,
            log,
            ClientConfig {
                timeout: Duration::from_secs(10),
                connect_timeout: Duration::from_secs(5),
            },
        )
    }

    pub fn with_config(server: ServerRef, log: Arc<RequestLog>, config: ClientConfig) -> Self {
        Self {
            server,
            log,
            config,
        }
    }

    /// Probes what the user typed: each candidate from [`parse_address`] in turn, until a Jellyfin server answers. When none does,
    /// the most telling error is returned. A name that does not resolve here gets only its first candidate tried, since every
    /// candidate shares the name (a proxy may still resolve it).
    ///
    /// # Errors
    /// [`ProbeError`] when no candidate leads to a usable server.
    pub async fn probe(&self, input: &str) -> Result<FoundServer, ProbeError> {
        let candidates = parse_address(input)?;
        let tried = if resolves(&candidates[0]).await {
            &candidates[..]
        } else {
            &candidates[..1]
        };
        let mut best: Option<ProbeError> = None;
        let mut https_failed_tls = false;
        for candidate in tried {
            match self.probe_url(candidate).await {
                Ok(mut found) => {
                    found.https_failed_tls =
                        https_failed_tls && found.transport != Transport::Secure;
                    return Ok(found);
                }
                Err(error) => {
                    if error == ProbeError::Tls && candidate.scheme() == "https" {
                        https_failed_tls = true;
                    }
                    if error.is_conclusive() {
                        return Err(error);
                    }
                    if best.as_ref().is_none_or(|best| error.rank() > best.rank()) {
                        best = Some(error);
                    }
                }
            }
        }
        Err(best.unwrap_or(ProbeError::Unreachable))
    }

    /// Probes one base URL. Follows the server's own redirects on the same host (ADR 0006): an http to https upgrade is adopted as
    /// the address, and a redirect to the web client (what Jellyfin answers for a path outside its base path) moves the probe to
    /// the base path once.
    ///
    /// # Errors
    /// [`ProbeError`] when no usable Jellyfin server answers there.
    pub async fn probe_url(&self, base: &Url) -> Result<FoundServer, ProbeError> {
        let mut base = base.clone();
        for _ in 0..2 {
            let client = ServerClient::new(
                self.server.clone(),
                Arc::new(FixedAddress(base.clone())),
                None,
                self.log.clone(),
                &self.config,
            )?;
            match ask(&client).await? {
                Answer::Found(found) => return Ok(found),
                Answer::WebClient(next) if next != base => base = next,
                Answer::WebClient(_) => break,
            }
        }
        Err(ProbeError::NotJellyfin)
    }

    /// Probes one base URL and checks it leads to the server with `expected` id: for an address of a server already added.
    ///
    /// # Errors
    /// [`ProbeError::DifferentServer`] for another server, or any error of [`Prober::probe_url`].
    pub async fn probe_expecting(
        &self,
        base: &Url,
        expected: &ServerId,
    ) -> Result<FoundServer, ProbeError> {
        let found = self.probe_url(base).await?;
        if found.info.id == *expected {
            Ok(found)
        } else {
            Err(ProbeError::DifferentServer)
        }
    }
}

/// What an address answered to the info request.
enum Answer {
    Found(FoundServer),
    /// The server sent the request to its web client: its base path is this one.
    WebClient(Url),
}

/// Sends the info request on `client` and reads the answer.
async fn ask(client: &ServerClient) -> Result<Answer, ProbeError> {
    let response = client.send(Request::get(INFO_PATH)).await?;
    let answered = response.url.url();
    if let Some(address) = strip_info_path(answered) {
        let info = response.json::<wire::PublicSystemInfo>()?;
        return found(info, address).map(Answer::Found);
    }
    web_client_base(answered)
        .map(Answer::WebClient)
        .ok_or(ProbeError::NotJellyfin)
}

/// The identity check for roaming between a server's addresses (ADR 0014): the address must answer as a usable Jellyfin server with
/// the expected id. Runs on `current-net`'s probe client for the address, which carries no credentials and is reused between races.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JellyfinIdentity {
    expected: ServerId,
}

impl JellyfinIdentity {
    pub fn new(expected: ServerId) -> Self {
        Self { expected }
    }

    /// Checks the address `client` is bound to. A server that sends the request to its web client fails: a stored address is
    /// already the base path, unlike what a user types in setup.
    pub async fn check_client(&self, client: &ServerClient) -> IdentityCheck {
        match ask(client).await {
            Ok(Answer::Found(found)) if found.info.id == self.expected => IdentityCheck::Passed,
            Ok(Answer::Found(_)) => IdentityCheck::DifferentServer,
            Ok(Answer::WebClient(_)) | Err(_) => IdentityCheck::Failed,
        }
    }
}

impl IdentityProbe for JellyfinIdentity {
    fn check<'a>(
        &'a self,
        client: &'a ServerClient,
    ) -> Pin<Box<dyn Future<Output = IdentityCheck> + Send + 'a>> {
        Box::pin(self.check_client(client))
    }
}

/// Whether the candidate's host is an IP address or a name that resolves here.
async fn resolves(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let port = url.port_or_known_default().unwrap_or(80);
            tokio::net::lookup_host((name, port)).await.is_ok()
        }
        Some(_) => true,
        None => false,
    }
}

/// The base URL a response to `{base}/System/Info/Public` came from, or `None` if it came from another path.
fn strip_info_path(answered: &Url) -> Option<Url> {
    let path = answered.path();
    let cut = path.len().checked_sub(INFO_PATH.len())?;
    let (base, rest) = path.split_at_checked(cut)?;
    if !rest.eq_ignore_ascii_case(INFO_PATH) {
        return None;
    }
    let mut url = answered.clone();
    url.set_path(if base.is_empty() { "/" } else { base });
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

/// The base URL before the last `web` segment, when the server sent the probe to its web client.
fn web_client_base(answered: &Url) -> Option<Url> {
    let segments: Vec<&str> = answered.path_segments()?.collect();
    let web = segments
        .iter()
        .rposition(|segment| segment.eq_ignore_ascii_case("web"))?;
    let mut url = answered.clone();
    url.set_path(&format!("/{}", segments[..web].join("/")));
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

/// Checks an answer and turns it into a [`FoundServer`].
fn found(info: wire::PublicSystemInfo, address: Url) -> Result<FoundServer, ProbeError> {
    let (Some(id), Some(version)) = (
        info.id.filter(|id| !id.is_empty()),
        info.version.filter(|version| !version.is_empty()),
    ) else {
        return Err(ProbeError::NotJellyfin);
    };
    if !version_supported(&version) {
        return Err(ProbeError::TooOld { version });
    }
    // Nullable only for older API clients (PublicSystemInfo.cs); the server always sets it.
    let setup_completed = info.startup_wizard_completed.unwrap_or(true);
    if !setup_completed {
        return Err(ProbeError::SetupIncomplete);
    }
    let transport = current_net::transport(&address);
    Ok(FoundServer {
        info: PublicServerInfo {
            id: ServerId::new(id),
            name: info.server_name.unwrap_or_default(),
            version,
            product: info.product_name.unwrap_or_default(),
            local_address: info.local_address.filter(|address| !address.is_empty()),
            setup_completed,
        },
        address,
        transport,
        https_failed_tls: false,
    })
}

/// Whether `version` is at least [`MINIMUM_VERSION`]. Read leniently: the leading dotted numbers count (`12.2.0`, `12.2`, and a
/// fork's `12.2.0-custom`); a version without a leading number is not supported.
fn version_supported(version: &str) -> bool {
    let numeric: &str = version
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .next()
        .unwrap_or_default();
    let mut parts = [0u64; 3];
    for (index, part) in numeric.split('.').take(3).enumerate() {
        match part.parse() {
            Ok(value) => parts[index] = value,
            Err(_) if index > 0 => break,
            Err(_) => return false,
        }
    }
    parts >= MINIMUM
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
    fn versions_from_12_0_on_are_supported() {
        for version in [
            "12.0.0",
            "12.0",
            "12",
            "12.2.0",
            "12.10.1",
            "13.0.0",
            "12.2.0-custom",
        ] {
            assert!(version_supported(version), "{version}");
        }
        for version in ["10.11.3", "11.9.9", "", "unknown", "v12.0.0", "1.0"] {
            assert!(!version_supported(version), "{version}");
        }
    }

    #[test]
    fn the_info_path_is_cut_from_the_answering_url() {
        let base = |text: &str| strip_info_path(&url(text)).map(String::from);
        assert_eq!(
            base("https://media.example.com:8920/System/Info/Public").as_deref(),
            Some("https://media.example.com:8920/")
        );
        assert_eq!(
            base("http://192.168.1.10:8096/jellyfin/system/info/public?x=1").as_deref(),
            Some("http://192.168.1.10:8096/jellyfin")
        );
        assert_eq!(base("http://192.168.1.10:8096/jellyfin/web/"), None);
        assert_eq!(base("http://192.168.1.10:8096/"), None);
    }

    #[test]
    fn a_web_client_url_gives_its_base() {
        let base = |text: &str| web_client_base(&url(text)).map(String::from);
        assert_eq!(
            base("http://192.168.1.10:8096/jellyfin/web/").as_deref(),
            Some("http://192.168.1.10:8096/jellyfin")
        );
        assert_eq!(
            base("http://192.168.1.10:8096/web/web/index.html").as_deref(),
            Some("http://192.168.1.10:8096/web")
        );
        assert_eq!(base("http://192.168.1.10:8096/login"), None);
    }

    #[test]
    fn the_most_telling_error_wins() {
        assert!(ProbeError::Tls.rank() > ProbeError::Unreachable.rank());
        assert!(ProbeError::Unavailable.rank() > ProbeError::NotJellyfin.rank());
        assert!(ProbeError::SetupIncomplete.is_conclusive());
        assert!(!ProbeError::Unavailable.is_conclusive());
    }

    #[test]
    fn net_errors_map_to_what_setup_shows() {
        let status = |status| NetError::Status {
            status,
            message: None,
        };
        assert_eq!(ProbeError::from(status(503)), ProbeError::Unavailable);
        assert_eq!(ProbeError::from(status(404)), ProbeError::NotJellyfin);
        assert_eq!(
            ProbeError::from(status(500)),
            ProbeError::Status { status: 500 }
        );
        assert_eq!(
            ProbeError::from(NetError::Auth {
                status: 403,
                message: None
            }),
            ProbeError::Status { status: 403 }
        );
        assert_eq!(
            ProbeError::from(NetError::Decode { line: 1, column: 1 }),
            ProbeError::NotJellyfin
        );
    }
}
