//! Probing and signing in through authentication gateways (ADR 0017's test plan for `current-jellyfin`), against hand-written
//! stand-ins on 127.0.0.1: a Basic proxy in front of a Jellyfin stand-in (forwarding or clearing `Authorization`), a login portal,
//! a header gate, a Negotiate-only gateway and a gate that refuses `Authorization: MediaBrowser ...`. Each gateway stand-in runs the
//! Jellyfin stand-in on the request it would forward, so what Jellyfin received is checked as well.

use std::sync::{Arc, Mutex};

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, GatewayAnswer, HeaderPlacement, JellyfinClient,
    JellyfinIdentity, ProbeError, Prober, Realm, SessionEvent, TokenCheck,
};
use current_model::{ServerId, ServerKey, ServerRef, ServiceType};
use current_net::{
    AddressProvider, AuthorizationUse, BasicCredentials, ExtraHeader, FixedAddress, Gateway,
    GatewayAddress, GatewayCookie, GatewayCredentials, IdentityCheck, RequestLog, ServerClient,
    Url,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// No test here frees a port, so only `ports::bind` is used.
#[allow(dead_code)]
mod ports;

const SERVER_ID: &str = "0123456789abcdef0123456789abcdef";
const USER_ID: &str = "5d1c0000000000000000000000000001";
const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const PASSWORD: &str = "jf-pa55-word-value";
const GATE_USER: &str = "gate-user";
const GATE_PASSWORD: &str = "gate-pa55word";
/// `gate-user:gate-pa55word` in base64.
const BASIC: &str = "Z2F0ZS11c2VyOmdhdGUtcGE1NXdvcmQ=";
const REALM: &str = "Media at home";
const COOKIE_NAME: &str = "gate_session_name";
const COOKIE_VALUE: &str = "gate-cookie-value";
const HEADER_NAME: &str = "x-gate-key";
const HEADER_VALUE: &str = "gate-header-value";

/// Every secret no log line, error or Debug output may show. The realm is the server's text, kept out of them too.
const SECRETS: &[&str] = &[
    TOKEN,
    PASSWORD,
    GATE_PASSWORD,
    BASIC,
    REALM,
    COOKIE_NAME,
    COOKIE_VALUE,
    HEADER_VALUE,
];

/// One request as a stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    /// Header lines, as received.
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The request as a gateway passes it on, without `Authorization` when it clears it.
    fn forwarded(&self, clear_authorization: bool) -> Self {
        let mut forwarded = self.clone();
        if clear_authorization {
            forwarded
                .headers
                .retain(|(key, _)| !key.eq_ignore_ascii_case("authorization"));
        }
        forwarded
    }

    fn all_headers(&self) -> String {
        self.headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}\n"))
            .collect()
    }
}

type Handler = Arc<dyn Fn(&Received) -> String + Send + Sync>;

/// A server on its own port of 127.0.0.1 that answers through its handler and keeps the requests it received.
struct StandIn {
    port: u16,
    received: Arc<Mutex<Vec<Received>>>,
}

impl StandIn {
    async fn start(handler: impl Fn(&Received) -> String + Send + Sync + 'static) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = ports::bind().await;
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request.clone());
                    let _ = socket.write_all(handler(&request).as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { port, received }
    }

    fn url(&self) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}/", self.port)).unwrap()
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn count(&self, path: &str) -> usize {
        self.received().iter().filter(|r| r.path == path).count()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Received> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.split('?').next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    }
    Some(Received {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

/// An HTTP/1.1 answer with these extra header lines.
fn answer(status: u16, headers: &[String], body: &str) -> String {
    let mut text = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\n", body.len());
    for header in headers {
        text.push_str(header);
        text.push_str("\r\n");
    }
    text.push_str("connection: close\r\n\r\n");
    text.push_str(body);
    text
}

fn json(status: u16, body: &str) -> String {
    answer(status, &["content-type: application/json".to_owned()], body)
}

fn html(status: u16) -> String {
    answer(
        status,
        &["content-type: text/html".to_owned()],
        "<!DOCTYPE html><html><body>Sign in</body></html>",
    )
}

fn challenge(scheme: &str) -> String {
    answer(401, &[format!("www-authenticate: {scheme}")], "")
}

fn basic_challenge() -> String {
    challenge(&format!(r#"Basic realm="{REALM}""#))
}

/// What Jellyfin 12.2 reads its client fields from (`AuthorizationContext.GetAuthorizationDictionary`, `docs/notes/gateway-auth.md`):
/// `Authorization` with the `MediaBrowser` scheme, or `X-Emby-Authorization` only with legacy authorization on and `Authorization`
/// empty.
fn jellyfin_fields(request: &Received, legacy: bool) -> Option<String> {
    let media_browser = |value: &str| {
        value
            .to_ascii_lowercase()
            .starts_with("mediabrowser ")
            .then(|| value.to_owned())
    };
    match request.header("authorization") {
        Some(value) => media_browser(value),
        None if legacy => request
            .header("x-emby-authorization")
            .and_then(media_browser),
        None => None,
    }
}

/// A Jellyfin 12.2 stand-in: the public info, sign-in (400 without the four fields, ADR 0015), Quick Connect's Initiate and the
/// current user. Its 401 carries no challenge, as Jellyfin's does not.
fn jellyfin(request: &Received, legacy: bool) -> String {
    let fields = jellyfin_fields(request, legacy);
    let four = fields.as_deref().is_some_and(|fields| {
        ["Client=", "Device=", "DeviceId=", "Version="]
            .iter()
            .all(|key| fields.contains(key))
    });
    let token = fields
        .as_deref()
        .is_some_and(|fields| fields.contains(&format!("Token=\"{TOKEN}\"")));
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/System/Info/Public") => json(
            200,
            &format!(
                r#"{{"ServerName":"Example","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{SERVER_ID}","StartupWizardCompleted":true}}"#
            ),
        ),
        ("POST", "/Users/AuthenticateByName" | "/QuickConnect/Initiate") if !four => {
            json(400, r#""Error processing request.""#)
        }
        ("POST", "/Users/AuthenticateByName") if request.body.contains(PASSWORD) => {
            json(200, &auth_result())
        }
        ("POST", "/Users/AuthenticateByName") => json(401, ""),
        ("POST", "/QuickConnect/Initiate") => json(
            200,
            r#"{"Authenticated":false,"Secret":"5ecre7","Code":"123456"}"#,
        ),
        ("GET", "/Users/Me") if token => json(200, &user_json()),
        ("GET", "/Users/Me") => json(401, ""),
        _ => json(404, ""),
    }
}

fn user_json() -> String {
    format!(r#"{{"Name":"Example User","ServerId":"{SERVER_ID}","Id":"{USER_ID}"}}"#)
}

fn auth_result() -> String {
    format!(
        r#"{{"User":{},"AccessToken":"{TOKEN}","ServerId":"{SERVER_ID}"}}"#,
        user_json()
    )
}

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

fn identity() -> ClientIdentity {
    ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef")
}

fn basic(password: &str) -> GatewayCredentials {
    GatewayCredentials::new(
        Some(BasicCredentials::new(GATE_USER, password).unwrap()),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn session_cookie() -> GatewayCredentials {
    GatewayCredentials::new(
        None,
        Vec::new(),
        vec![GatewayCookie::new(COOKIE_NAME, COOKIE_VALUE, "127.0.0.1").unwrap()],
    )
    .unwrap()
}

fn has_cookie(request: &Received) -> bool {
    request
        .header("cookie")
        .is_some_and(|cookie| cookie.contains(&format!("{COOKIE_NAME}={COOKIE_VALUE}")))
}

fn has_basic(request: &Received) -> bool {
    request.header("authorization") == Some(&format!("Basic {BASIC}"))
}

fn through(stand_in: &StandIn, credentials: GatewayCredentials) -> Arc<dyn AddressProvider> {
    Arc::new(GatewayAddress::new(
        stand_in.url(),
        Arc::new(Gateway::new(credentials)),
        false,
    ))
}

fn client(address: Arc<dyn AddressProvider>, log: &Arc<RequestLog>) -> JellyfinClient {
    JellyfinClient::new(server(), address, identity(), log.clone()).unwrap()
}

/// No secret in `text`.
fn assert_clean(text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

/// A Basic proxy in front of Jellyfin: 401 with a Basic challenge unless the password arrives; then it passes the request on,
/// with its `Authorization` header or without it ("Pass Auth to Upstream" off).
async fn basic_proxy(
    clear_authorization: bool,
    legacy: bool,
) -> (StandIn, Arc<Mutex<Vec<Received>>>) {
    let upstream = Arc::new(Mutex::new(Vec::new()));
    let seen = upstream.clone();
    let proxy = StandIn::start(move |request| {
        if !has_basic(request) {
            return basic_challenge();
        }
        let forwarded = request.forwarded(clear_authorization);
        seen.lock().unwrap().push(forwarded.clone());
        jellyfin(&forwarded, legacy)
    })
    .await;
    (proxy, upstream)
}

#[tokio::test]
async fn a_basic_proxy_asks_for_a_password_with_its_realm() {
    let (proxy, _) = basic_proxy(true, true).await;
    let log = Arc::new(RequestLog::default());
    let prober = Prober::new(server(), log.clone());

    let without = prober.probe_url(&proxy.url()).await;
    assert_eq!(
        without,
        Err(ProbeError::GatewayPassword {
            realm: Realm::new(REALM)
        })
    );
    let Err(ProbeError::GatewayPassword { realm: Some(realm) }) = &without else {
        panic!("{without:?}");
    };
    assert_eq!(realm.as_str(), REALM);

    // A wrong password asks again.
    let wrong = prober
        .probe_url_with(
            &proxy.url(),
            Some(Arc::new(Gateway::new(basic("wrong")))),
            false,
        )
        .await;
    assert!(
        matches!(wrong, Err(ProbeError::GatewayPassword { .. })),
        "{wrong:?}"
    );

    let found = prober
        .probe_url_with(
            &proxy.url(),
            Some(Arc::new(Gateway::new(basic(GATE_PASSWORD)))),
            false,
        )
        .await
        .unwrap();
    assert_eq!(found.info.id, ServerId::new(SERVER_ID));
    let found = prober
        .probe_expecting_with(
            &proxy.url(),
            &ServerId::new(SERVER_ID),
            Some(Arc::new(Gateway::new(basic(GATE_PASSWORD)))),
            false,
        )
        .await;
    assert!(found.is_ok(), "{found:?}");

    let errors = format!("{without:?} {} {wrong:?}", without.as_ref().unwrap_err());
    assert_clean(&format!("{errors}\n{}", log.to_text()));
}

#[tokio::test]
async fn behind_a_basic_proxy_that_clears_its_header_jellyfin_reads_x_emby_authorization() {
    let (proxy, upstream) = basic_proxy(true, true).await;
    let log = Arc::new(RequestLog::default());
    let credentials = basic(GATE_PASSWORD);
    let client = client(through(&proxy, credentials.clone()), &log);
    client.set_header_placement(HeaderPlacement::for_gateway(
        &credentials,
        HeaderPlacement::default(),
    ));
    assert_eq!(
        client.header_placement(),
        HeaderPlacement::EmbyAuthorization
    );

    let signed_in = client
        .sign_in_by_name("Example User", PASSWORD)
        .await
        .unwrap();
    assert_eq!(signed_in.token, AccessToken::new(TOKEN));
    // The gateway got its password in `Authorization`, Jellyfin its fields in `X-Emby-Authorization`.
    let sign_in = &proxy.received()[0];
    assert!(has_basic(sign_in));
    let forwarded = upstream.lock().unwrap()[0].clone();
    assert_eq!(forwarded.header("authorization"), None);
    assert!(
        forwarded
            .header("x-emby-authorization")
            .is_some_and(|value| value.starts_with("MediaBrowser Client=\"Current\""))
    );

    client.set_token(signed_in.token);
    assert!(client.current_user().await.is_ok());
    assert_clean(&format!("{}\n{client:?}", log.to_text()));
}

#[tokio::test]
async fn a_server_that_ignores_x_emby_authorization_gets_the_400_diagnosis() {
    // The proxy passes its own `Authorization` on: Jellyfin reads it and finds no fields, whatever the legacy setting.
    let (forwarding, _) = basic_proxy(false, true).await;
    // The proxy clears it, but legacy authorization is off: Jellyfin never reads `X-Emby-Authorization`.
    let (legacy_off, _) = basic_proxy(true, false).await;
    for proxy in [forwarding, legacy_off] {
        let log = Arc::new(RequestLog::default());
        let client = client(through(&proxy, basic(GATE_PASSWORD)), &log);
        client.set_header_placement(HeaderPlacement::EmbyAuthorization);
        let error = client
            .sign_in_by_name("Example User", PASSWORD)
            .await
            .unwrap_err();
        assert_eq!(error, AuthError::EmbyHeaderIgnored);
        let text = error.to_string();
        assert!(text.contains("legacy authorization"), "{text}");
        assert!(text.contains("let the server's API through"), "{text}");
        assert_eq!(
            client.quick_connect_initiate().await.unwrap_err(),
            AuthError::EmbyHeaderIgnored
        );
        assert_clean(&format!("{error:?} {error}\n{}", log.to_text()));
    }
}

#[tokio::test]
async fn a_basic_address_left_on_authorization_moves_after_the_challenge() {
    // Jellyfin's `Authorization` keeps the gateway's Basic off the request: the proxy challenges, the bare probe (which carries
    // the Basic) passes, so the sign-in moves to `X-Emby-Authorization` and is sent once more.
    let (proxy, _) = basic_proxy(true, true).await;
    let log = Arc::new(RequestLog::default());
    let client = client(through(&proxy, basic(GATE_PASSWORD)), &log);
    let signed_in = client.sign_in_by_name("Example User", PASSWORD).await;
    assert!(signed_in.is_ok(), "{signed_in:?}");
    assert_eq!(
        client.header_placement(),
        HeaderPlacement::EmbyAuthorization
    );
    assert_eq!(proxy.count("/Users/AuthenticateByName"), 2);
}

/// A gate like some portal gateways' default endpoints: it refuses a request whose `Authorization` is `MediaBrowser ...` with a
/// challenge, even with its session cookie, and passes the rest on with the cookie.
async fn media_browser_gate(legacy: bool) -> StandIn {
    StandIn::start(move |request| {
        let media_browser = request
            .header("authorization")
            .is_some_and(|value| value.starts_with("MediaBrowser"));
        if media_browser {
            challenge(r#"Basic realm="Authorization Required""#)
        } else if has_cookie(request) {
            jellyfin(request, legacy)
        } else {
            answer(
                401,
                &["location: http://127.0.0.1:1/login".to_owned()],
                r#"{"status":"Unauthorized"}"#,
            )
        }
    })
    .await
}

#[tokio::test]
async fn a_gate_refusing_jellyfins_header_gets_one_retry_with_x_emby_authorization() {
    let gate = media_browser_gate(true).await;
    let log = Arc::new(RequestLog::default());
    let first = client(through(&gate, session_cookie()), &log);

    let signed_in = first.sign_in_by_name("Example User", PASSWORD).await;
    assert!(signed_in.is_ok(), "{signed_in:?}");
    assert_eq!(first.header_placement(), HeaderPlacement::EmbyAuthorization);
    let paths: Vec<String> = gate.received().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [
            "/Users/AuthenticateByName",
            "/System/Info/Public",
            "/Users/AuthenticateByName"
        ]
    );
    let sign_ins: Vec<Received> = gate
        .received()
        .into_iter()
        .filter(|r| r.path == "/Users/AuthenticateByName")
        .collect();
    assert!(sign_ins[0].header("authorization").is_some());
    assert!(sign_ins[1].header("authorization").is_none());
    assert!(sign_ins[1].header("x-emby-authorization").is_some());

    // Quick Connect's Initiate, from a client that already holds a token: moves the same way, and the probe in between carries the
    // address's cookie but never Jellyfin's header or token.
    let other = client(through(&gate, session_cookie()), &log);
    other.set_token(AccessToken::new(TOKEN));
    let code = other.quick_connect_initiate().await;
    assert!(code.is_ok(), "{code:?}");
    assert_eq!(other.header_placement(), HeaderPlacement::EmbyAuthorization);
    assert_eq!(gate.count("/QuickConnect/Initiate"), 2);
    let probes: Vec<Received> = gate
        .received()
        .into_iter()
        .filter(|r| r.path == "/System/Info/Public")
        .collect();
    assert_eq!(probes.len(), 2);
    for probe in probes {
        assert!(has_cookie(&probe));
        let headers = probe.all_headers();
        assert!(!headers.contains("MediaBrowser"), "{headers}");
        assert!(!headers.contains(TOKEN), "{headers}");
    }
    assert_clean(&format!("{}\n{first:?} {other:?}", log.to_text()));
}

#[tokio::test]
async fn the_retry_goes_back_when_the_server_ignores_the_other_header() {
    let gate = media_browser_gate(false).await;
    let log = Arc::new(RequestLog::default());
    let client = client(through(&gate, session_cookie()), &log);
    assert_eq!(
        client.sign_in_by_name("Example User", PASSWORD).await,
        Err(AuthError::EmbyHeaderIgnored)
    );
    assert_eq!(gate.count("/Users/AuthenticateByName"), 2);
    // Back on the default, so a later fix on the gateway's side works without a setting to undo.
    assert_eq!(client.header_placement(), HeaderPlacement::Authorization);
}

#[tokio::test]
async fn no_retry_when_the_bare_probe_does_not_pass() {
    // A Negotiate-only gateway challenges every request, the bare probe included: nothing proves the header was the problem.
    let gate = StandIn::start(|_| challenge("Negotiate")).await;
    let log = Arc::new(RequestLog::default());
    let client = client(Arc::new(FixedAddress(gate.url())), &log);
    assert_eq!(
        client.sign_in_by_name("Example User", PASSWORD).await,
        Err(AuthError::Gateway(GatewayAnswer::UnsupportedChallenge))
    );
    assert_eq!(gate.count("/Users/AuthenticateByName"), 1);
    assert_eq!(client.header_placement(), HeaderPlacement::Authorization);
    let probe = Prober::new(server(), log).probe_url(&gate.url()).await;
    assert_eq!(probe, Err(ProbeError::UnsupportedChallenge));
}

#[tokio::test]
async fn a_login_portal_asks_for_a_web_sign_in() {
    let portal = StandIn::start(|_| html(200)).await;
    let login = format!("location: http://127.0.0.1:{}/login", portal.port);
    let redirecting = StandIn::start(move |request| {
        if has_cookie(request) {
            jellyfin(request, false)
        } else {
            answer(302, std::slice::from_ref(&login), "")
        }
    })
    .await;
    let json_401 = StandIn::start(|request| {
        if has_cookie(request) {
            jellyfin(request, false)
        } else {
            answer(
                401,
                &[
                    "location: http://127.0.0.1:1/login".to_owned(),
                    "content-type: application/json".to_owned(),
                ],
                r#"{"status":"Unauthorized"}"#,
            )
        }
    })
    .await;
    let html_403 = StandIn::start(|request| {
        if has_cookie(request) {
            jellyfin(request, false)
        } else {
            html(403)
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let prober = Prober::new(server(), log.clone());

    // A redirect to another port stays ambiguous: setup offers "Sign in" and the address it points to.
    let redirected = prober.probe_url(&redirecting.url()).await.unwrap_err();
    assert!(
        matches!(
            redirected,
            ProbeError::Redirected {
                status: 302,
                location: Some(_)
            }
        ),
        "{redirected:?}"
    );
    assert!(redirected.offers_sign_in());

    for gate in [&json_401, &html_403] {
        let error = prober.probe_url(&gate.url()).await.unwrap_err();
        assert_eq!(error, ProbeError::GatewaySignIn);
        assert!(error.offers_sign_in());
    }
    // With the portal's cookie, every probe passes.
    for gate in [&redirecting, &json_401, &html_403] {
        let found = prober
            .probe_url_with(
                &gate.url(),
                Some(Arc::new(Gateway::new(session_cookie()))),
                false,
            )
            .await;
        assert!(found.is_ok(), "{found:?}");
    }
    // The portal itself was never asked: the redirect was not followed.
    assert!(portal.received().is_empty());
    assert_clean(&log.to_text());
}

#[tokio::test]
async fn a_header_gate_refuses_until_its_header_arrives() {
    let gate = StandIn::start(|request| {
        if request.header(HEADER_NAME) == Some(HEADER_VALUE) {
            jellyfin(request, false)
        } else {
            answer(403, &[], "")
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let prober = Prober::new(server(), log.clone());
    assert_eq!(
        prober.probe_url(&gate.url()).await,
        Err(ProbeError::GatewayRefused)
    );
    let credentials = GatewayCredentials::new(
        None,
        vec![ExtraHeader::new(HEADER_NAME, HEADER_VALUE, AuthorizationUse::Service).unwrap()],
        Vec::new(),
    )
    .unwrap();
    let found = prober
        .probe_url_with(
            &gate.url(),
            Some(Arc::new(Gateway::new(credentials))),
            false,
        )
        .await;
    assert!(found.is_ok(), "{found:?}");
    assert_clean(&log.to_text());
}

#[tokio::test]
async fn the_identity_probe_classifies_and_carries_the_gateway_only() {
    let (proxy, _) = basic_proxy(true, true).await;
    let log = Arc::new(RequestLog::default());
    let identity = JellyfinIdentity::new(ServerId::new(SERVER_ID));
    let probe_client = |credentials: Option<GatewayCredentials>| {
        let address: Arc<dyn AddressProvider> = match credentials {
            Some(credentials) => through(&proxy, credentials),
            None => Arc::new(FixedAddress(proxy.url())),
        };
        ServerClient::new(server(), address, None, log.clone(), &Default::default()).unwrap()
    };

    let bare = probe_client(None);
    assert!(matches!(
        identity.probe_client(&bare).await,
        Err(ProbeError::GatewayPassword { .. })
    ));
    assert_eq!(identity.check_client(&bare).await, IdentityCheck::Failed);

    let with = probe_client(Some(basic(GATE_PASSWORD)));
    assert!(identity.probe_client(&with).await.is_ok());
    assert_eq!(identity.check_client(&with).await, IdentityCheck::Passed);
    assert_eq!(identity.version().as_deref(), Some("12.2.0"));

    let other = JellyfinIdentity::new(ServerId::new("another"));
    assert_eq!(
        other.probe_client(&with).await.unwrap_err(),
        ProbeError::DifferentServer
    );
    for request in proxy.received() {
        assert!(!request.all_headers().contains("MediaBrowser"));
    }
}

#[tokio::test]
async fn a_gateway_challenge_mid_session_keeps_the_token() {
    // A Basic gateway whose password stopped working answers every request with its challenge.
    let expired = StandIn::start(|_| basic_challenge()).await;
    let log = Arc::new(RequestLog::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let gated = client(Arc::new(FixedAddress(expired.url())), &log);
    gated.set_token(AccessToken::new(TOKEN));
    let heard = events.clone();
    gated.on_event(Arc::new(move |event| heard.lock().unwrap().push(event)));
    let error = gated.current_user().await.unwrap_err();
    assert!(
        matches!(error, AuthError::Gateway(GatewayAnswer::Password { .. })),
        "{error:?}"
    );
    assert!(gated.has_token());
    // The hook hears the challenge; nothing waits for a check.
    assert!(matches!(
        events.lock().unwrap().as_slice(),
        [SessionEvent::Gateway(GatewayAnswer::Password { .. })]
    ));
    assert!(matches!(
        gated.current_user().await,
        Err(AuthError::Gateway(_))
    ));
    // A sign-in's challenge is the gateway's too, not a wrong password.
    let sign_in = gated
        .sign_in_by_name("Example User", PASSWORD)
        .await
        .unwrap_err();
    assert!(
        matches!(sign_in, AuthError::Gateway(GatewayAnswer::Password { .. })),
        "{sign_in:?}"
    );
    assert_clean(&format!(
        "{error:?} {error} {sign_in:?} {sign_in}\n{}",
        log.to_text()
    ));

    // Jellyfin's own 401, without a challenge, still ends the session.
    let direct = StandIn::start(|request| jellyfin(request, false)).await;
    let session = client(Arc::new(FixedAddress(direct.url())), &log);
    session.set_token(AccessToken::new("not-the-token"));
    assert_eq!(
        session.current_user().await.unwrap_err(),
        AuthError::SessionRevoked
    );
    assert!(!session.has_token());
}

#[tokio::test]
async fn the_token_check_decides_nothing_on_a_gateway_s_answer() {
    let log = Arc::new(RequestLog::default());
    // A gateway's challenge, and a portal's sign-in page with 401: neither is Jellyfin's, so the session stays.
    for answer in [basic_challenge(), html(401)] {
        let gated = StandIn::start(move |_| answer.clone()).await;
        let session = client(Arc::new(FixedAddress(gated.url())), &log);
        session.set_token(AccessToken::new(TOKEN));
        assert_eq!(
            session.check_token(&AccessToken::new(TOKEN)).await,
            TokenCheck::Unknown
        );
        assert!(session.has_token());
        assert_eq!(gated.count("/Users/Me"), 1);
    }
    assert_clean(&log.to_text());
}
