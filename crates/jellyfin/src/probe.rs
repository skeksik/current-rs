//! Probing an address: is a Jellyfin server there, which one, and can Current use it (ADR 0012).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use current_model::{PublicServerInfo, ServerAddress, ServerId, ServerRef};
use current_net::{
    AddressProvider, ClientConfig, FixedAddress, Gateway, GatewayAddress, IdentityCheck,
    IdentityProbe, Location, NetError, Request, RequestLog, ServerClient, Transport, Url,
};

use crate::address::{AddressError, parse_address};
use crate::gateway::{GatewayAnswer, Realm};
use crate::wire;

/// The unauthenticated endpoint that identifies a Jellyfin server.
pub(crate) const INFO_PATH: &str = "/System/Info/Public";

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
    /// `location` is where it points, for the UI to offer. Ambiguous: the server moved, or a gateway sends the user to its sign-in
    /// portal (ADR 0017), so setup offers both "Sign in" and "use the address it points to" (see [`ProbeError::offers_sign_in`]).
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
    /// A password gateway in front of the server: 401 or 403 with a `WWW-Authenticate: Basic` challenge (ADR 0017). Setup asks for
    /// the gateway's user name and password; `realm` is the server's text for "The site says: ...", never in Display or Debug.
    #[error("the server's address asks for a password")]
    GatewayPassword { realm: Option<Realm> },
    /// A web sign-in gateway: an HTML page where Jellyfin's JSON was expected (also at the end of a followed redirect), or a 401 or 403
    /// without a Basic challenge (Jellyfin's public endpoint never answers either). Setup offers the sign-in window.
    #[error("the server's address needs a sign-in in a browser window")]
    GatewaySignIn,
    /// A 403 without a challenge and without a page: the gateway wants something else, such as extra headers.
    #[error("the server's address refused access")]
    GatewayRefused,
    /// A 401 whose challenges are all of schemes Current does not support (Digest, NTLM, Negotiate, Bearer, ...).
    #[error("the server asks for a sign-in method Current does not support")]
    UnsupportedChallenge,
}

impl ProbeError {
    /// Whether setup offers "Sign in" (the web sign-in window, ADR 0017): for a web sign-in gateway, and next to "use the address it
    /// points to" for a redirect that was not followed, which may be a gateway's portal as well as a moved server.
    pub fn offers_sign_in(&self) -> bool {
        matches!(self, Self::GatewaySignIn | Self::Redirected { .. })
    }

    /// Whether the answer came from an authentication gateway in front of the server.
    pub fn is_gateway(&self) -> bool {
        matches!(
            self,
            Self::GatewayPassword { .. }
                | Self::GatewaySignIn
                | Self::GatewayRefused
                | Self::UnsupportedChallenge
        )
    }

    /// Whether a Jellyfin server answered: trying other candidates cannot do better.
    fn is_conclusive(&self) -> bool {
        matches!(self, Self::TooOld { .. } | Self::SetupIncomplete)
    }

    /// Whether an HTTP server answered: the request reached it and it sent a response.
    fn http_answered(&self) -> bool {
        matches!(
            self,
            Self::NotJellyfin | Self::Status { .. } | Self::Redirected { .. } | Self::Unavailable
        ) || self.is_gateway()
    }

    /// How much an error tells the user, when several candidates failed: the most telling one is reported. A gateway's answer
    /// beats a redirect and a 503: the user can sign in to it.
    fn rank(&self) -> u8 {
        match self {
            Self::InvalidAddress(_) | Self::DifferentServer => 0,
            Self::Unreachable => 1,
            Self::Timeout => 2,
            Self::NotJellyfin | Self::Status { .. } => 3,
            Self::Tls => 4,
            Self::Redirected { .. } => 5,
            Self::Unavailable => 6,
            Self::GatewayPassword { .. }
            | Self::GatewaySignIn
            | Self::GatewayRefused
            | Self::UnsupportedChallenge => 7,
            Self::TooOld { .. } | Self::SetupIncomplete => 8,
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

impl From<GatewayAnswer> for ProbeError {
    fn from(answer: GatewayAnswer) -> Self {
        match answer {
            GatewayAnswer::Password { realm } => Self::GatewayPassword { realm },
            GatewayAnswer::SignIn => Self::GatewaySignIn,
            GatewayAnswer::Refused => Self::GatewayRefused,
            GatewayAnswer::UnsupportedChallenge => Self::UnsupportedChallenge,
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
        ServerAddress::new(
            String::from(self.address.clone())
                .trim_end_matches('/')
                .to_owned(),
            plain_http_confirmed,
        )
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
        Self::with_config(server, log, probe_config())
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
        let mut failures: Vec<(&Url, ProbeError)> = Vec::new();
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
                    failures.push((candidate, error));
                }
            }
        }
        Err(most_telling(&failures))
    }

    /// Probes one base URL. Follows the server's own redirects on the same host (ADR 0006): an http to https upgrade is adopted as
    /// the address, and a redirect to the web client (what Jellyfin answers for a path outside its base path) moves the probe to
    /// the base path once.
    ///
    /// # Errors
    /// [`ProbeError`] when no usable Jellyfin server answers there.
    pub async fn probe_url(&self, base: &Url) -> Result<FoundServer, ProbeError> {
        self.probe_url_with(base, None, false).await
    }

    /// Like [`Prober::probe_url`], with the address's own gateway credentials (ADR 0017, "Order of credentials"): the probe carries
    /// them, following the token's transport rule (`plain_http_confirmed`, ADR 0012), and never the Jellyfin token. The first probe
    /// of an address is made without credentials; this one once the user gave the gateway's.
    ///
    /// # Errors
    /// As [`Prober::probe_url`].
    pub async fn probe_url_with(
        &self,
        base: &Url,
        gateway: Option<Arc<Gateway>>,
        plain_http_confirmed: bool,
    ) -> Result<FoundServer, ProbeError> {
        let mut base = base.clone();
        for _ in 0..2 {
            let address: Arc<dyn AddressProvider> = match &gateway {
                Some(gateway) => Arc::new(GatewayAddress::new(
                    base.clone(),
                    gateway.clone(),
                    plain_http_confirmed,
                )),
                None => Arc::new(FixedAddress(base.clone())),
            };
            // No `RequestAuth`: the probe never carries the token.
            let client = ServerClient::new(
                self.server.clone(),
                address,
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
        self.probe_expecting_with(base, expected, None, false).await
    }

    /// Like [`Prober::probe_expecting`], with the address's own gateway credentials, as [`Prober::probe_url_with`].
    ///
    /// # Errors
    /// As [`Prober::probe_expecting`].
    pub async fn probe_expecting_with(
        &self,
        base: &Url,
        expected: &ServerId,
        gateway: Option<Arc<Gateway>>,
        plain_http_confirmed: bool,
    ) -> Result<FoundServer, ProbeError> {
        let found = self
            .probe_url_with(base, gateway, plain_http_confirmed)
            .await?;
        if found.info.id == *expected {
            Ok(found)
        } else {
            Err(ProbeError::DifferentServer)
        }
    }
}

/// A probe's timeouts: 5 s to connect, 10 s per request.
pub(crate) fn probe_config() -> ClientConfig {
    ClientConfig {
        timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(5),
    }
}

/// What an address answered to the info request.
pub(crate) enum Answer {
    Found(FoundServer),
    /// The server sent the request to its web client: its base path is this one.
    WebClient(Url),
}

/// The most telling of the candidates' errors. A failed https handshake on a host and port where plain http answered does not count:
/// that port speaks plain http, so the http answer says more than the handshake.
fn most_telling(failures: &[(&Url, ProbeError)]) -> ProbeError {
    let http_answered_on = |https: &Url| {
        failures.iter().any(|(url, error)| {
            url.scheme() == "http"
                && url.host() == https.host()
                && url.port_or_known_default() == https.port_or_known_default()
                && error.http_answered()
        })
    };
    let mut best: Option<&ProbeError> = None;
    for (url, error) in failures {
        if *error == ProbeError::Tls && url.scheme() == "https" && http_answered_on(url) {
            continue;
        }
        if best.is_none_or(|best| error.rank() > best.rank()) {
            best = Some(error);
        }
    }
    best.cloned().unwrap_or(ProbeError::Unreachable)
}

/// Sends the info request on `client` and reads the answer. A gateway's answer is classified by `current-net` (ADR 0017); a redirect
/// that was not followed stays [`ProbeError::Redirected`], and Jellyfin's own redirect to its web client is recognised before an
/// HTML page counts as a sign-in page.
pub(crate) async fn ask(client: &ServerClient) -> Result<Answer, ProbeError> {
    let result = client.send(Request::get(INFO_PATH)).await;
    let gateway = GatewayAnswer::of(&result);
    let response = match result {
        Ok(response) => response,
        Err(error @ NetError::Redirected { .. }) => return Err(error.into()),
        Err(error) => return Err(gateway.map_or_else(|| error.into(), ProbeError::from)),
    };
    let answered = response.url.url();
    if let Some(address) = strip_info_path(answered) {
        if let Some(gateway) = gateway {
            return Err(gateway.into());
        }
        let info = response.json::<wire::PublicSystemInfo>()?;
        return found(info, address).map(Answer::Found);
    }
    if let Some(base) = web_client_base(answered) {
        return Ok(Answer::WebClient(base));
    }
    Err(gateway.map_or(ProbeError::NotJellyfin, ProbeError::from))
}

/// The identity check for roaming between a server's addresses (ADR 0014): the address must answer as a usable Jellyfin server with
/// the expected id. Runs on `current-net`'s probe client for the address, which carries no credentials and is reused between races.
/// It keeps the version the server reported in its last passed check, for the capabilities state; clones share it.
#[derive(Clone, Debug)]
pub struct JellyfinIdentity {
    expected: ServerId,
    version: Arc<Mutex<Option<String>>>,
}

impl JellyfinIdentity {
    pub fn new(expected: ServerId) -> Self {
        Self {
            expected,
            version: Arc::default(),
        }
    }

    /// Checks the address `client` is bound to. A server that sends the request to its web client fails: a stored address is
    /// already the base path, unlike what a user types in setup.
    pub async fn check_client(&self, client: &ServerClient) -> IdentityCheck {
        match self.probe_client(client).await {
            Ok(_) => IdentityCheck::Passed,
            Err(ProbeError::DifferentServer) => IdentityCheck::DifferentServer,
            Err(_) => IdentityCheck::Failed,
        }
    }

    /// The identity check with its full answer: the server, or why the address failed, a gateway's answer included (ADR 0017), for
    /// the 401 rule's confirming probe. `client` carries the address's gateway credentials and never the token (a roaming probe
    /// client from [`RoamingAddresses::with_gateways`](current_net::RoamingAddresses::with_gateways) does). A passed check records
    /// the version.
    ///
    /// # Errors
    /// [`ProbeError::DifferentServer`] for another server, [`ProbeError::NotJellyfin`] when the server sends the request to its web
    /// client, or any error of [`Prober::probe_url`].
    pub async fn probe_client(&self, client: &ServerClient) -> Result<FoundServer, ProbeError> {
        match ask(client).await? {
            Answer::Found(found) if found.info.id == self.expected => {
                *self.version.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(found.info.version.clone());
                Ok(found)
            }
            Answer::Found(_) => Err(ProbeError::DifferentServer),
            Answer::WebClient(_) => Err(ProbeError::NotJellyfin),
        }
    }

    /// The version the server reported in the last check that passed; `None` before one did. Recorded before the check returns,
    /// so it is there by the time the race that selects the address publishes its result.
    pub fn version(&self) -> Option<String> {
        self.version
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
        assert!(ProbeError::GatewaySignIn.rank() > ProbeError::Unavailable.rank());
        assert!(
            ProbeError::GatewayPassword { realm: None }.rank()
                > ProbeError::Redirected {
                    status: 302,
                    location: None
                }
                .rank()
        );
        assert!(
            ProbeError::TooOld {
                version: "10.11.0".into()
            }
            .rank()
                > ProbeError::GatewayRefused.rank()
        );
        assert!(!ProbeError::GatewaySignIn.is_conclusive());
        assert!(!ProbeError::Unavailable.is_conclusive());
    }

    #[test]
    fn sign_in_is_offered_for_portals_and_redirects() {
        assert!(ProbeError::GatewaySignIn.offers_sign_in());
        assert!(
            ProbeError::Redirected {
                status: 302,
                location: None
            }
            .offers_sign_in()
        );
        assert!(!ProbeError::GatewayPassword { realm: None }.offers_sign_in());
        assert!(!ProbeError::NotJellyfin.offers_sign_in());
    }

    #[test]
    fn gateway_answers_map_to_probe_errors() {
        assert_eq!(
            ProbeError::from(GatewayAnswer::Password {
                realm: Realm::new("Home")
            }),
            ProbeError::GatewayPassword {
                realm: Realm::new("Home")
            }
        );
        assert_eq!(
            ProbeError::from(GatewayAnswer::SignIn),
            ProbeError::GatewaySignIn
        );
        assert_eq!(
            ProbeError::from(GatewayAnswer::Refused),
            ProbeError::GatewayRefused
        );
        assert_eq!(
            ProbeError::from(GatewayAnswer::UnsupportedChallenge),
            ProbeError::UnsupportedChallenge
        );
        let error = ProbeError::GatewayPassword {
            realm: Realm::new("Private Realm Text"),
        };
        assert!(!format!("{error:?} {error}").contains("Private"));
    }

    #[test]
    fn plain_http_answering_on_the_port_beats_its_failed_handshake() {
        let https = url("https://192.168.1.10:3000/");
        let http = url("http://192.168.1.10:3000/");
        for answer in [
            ProbeError::NotJellyfin,
            ProbeError::Status { status: 500 },
            ProbeError::Redirected {
                status: 302,
                location: None,
            },
        ] {
            assert_eq!(
                most_telling(&[(&https, ProbeError::Tls), (&http, answer.clone())]),
                answer
            );
            // Whatever order they were tried in (http comes first on 80 and 8096).
            assert_eq!(
                most_telling(&[(&http, answer.clone()), (&https, ProbeError::Tls)]),
                answer
            );
        }
        // Nothing answered on http: the handshake still says the most.
        for silent in [ProbeError::Unreachable, ProbeError::Timeout] {
            assert_eq!(
                most_telling(&[(&https, ProbeError::Tls), (&http, silent)]),
                ProbeError::Tls
            );
        }
    }

    #[test]
    fn a_failed_handshake_on_another_port_keeps_its_rank() {
        // No port typed: https on 443 and 8920, http on 8096 and 80.
        let candidates = [
            url("https://media.example.com/"),
            url("https://media.example.com:8920/"),
            url("http://media.example.com:8096/"),
            url("http://media.example.com/"),
        ];
        let failures = [
            (&candidates[0], ProbeError::Tls),
            (&candidates[1], ProbeError::Unreachable),
            (&candidates[2], ProbeError::Unreachable),
            (&candidates[3], ProbeError::NotJellyfin),
        ];
        assert_eq!(most_telling(&failures), ProbeError::Tls);
        // Another host on the same port does not count either.
        let other = url("http://192.168.1.10:443/");
        assert_eq!(
            most_telling(&[
                (&candidates[0], ProbeError::Tls),
                (&other, ProbeError::NotJellyfin)
            ]),
            ProbeError::Tls
        );
        assert_eq!(most_telling(&[]), ProbeError::Unreachable);
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
                message: None,
                challenges: Vec::new(),
                page: false,
            }),
            ProbeError::Status { status: 403 }
        );
        assert_eq!(
            ProbeError::from(NetError::Decode { line: 1, column: 1 }),
            ProbeError::NotJellyfin
        );
    }
}
