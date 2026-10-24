//! Gateway credentials and answer shapes (ADR 0017's test plan for `current-net`) against hand-written stand-ins on 127.0.0.1: a
//! Basic proxy, a login portal, a header gate, and a second address standing in for the LAN address.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_model::{ServerAddress, ServerKey, ServerRef, ServiceType};
use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{
    AddressProvider, AuthorizationUse, BasicCredentials, ClientConfig, ExtraHeader, FixedAddress,
    Gateway, GatewayAddress, GatewayCookie, GatewayCredentials, GatewayError, GatewayShape,
    IdentityCheck, IdentityProbe, NetError, Request, RequestAuth, RequestLog, Response,
    RoamingAddresses, RoamingConfig, RuntimeHandle, ServerClient, Url, classify,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// No test here frees a port, so only `ports::bind` is used.
#[allow(dead_code)]
mod ports;

const TOKEN: &str = "s3cret-token-value";
const SERVER_ID: &str = "0123456789abcdef0123456789abcdef";
const USER: &str = "gate-user";
const PASSWORD: &str = "gate-pa55word";
/// `gate-user:gate-pa55word` in base64.
const BASIC: &str = "Z2F0ZS11c2VyOmdhdGUtcGE1NXdvcmQ=";
const COOKIE_NAME: &str = "gate_session_name";
const COOKIE_VALUE: &str = "gate-cookie-value";
const ROTATED_VALUE: &str = "gate-cookie-rotated";
const HEADER_NAME: &str = "x-gate-key";
const HEADER_VALUE: &str = "gate-header-value";

/// Every secret no log line, error or Debug output may show.
const SECRETS: &[&str] = &[
    TOKEN,
    PASSWORD,
    BASIC,
    COOKIE_NAME,
    COOKIE_VALUE,
    ROTATED_VALUE,
    HEADER_VALUE,
];

/// Builds the answer to a request from its head (request line and headers, as received).
type Handler = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// A server on its own port of 127.0.0.1 that answers through its handler and keeps the raw requests it received.
struct StandIn {
    port: u16,
    received: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    async fn start(handler: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = ports::bind().await;
        let port = ok(listener.local_addr()).port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let head = read_head(&mut socket).await;
                    if let Ok(mut log) = log.lock() {
                        log.push(head.clone());
                    }
                    let _ = socket.write_all(handler(&head).as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { port, received }
    }

    fn url(&self) -> Url {
        ok(Url::parse(&format!("http://127.0.0.1:{}/", self.port)))
    }

    fn received(&self) -> Vec<String> {
        self.received.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

async fn read_head(socket: &mut tokio::net::TcpStream) -> String {
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&buffer[..read]),
        }
    }
    String::from_utf8_lossy(&head).into_owned()
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

fn json(body: &str) -> String {
    answer(200, &["content-type: application/json".to_owned()], body)
}

fn info() -> String {
    json(&format!(r#"{{"Id":"{SERVER_ID}"}}"#))
}

/// The value of header `name` in a request head, if present (names compared without case).
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{error:?}"),
    }
}

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

fn config() -> ClientConfig {
    ClientConfig {
        timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(5),
    }
}

/// A service token as a header, the way Jellyfin's will be.
struct TokenAuth;

impl RequestAuth for TokenAuth {
    fn apply(&self, headers: &mut HeaderMap, _query: &mut Vec<(String, String)>) {
        headers.insert(
            HeaderName::from_static("x-test-token"),
            HeaderValue::from_static(TOKEN),
        );
    }
}

fn client(
    address: impl AddressProvider + 'static,
    auth: bool,
    log: &Arc<RequestLog>,
) -> ServerClient {
    let auth: Option<Arc<dyn RequestAuth>> = if auth {
        Some(Arc::new(TokenAuth))
    } else {
        None
    };
    ok(ServerClient::new(
        server(),
        Arc::new(address),
        auth,
        log.clone(),
        &config(),
    ))
}

fn basic(password: &str) -> GatewayCredentials {
    ok(GatewayCredentials::new(
        Some(ok(BasicCredentials::new(USER, password))),
        Vec::new(),
        Vec::new(),
    ))
}

fn cookies(cookies: Vec<GatewayCookie>) -> GatewayCredentials {
    ok(GatewayCredentials::new(None, Vec::new(), cookies))
}

fn session_cookie() -> GatewayCookie {
    ok(GatewayCookie::new(COOKIE_NAME, COOKIE_VALUE, "127.0.0.1"))
}

fn everything() -> GatewayCredentials {
    ok(GatewayCredentials::new(
        Some(ok(BasicCredentials::new(USER, PASSWORD))),
        vec![ok(ExtraHeader::new(
            HEADER_NAME,
            HEADER_VALUE,
            AuthorizationUse::Service,
        ))],
        vec![session_cookie()],
    ))
}

/// No secret anywhere in `text`.
fn assert_clean(text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

/// The result's error, Debug and Display, for [`assert_clean`].
fn shown(result: &Result<Response, NetError>) -> String {
    match result {
        Ok(response) => format!("{response:?}"),
        Err(error) => format!("{error:?} {error}"),
    }
}

/// A Basic proxy: 401 with a Basic challenge unless the right password arrives.
async fn basic_proxy() -> StandIn {
    StandIn::start(|head| {
        if header(head, "authorization") == Some(&format!("Basic {BASIC}")) {
            info()
        } else {
            answer(
                401,
                &[r#"www-authenticate: Basic realm="Media at home""#.to_owned()],
                "",
            )
        }
    })
    .await
}

#[tokio::test]
async fn a_basic_proxy_asks_for_a_password_and_takes_it() {
    let proxy = basic_proxy().await;
    let log = Arc::new(RequestLog::default());

    let without = client(FixedAddress(proxy.url()), false, &log)
        .send(Request::get("/info"))
        .await;
    assert_eq!(
        classify(&without),
        GatewayShape::Password {
            realm: Some("Media at home".to_owned())
        }
    );

    let wrong = Arc::new(Gateway::new(basic("wrong-password")));
    let wrong = client(GatewayAddress::new(proxy.url(), wrong, false), false, &log)
        .send(Request::get("/info"))
        .await;
    assert!(matches!(classify(&wrong), GatewayShape::Password { .. }));

    let gateway = Arc::new(Gateway::new(basic(PASSWORD)));
    let with = client(
        GatewayAddress::new(proxy.url(), gateway, false),
        false,
        &log,
    )
    .send(Request::get("/info"))
    .await;
    assert_eq!(classify(&with), GatewayShape::Expected);

    for result in [&without, &wrong, &with] {
        assert_clean(&shown(result));
    }
    assert_clean(&log.to_text());
}

#[tokio::test]
async fn the_service_authorization_header_is_not_replaced_by_basic() {
    struct MediaBrowser;
    impl RequestAuth for MediaBrowser {
        fn apply(&self, headers: &mut HeaderMap, _query: &mut Vec<(String, String)>) {
            headers.insert(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("MediaBrowser Client=\"x\""),
            );
        }
    }
    let proxy = basic_proxy().await;
    let gateway = Arc::new(Gateway::new(basic(PASSWORD)));
    let client = ok(ServerClient::new(
        server(),
        Arc::new(GatewayAddress::new(proxy.url(), gateway, false)),
        Some(Arc::new(MediaBrowser)),
        Arc::new(RequestLog::default()),
        &config(),
    ));
    // The gateway answers with its challenge again: never a session end (ADR 0017's 401 rule), and placement is current-jellyfin's.
    let result = client.send(Request::get("/info")).await;
    assert!(matches!(classify(&result), GatewayShape::Password { .. }));
    assert_eq!(
        header(&proxy.received()[0], "authorization"),
        Some("MediaBrowser Client=\"x\"")
    );
}

/// How a login portal's gateway answers a request without its cookie.
#[derive(Clone, Copy)]
enum Portal {
    /// 302 to the portal on another port.
    Redirect,
    /// 401 with a JSON body and a `Location`.
    JsonUnauthorized,
    /// 403 with an HTML page.
    HtmlForbidden,
    /// 200 with an HTML sign-in page at the end of a same-host redirect.
    FollowedToPage,
}

/// A gateway that passes requests carrying its cookie and answers the others as `mode` says; with the cookie it rotates it.
async fn portal_gateway(mode: Portal, portal: u16) -> StandIn {
    StandIn::start(move |head| {
        let cookie = header(head, "cookie").unwrap_or("");
        let signed_in = cookie.contains(&format!("{COOKIE_NAME}={COOKIE_VALUE}"))
            || cookie.contains(&format!("{COOKIE_NAME}={ROTATED_VALUE}"));
        if signed_in {
            return answer(
                200,
                &[
                    "content-type: application/json".to_owned(),
                    format!(
                        "set-cookie: {COOKIE_NAME}={ROTATED_VALUE}; Path=/; HttpOnly; Max-Age=3600"
                    ),
                    "set-cookie: tracker_new=1; Path=/".to_owned(),
                ],
                &format!(r#"{{"Id":"{SERVER_ID}"}}"#),
            );
        }
        let location = format!("location: http://127.0.0.1:{portal}/login?rd=x");
        match mode {
            Portal::Redirect => answer(302, &[location], ""),
            Portal::JsonUnauthorized => answer(
                401,
                &[location, "content-type: application/json".to_owned()],
                r#"{"status":"Unauthorized"}"#,
            ),
            Portal::HtmlForbidden => answer(
                403,
                &["content-type: text/html".to_owned()],
                "<html><a href=\"/oauth2/start\">Sign in</a></html>",
            ),
            Portal::FollowedToPage if head.starts_with("GET /info ") => {
                answer(302, &["location: /signin".to_owned()], "")
            }
            Portal::FollowedToPage => answer(
                200,
                &["content-type: text/html; charset=utf-8".to_owned()],
                "<!doctype html><form></form>",
            ),
        }
    })
    .await
}

#[tokio::test]
async fn a_login_portal_needs_a_web_sign_in_and_its_cookie_passes() {
    let portal = StandIn::start(|_| answer(200, &[], "<html>login</html>")).await;
    for mode in [
        Portal::Redirect,
        Portal::JsonUnauthorized,
        Portal::HtmlForbidden,
        Portal::FollowedToPage,
    ] {
        let gateway_server = portal_gateway(mode, portal.port).await;
        let log = Arc::new(RequestLog::default());
        let without = client(FixedAddress(gateway_server.url()), false, &log)
            .send(Request::get("/info"))
            .await;
        assert_eq!(classify(&without), GatewayShape::SignIn);
        if let (Portal::FollowedToPage, Ok(response)) = (mode, &without) {
            assert!(response.redirected);
        }

        let gateway = Arc::new(Gateway::new(cookies(vec![session_cookie()])));
        let with = client(
            GatewayAddress::new(gateway_server.url(), gateway.clone(), false),
            false,
            &log,
        );
        let first = with.send(Request::get("/info")).await;
        assert_eq!(classify(&first), GatewayShape::Expected);
        // The gateway rotated its cookie: the next request carries the new value, and a new cookie name was not taken.
        assert_eq!(gateway.changes(), 1);
        let second = with.send(Request::get("/info")).await;
        assert_eq!(classify(&second), GatewayShape::Expected);
        let received = gateway_server.received();
        assert_eq!(
            header(&received[received.len() - 1], "cookie"),
            Some(format!("{COOKIE_NAME}={ROTATED_VALUE}").as_str())
        );
        let snapshot = gateway.snapshot();
        assert_eq!(snapshot.cookies().len(), 1);
        assert_eq!(snapshot.cookies()[0].value(), ROTATED_VALUE);

        for result in [&without, &first, &second] {
            assert_clean(&shown(result));
        }
        assert_clean(&log.to_text());
        assert_clean(&format!("{gateway:?} {snapshot:?}"));
    }
    // The portal itself is never contacted: the redirect to another port is not followed.
    assert!(portal.received().is_empty());
}

#[tokio::test]
async fn a_header_gate_refuses_without_its_header() {
    let gate = StandIn::start(|head| {
        if header(head, HEADER_NAME) == Some(HEADER_VALUE) {
            info()
        } else {
            answer(403, &[], "")
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let without = client(FixedAddress(gate.url()), false, &log)
        .send(Request::get("/info"))
        .await;
    assert_eq!(classify(&without), GatewayShape::Refused);

    let credentials = ok(GatewayCredentials::new(
        None,
        vec![ok(ExtraHeader::new(
            HEADER_NAME,
            HEADER_VALUE,
            AuthorizationUse::Service,
        ))],
        Vec::new(),
    ));
    let gateway = Arc::new(Gateway::new(credentials));
    let with = client(GatewayAddress::new(gate.url(), gateway, false), true, &log)
        .send(Request::get("/info"))
        .await;
    assert_eq!(classify(&with), GatewayShape::Expected);

    for name in ["Cookie", "Host", "Content-Length", "Transfer-Encoding"] {
        assert_eq!(
            ExtraHeader::new(name, HEADER_VALUE, AuthorizationUse::Free).map(|_| ()),
            Err(GatewayError::ReservedHeader)
        );
    }
    assert_eq!(
        ExtraHeader::new("Authorization", HEADER_VALUE, AuthorizationUse::Service).map(|_| ()),
        Err(GatewayError::ReservedHeader)
    );
    assert_clean(&format!(
        "{:?}",
        ExtraHeader::new("x", "\n", AuthorizationUse::Free)
    ));
    assert_clean(&log.to_text());
}

#[tokio::test]
async fn another_scheme_only_is_unsupported() {
    let negotiate = StandIn::start(|_| {
        answer(
            401,
            &[
                "www-authenticate: Negotiate".to_owned(),
                "www-authenticate: NTLM".to_owned(),
            ],
            "",
        )
    })
    .await;
    let result = client(
        FixedAddress(negotiate.url()),
        false,
        &Arc::new(RequestLog::default()),
    )
    .send(Request::get("/info"))
    .await;
    assert_eq!(classify(&result), GatewayShape::Unsupported);
}

/// A server that answers its identity and 200 to anything else.
async fn plain_server() -> StandIn {
    StandIn::start(|head| {
        if head.starts_with("GET /info ") {
            info()
        } else {
            json("{}")
        }
    })
    .await
}

/// An address as the user's list holds it (plain HTTP to 127.0.0.1 needs no confirmation).
fn address(stand_in: &StandIn) -> ServerAddress {
    ServerAddress {
        url: stand_in.url().to_string(),
        plain_http_confirmed: false,
    }
}

/// The stand-ins' identity check: `GET /info` must answer `SERVER_ID`.
struct TestIdentity;

impl IdentityProbe for TestIdentity {
    fn check<'a>(
        &'a self,
        client: &'a ServerClient,
    ) -> Pin<Box<dyn Future<Output = IdentityCheck> + Send + 'a>> {
        Box::pin(async move {
            match client
                .send_json::<serde_json::Value>(Request::get("/info"))
                .await
            {
                Ok(info) if info["Id"] == SERVER_ID => IdentityCheck::Passed,
                Ok(_) => IdentityCheck::DifferentServer,
                Err(_) => IdentityCheck::Failed,
            }
        })
    }
}

fn roaming(
    addresses: &[(ServerAddress, Option<Arc<Gateway>>)],
    log: &Arc<RequestLog>,
) -> Arc<RoamingAddresses> {
    ok(RoamingAddresses::with_gateways(
        server(),
        addresses,
        Arc::new(TestIdentity),
        log.clone(),
        RoamingConfig {
            stagger: Duration::from_millis(100),
            probe_timeout: Duration::from_secs(2),
            ..RoamingConfig::default()
        },
        &RuntimeHandle::current(),
    ))
}

/// Gateway credentials of any kind in a request head.
fn carries_gateway(head: &str) -> bool {
    header(head, "cookie").is_some()
        || header(head, "authorization").is_some()
        || header(head, HEADER_NAME).is_some()
}

#[tokio::test]
async fn the_probe_carries_gateway_credentials_but_never_the_token() {
    let gated = plain_server().await;
    let log = Arc::new(RequestLog::default());
    let gateway = Arc::new(Gateway::new(everything()));
    let roaming = roaming(&[(address(&gated), Some(gateway))], &log);
    let client = ok(ServerClient::new(
        server(),
        roaming.clone(),
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config(),
    ));
    ok(client.send(Request::get("/Items")).await);
    let received = gated.received();
    let (probe, request) = (&received[0], &received[received.len() - 1]);
    assert!(probe.starts_with("GET /info "), "{probe}");
    assert_eq!(
        header(probe, "authorization"),
        Some(format!("Basic {BASIC}").as_str())
    );
    assert_eq!(header(probe, HEADER_NAME), Some(HEADER_VALUE));
    assert_eq!(
        header(probe, "cookie"),
        Some(format!("{COOKIE_NAME}={COOKIE_VALUE}").as_str())
    );
    assert!(!probe.contains(TOKEN), "{probe}");
    // After the identity check, both.
    assert!(request.starts_with("GET /Items "), "{request}");
    assert!(
        request.contains(TOKEN) && carries_gateway(request),
        "{request}"
    );
    assert_clean(&log.to_text());
    assert_clean(&format!("{roaming:?} {:?}", roaming.current()));
}

#[tokio::test]
async fn another_address_never_receives_gateway_credentials() {
    // Address 1 (standing in for the LAN address) has no gateway; address 2 has one. Address 1 wins the race and serves.
    let lan = plain_server().await;
    let public = plain_server().await;
    let log = Arc::new(RequestLog::default());
    let gateway = Arc::new(Gateway::new(everything()));
    let roaming = roaming(
        &[
            (address(&lan), None),
            (address(&public), Some(gateway.clone())),
        ],
        &log,
    );
    let client = ok(ServerClient::new(
        server(),
        roaming,
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config(),
    ));
    for _ in 0..3 {
        ok(client.send(Request::get("/Items")).await);
    }
    let received = lan.received();
    assert!(received.len() >= 4, "{received:?}");
    assert!(
        !received.iter().any(|head| carries_gateway(head)),
        "{received:?}"
    );
    // Whatever address 2 got (its probe, if it started), it never got the token.
    assert!(!public.received().iter().any(|head| head.contains(TOKEN)));

    // A single address without a gateway next to one with: nothing crosses either.
    let other = plain_server().await;
    ok(client_for(&other, None).send(Request::get("/Items")).await);
    assert!(!carries_gateway(&other.received()[0]));
}

fn client_for(stand_in: &StandIn, gateway: Option<Arc<Gateway>>) -> ServerClient {
    let log = Arc::new(RequestLog::default());
    match gateway {
        Some(gateway) => client(
            GatewayAddress::new(stand_in.url(), gateway, false),
            true,
            &log,
        ),
        None => client(FixedAddress(stand_in.url()), true, &log),
    }
}

#[tokio::test]
async fn cookies_go_only_where_rfc_6265_says() {
    let server = plain_server().await;
    let gateway = Arc::new(Gateway::new(cookies(vec![
        session_cookie(),
        ok(GatewayCookie::new(
            "secure_only",
            "secure-value",
            "127.0.0.1",
        ))
        .with_secure(true),
        ok(GatewayCookie::new(
            "elsewhere",
            "elsewhere-value",
            "media.example.com",
        )),
        ok(ok(GatewayCookie::new("deeper", "deeper-value", "127.0.0.1")).with_path("/jellyfin")),
    ])));
    ok(client_for(&server, Some(gateway))
        .send(Request::get("/Items"))
        .await);
    let head = &server.received()[0];
    assert_eq!(
        header(head, "cookie"),
        Some(format!("{COOKIE_NAME}={COOKIE_VALUE}").as_str())
    );
    for value in ["secure-value", "elsewhere-value", "deeper-value"] {
        assert!(!head.contains(value), "{head}");
    }
}

#[tokio::test]
async fn a_redirect_to_another_host_or_port_carries_nothing() {
    let target = plain_server().await;
    let port = target.port;
    for location in [
        format!("http://localhost:{port}/info"),
        format!("http://127.0.0.1:{port}/info"),
    ] {
        let redirecting =
            StandIn::start(move |_| answer(302, &[format!("location: {location}")], "")).await;
        let gateway = Arc::new(Gateway::new(everything()));
        let result = client_for(&redirecting, Some(gateway))
            .send(Request::get("/info"))
            .await;
        assert_eq!(classify(&result), GatewayShape::SignIn);
        assert_clean(&shown(&result));
    }
    assert!(target.received().is_empty());
}

#[tokio::test]
async fn a_same_host_redirect_keeps_the_gateway_credentials() {
    let server = StandIn::start(|head| {
        if head.starts_with("GET /info ") {
            answer(307, &["location: /moved/info".to_owned()], "")
        } else {
            info()
        }
    })
    .await;
    let gateway = Arc::new(Gateway::new(everything()));
    let result = client_for(&server, Some(gateway))
        .send(Request::get("/info"))
        .await;
    assert_eq!(classify(&result), GatewayShape::Expected);
    let received = server.received();
    assert_eq!(received.len(), 2);
    assert!(
        received.iter().all(|head| carries_gateway(head)),
        "{received:?}"
    );
}
