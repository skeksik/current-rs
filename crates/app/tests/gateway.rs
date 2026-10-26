//! Authentication gateways end to end through `App` (ADR 0017's test plan for `current-app`): setup's gateway states, saving and
//! loading each address's own credentials, rotated cookies saved, Jellyfin's header placement saved and reapplied, the 401 rule's
//! confirming probe (ADR 0015 as amended), a sign-in to the gateway again, and that no secret reaches a log, an error, a state or
//! Debug output. Against hand-written stand-ins on 127.0.0.1: one port runs a gateway (Basic, a cookie portal, a header gate, a
//! Negotiate-only gate, or none) in front of a small Jellyfin; another port reaches the same Jellyfin without a gateway, as a LAN
//! address does. Settings go to a temporary folder, secrets to memory.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use current_app::{
    AddServerState, App, CookieInput, GatewaySignInState, GatewaySubmitError, HeaderInput,
    NewServer, SignInState,
};
use current_jellyfin::{AuthError, GatewayAnswer, JellyfinClient};
use current_model::{
    AuthHeader, Connectivity, GatewayKind, NetworkFacts, NetworkKey, ServerKey, ServerRef,
    ServiceType,
};
use current_net::GatewayError;
use current_store::{GatewayKey, MemorySecrets, Secrets, get_gateway_secret};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SERVER_ID: &str = "0123456789abcdef0123456789abcdef";
const USER_ID: &str = "5d1c0000000000000000000000000001";
const PASSWORD: &str = "jf-pa55-word-value";
const GATE_USER: &str = "gate-user";
const GATE_PASSWORD: &str = "gate-pa55word";
/// `gate-user:gate-pa55word` in base64.
const BASIC: &str = "Z2F0ZS11c2VyOmdhdGUtcGE1NXdvcmQ=";
const REALM: &str = "Media at home";
const COOKIE_NAME: &str = "gate_session_name";
const COOKIE_VALUE: &str = "gate-cookie-value";
const ROTATED_VALUE: &str = "gate-cookie-rotated";
const RENEWED_VALUE: &str = "gate-cookie-renewed";
const PROVIDER_COOKIE: &str = "provider_session_name";
const HEADER_NAME: &str = "x-gate-key";
const HEADER_VALUE: &str = "gate-header-value";

/// Every secret no log line, error, state, setting or Debug output may show. The realm is the server's text, kept out of them too.
const SECRETS: &[&str] = &[
    PASSWORD,
    GATE_PASSWORD,
    BASIC,
    REALM,
    COOKIE_NAME,
    COOKIE_VALUE,
    ROTATED_VALUE,
    RENEWED_VALUE,
    PROVIDER_COOKIE,
    HEADER_VALUE,
    "t0ken",
];

/// What stands in front of the Jellyfin stand-in on the gateway's port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    /// No gateway.
    Open,
    /// HTTP Basic; `Authorization` is cleared before Jellyfin (a proxy that does not pass it on).
    Basic,
    /// A portal: without its cookie, an HTML sign-in page.
    Portal,
    /// A portal on another host: without its cookie, a redirect there.
    PortalRedirect,
    /// 403 unless the secret header is there.
    Header,
    /// 401 with a Negotiate challenge only.
    Negotiate,
}

/// One request as a stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    /// Which listener: `true` for the gateway's port.
    gated: bool,
    path: String,
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

    fn all(&self) -> String {
        self.headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}\n"))
            .collect()
    }
}

/// The stand-ins' shared state: one Jellyfin, reached through the gateway's port or the open one.
struct Server {
    gate: Mutex<Gate>,
    /// The portal cookie values the gateway accepts.
    cookies: Mutex<Vec<String>>,
    /// When set, `/Users/Me` through the portal rotates the cookie to this value.
    rotate_to: Mutex<Option<String>>,
    /// Closes the connection without an answer on `/System/Info/Public`.
    info_down: AtomicBool,
    /// `/System/Info/Public` answers 500.
    info_fails: AtomicBool,
    /// `/System/Info/Public` answers after 300 ms.
    info_slow: AtomicBool,
    /// The next `/Users/Me` answers 401, once.
    me_fails_once: AtomicBool,
    /// A proxy rule in front of the server blocks `/Users/Me` with a 403 page; the rest goes through.
    me_blocked: AtomicBool,
    /// The server's id.
    id: Mutex<String>,
    tokens: Mutex<HashSet<String>>,
    issued: AtomicUsize,
    received: Mutex<Vec<Received>>,
}

impl Server {
    fn new(gate: Gate) -> Arc<Self> {
        Arc::new(Self {
            gate: Mutex::new(gate),
            cookies: Mutex::new(vec![COOKIE_VALUE.to_owned()]),
            rotate_to: Mutex::new(None),
            info_down: AtomicBool::new(false),
            info_fails: AtomicBool::new(false),
            info_slow: AtomicBool::new(false),
            me_fails_once: AtomicBool::new(false),
            me_blocked: AtomicBool::new(false),
            id: Mutex::new(SERVER_ID.to_owned()),
            tokens: Mutex::new(HashSet::new()),
            issued: AtomicUsize::new(0),
            received: Mutex::new(Vec::new()),
        })
    }

    fn set_gate(&self, gate: Gate) {
        *self.gate.lock().unwrap() = gate;
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn clear_received(&self) {
        self.received.lock().unwrap().clear();
    }

    /// The gateway's answer when it does not let `request` through.
    fn gate(&self, request: &Received) -> Option<String> {
        let has_cookie = |request: &Received| {
            let line = request.header("cookie").unwrap_or_default().to_owned();
            self.cookies
                .lock()
                .unwrap()
                .iter()
                .any(|value| line.contains(&format!("{COOKIE_NAME}={value}")))
        };
        match *self.gate.lock().unwrap() {
            Gate::Open => None,
            Gate::Basic => (request.header("authorization") != Some(&format!("Basic {BASIC}")))
                .then(|| {
                    answer(
                        401,
                        &[format!("www-authenticate: Basic realm=\"{REALM}\"")],
                        "",
                    )
                }),
            Gate::Portal => (!has_cookie(request)).then(|| {
                answer(
                    200,
                    &["content-type: text/html".to_owned()],
                    "<!DOCTYPE html><html><body>Sign in</body></html>",
                )
            }),
            Gate::PortalRedirect => (!has_cookie(request))
                .then(|| answer(302, &["location: http://127.0.0.1:1/login".to_owned()], "")),
            Gate::Header => {
                (request.header(HEADER_NAME) != Some(HEADER_VALUE)).then(|| answer(403, &[], ""))
            }
            Gate::Negotiate => Some(answer(401, &["www-authenticate: Negotiate".to_owned()], "")),
        }
    }

    /// The Jellyfin stand-in. Its client fields come from `Authorization: MediaBrowser ...` or, when that header is empty, from
    /// `X-Emby-Authorization` (Jellyfin with legacy authorization on).
    fn jellyfin(&self, request: &Received) -> String {
        let fields = request
            .header("authorization")
            .filter(|value| value.starts_with("MediaBrowser"))
            .or_else(|| {
                request
                    .header("authorization")
                    .is_none()
                    .then(|| request.header("x-emby-authorization"))
                    .flatten()
            })
            .unwrap_or_default()
            .to_owned();
        let token = fields
            .split_once("Token=\"")
            .and_then(|(_, rest)| rest.split('"').next())
            .map(str::to_owned);
        let valid = token
            .as_ref()
            .is_some_and(|token| self.tokens.lock().unwrap().contains(token));
        let id = self.id.lock().unwrap().clone();
        match request.path.as_str() {
            "/System/Info/Public" if self.info_fails.load(Ordering::SeqCst) => json(500, ""),
            "/Users/Me" if self.me_fails_once.swap(false, Ordering::SeqCst) => json(401, ""),
            "/System/Info/Public" => json(
                200,
                &format!(
                    r#"{{"ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{id}","StartupWizardCompleted":true}}"#
                ),
            ),
            "/Users/AuthenticateByName" if !fields.contains("Client=") => json(400, ""),
            "/Users/AuthenticateByName" if request.body.contains(PASSWORD) => {
                let token = format!("t0ken{:027}", self.issued.fetch_add(1, Ordering::SeqCst));
                self.tokens.lock().unwrap().insert(token.clone());
                json(
                    200,
                    &format!(
                        r#"{{"User":{},"SessionInfo":{{}},"AccessToken":"{token}","ServerId":"{id}"}}"#,
                        user()
                    ),
                )
            }
            "/Users/AuthenticateByName" => json(401, ""),
            "/QuickConnect/Enabled" => json(200, "false"),
            "/Users/Me" if valid => {
                let rotate = self.rotate_to.lock().unwrap().take();
                match rotate {
                    Some(value) => {
                        self.cookies.lock().unwrap().push(value.clone());
                        answer(
                            200,
                            &[
                                "content-type: application/json".to_owned(),
                                format!("set-cookie: {COOKIE_NAME}={value}; Path=/; HttpOnly"),
                            ],
                            &user(),
                        )
                    }
                    None => json(200, &user()),
                }
            }
            "/Sessions/Logout" => answer(204, &[], ""),
            _ => json(401, ""),
        }
    }

    fn revoke_all(&self) {
        self.tokens.lock().unwrap().clear();
    }
}

fn user() -> String {
    format!(r#"{{"Name":"Example User","Id":"{USER_ID}","Policy":{{"IsAdministrator":true}}}}"#)
}

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

/// Opens a port for `server`: through its gateway when `gated`, straight to Jellyfin otherwise. Returns the address.
async fn listen(server: &Arc<Server>, gated: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let server = server.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let Some(mut request) = read_request(&mut socket, gated).await else {
                    return;
                };
                server.received.lock().unwrap().push(request.clone());
                if request.path == "/System/Info/Public" {
                    if server.info_down.load(Ordering::SeqCst) {
                        return;
                    }
                    if server.info_slow.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
                if gated && request.path == "/Users/Me" && server.me_blocked.load(Ordering::SeqCst)
                {
                    let page = "<!DOCTYPE html><html><body>Blocked</body></html>";
                    let text = answer(403, &["content-type: text/html".to_owned()], page);
                    let _ = socket.write_all(text.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
                let text = match gated.then(|| server.gate(&request)).flatten() {
                    Some(blocked) => blocked,
                    None => {
                        if gated && *server.gate.lock().unwrap() == Gate::Basic {
                            // The proxy does not pass its own `Authorization` on.
                            request
                                .headers
                                .retain(|(key, _)| !key.eq_ignore_ascii_case("authorization"));
                        }
                        server.jellyfin(&request)
                    }
                };
                let _ = socket.write_all(text.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    address
}

async fn read_request(socket: &mut tokio::net::TcpStream, gated: bool) -> Option<Received> {
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
    let path = lines
        .next()?
        .split(' ')
        .nth(1)?
        .split('?')
        .next()?
        .to_owned();
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
        gated,
        path,
        headers,
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

fn jellyfin(key: &str) -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key))
}

fn open(dir: &Path, secrets: &Arc<MemorySecrets>) -> App {
    let secrets: Arc<dyn Secrets> = secrets.clone();
    App::open(dir, &dir.join("cache"), secrets, "Office PC")
        .unwrap()
        .0
}

fn portal_cookie(value: &str) -> CookieInput {
    CookieInput {
        name: COOKIE_NAME.into(),
        value: value.into(),
        domain: "127.0.0.1".into(),
        path: "/".into(),
        secure: false,
        http_only: true,
        expires: Some(SystemTime::now() + Duration::from_secs(3600)),
    }
}

/// A cookie the identity provider set for its own host: it stays in the web view.
fn provider_cookie() -> CookieInput {
    CookieInput {
        name: PROVIDER_COOKIE.into(),
        value: "provider-value".into(),
        domain: "login.example.net".into(),
        ..portal_cookie("x")
    }
}

/// Waits (at most 5 s) until `check` passes.
async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 5 s");
}

fn state_of(flow: &current_app::AddServer) -> AddServerState {
    flow.state().borrow().clone()
}

fn ready(state: AddServerState) -> NewServer {
    match state {
        AddServerState::Ready { server } => server,
        state => panic!("{state:?}"),
    }
}

/// Signs in to the server setup found and returns the session's client.
async fn sign_in(app: &App, server: NewServer) -> Arc<JellyfinClient> {
    let flow = app.sign_in(server).unwrap();
    flow.with_password("Example User", PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
    // The provider's race after the sign-in has selected an address: what the stand-in does next reaches the session.
    eventually(|| app.capabilities().borrow().servers[0].address.is_some()).await;
    app.jellyfin(&jellyfin("1")).unwrap()
}

/// The stored gateway secret of the saved server's address `index` (0-based), as text.
fn stored_gateway(app: &App, secrets: &MemorySecrets, index: usize) -> Option<String> {
    let saved = &app.servers()[0];
    let id = saved.addresses[index].id.unwrap();
    get_gateway_secret(secrets, &GatewayKey::new(saved.server.clone(), id))
        .unwrap()
        .map(|secret| secret.expose().to_owned())
}

/// Everything a user could paste into a bug report or the UI holds: none of it may contain a secret.
fn assert_nothing_secret(app: &App, dir: &Path, extra: &str) {
    let settings = std::fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
    let text = format!(
        "{}\n{settings}\n{extra}\n{app:?}\n{:?}\n{:?}",
        app.diagnostics_text(),
        app.servers(),
        *app.capabilities().borrow(),
    );
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} in:\n{text}");
    }
}

#[tokio::test]
async fn a_basic_gateway_asks_again_for_a_wrong_password_then_signs_in_and_restores() {
    let server = Server::new(Gate::Basic);
    let address = listen(&server, true).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    let AddServerState::GatewayPassword {
        origin,
        realm,
        wrong: false,
        plain_http: false,
        ..
    } = state_of(&setup)
    else {
        panic!("{:?}", state_of(&setup))
    };
    assert_eq!(origin, address);
    assert_eq!(realm.unwrap().as_str(), REALM);
    let mut shown = format!("{:?}", state_of(&setup));

    // Nothing is sent for credentials that cannot be used; a wrong password asks again.
    assert_eq!(
        setup.submit_password("a:b", GATE_PASSWORD, false).await,
        Err(GatewaySubmitError::Invalid(GatewayError::InvalidUserName))
    );
    setup
        .submit_password(GATE_USER, "not-the-gate-password", false)
        .await
        .unwrap();
    assert!(
        matches!(
            state_of(&setup),
            AddServerState::GatewayPassword { wrong: true, .. }
        ),
        "{:?}",
        state_of(&setup)
    );
    setup
        .submit_password(GATE_USER, GATE_PASSWORD, false)
        .await
        .unwrap();
    let found = ready(state_of(&setup));
    shown.push_str(&format!("{found:?} {:?}", state_of(&setup)));
    // The probe carried the gateway's password and never Jellyfin's header.
    for request in server.received() {
        assert!(!request.all().contains("MediaBrowser"), "{request:?}");
    }

    let client = sign_in(&app, found).await;
    // Jellyfin's details moved to X-Emby-Authorization, past the gateway's `Authorization`.
    let sign_in = server
        .received()
        .into_iter()
        .find(|r| r.path == "/Users/AuthenticateByName")
        .unwrap();
    assert_eq!(
        sign_in.header("authorization"),
        Some(&*format!("Basic {BASIC}"))
    );
    assert!(
        sign_in
            .header("x-emby-authorization")
            .unwrap()
            .contains("Client=")
    );
    let saved = &app.servers()[0];
    assert_eq!(saved.header_placement, AuthHeader::X_EMBY_AUTHORIZATION);
    assert_eq!(saved.addresses[0].gateway, [GatewayKind::BASIC]);
    let stored = stored_gateway(&app, &secrets, 0).unwrap();
    assert!(stored.contains(GATE_PASSWORD));
    assert!(client.current_user().await.is_ok());
    assert_nothing_secret(&app, temp.path(), &shown);
    drop((client, app));

    // Restored from the settings and the keyring: the header placement and the password are used again.
    server.clear_received();
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in, "{status:?}");
    assert!(!status.gateway_sign_in_needed);
    let me = server
        .received()
        .into_iter()
        .find(|r| r.path == "/Users/Me")
        .unwrap();
    assert_eq!(me.header("authorization"), Some(&*format!("Basic {BASIC}")));
    assert!(
        me.header("x-emby-authorization")
            .unwrap()
            .contains("Token=")
    );
    assert_nothing_secret(&app, temp.path(), "");
}

#[tokio::test]
async fn a_web_sign_in_keeps_only_the_servers_cookies_and_never_sends_the_token_with_them() {
    let server = Server::new(Gate::Portal);
    let address = listen(&server, true).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    let AddServerState::GatewaySignIn {
        url,
        origin,
        redirected: false,
        location: None,
        plain_http: false,
        ..
    } = state_of(&setup)
    else {
        panic!("{:?}", state_of(&setup))
    };
    assert_eq!(
        (url.as_str(), origin.as_str()),
        (address.as_str(), address.as_str())
    );
    let before = server.received().len();

    // Only the identity provider's cookie: nothing applies to the server, nothing is sent.
    setup
        .submit_cookies(&[provider_cookie()], false)
        .await
        .unwrap();
    assert_eq!(server.received().len(), before);
    // A cookie the gateway does not take (yet): still waiting, the window stays open.
    setup
        .submit_cookies(&[portal_cookie("not-yet"), provider_cookie()], false)
        .await
        .unwrap();
    assert!(matches!(
        state_of(&setup),
        AddServerState::GatewaySignIn { .. }
    ));
    assert_eq!(server.received().len(), before + 1);
    setup
        .submit_cookies(&[portal_cookie(COOKIE_VALUE), provider_cookie()], false)
        .await
        .unwrap();
    let found = ready(state_of(&setup));
    // The probes after the sign-in carried only the server's cookie, and no Jellyfin header or token.
    for request in &server.received()[before..] {
        let cookie = request.header("cookie").unwrap();
        assert!(cookie.starts_with(COOKIE_NAME), "{request:?}");
        assert!(!cookie.contains(PROVIDER_COOKIE), "{request:?}");
        assert!(!request.all().contains("MediaBrowser"), "{request:?}");
    }

    let client = sign_in(&app, found).await;
    assert!(client.current_user().await.is_ok());
    let saved = &app.servers()[0];
    assert_eq!(saved.addresses[0].gateway, [GatewayKind::COOKIES]);
    assert_eq!(saved.header_placement, AuthHeader::AUTHORIZATION);
    // The provider's cookie never reached the keyring.
    let stored = stored_gateway(&app, &secrets, 0).unwrap();
    assert!(stored.contains(COOKIE_VALUE));
    assert!(!stored.contains(PROVIDER_COOKIE));
    assert_nothing_secret(&app, temp.path(), &format!("{:?}", state_of(&setup)));
}

#[tokio::test]
async fn a_redirect_offers_sign_in_and_the_address_it_points_to() {
    let server = Server::new(Gate::PortalRedirect);
    let address = listen(&server, true).await;
    let app = App::new();
    let setup = app.add_server();
    setup.probe(&address).await;
    let AddServerState::GatewaySignIn {
        redirected: true,
        location: Some(location),
        ..
    } = state_of(&setup)
    else {
        panic!("{:?}", state_of(&setup))
    };
    assert_eq!(location.url().as_str(), "http://127.0.0.1:1/login");
    assert!(!format!("{:?}", state_of(&setup)).contains("/login"));
    setup
        .submit_cookies(&[portal_cookie(COOKIE_VALUE)], false)
        .await
        .unwrap();
    assert!(matches!(state_of(&setup), AddServerState::Ready { .. }));
}

#[tokio::test]
async fn a_refusing_gateway_takes_extra_headers() {
    let server = Server::new(Gate::Header);
    let address = listen(&server, true).await;
    let app = App::new();
    let setup = app.add_server();
    setup.probe(&address).await;
    assert!(
        matches!(
            state_of(&setup),
            AddServerState::GatewayRefused {
                plain_http: false,
                ..
            }
        ),
        "{:?}",
        state_of(&setup)
    );
    let header = |name: &str, value: &str| HeaderInput {
        name: name.into(),
        value: value.into(),
    };
    assert_eq!(
        setup.submit_headers(&[header("Cookie", "x")], false).await,
        Err(GatewaySubmitError::Invalid(GatewayError::ReservedHeader))
    );
    assert_eq!(
        setup
            .submit_cookies(&[portal_cookie(COOKIE_VALUE)], false)
            .await,
        Err(GatewaySubmitError::NotAsked)
    );
    setup
        .submit_headers(&[header(HEADER_NAME, "wrong")], false)
        .await
        .unwrap();
    assert!(matches!(
        state_of(&setup),
        AddServerState::GatewayRefused { .. }
    ));
    setup
        .submit_headers(&[header(HEADER_NAME, HEADER_VALUE)], false)
        .await
        .unwrap();
    let found = ready(state_of(&setup));
    assert!(!format!("{found:?}").contains(HEADER_VALUE));

    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let client = sign_in(&app, found).await;
    assert!(client.current_user().await.is_ok());
    assert_eq!(
        app.servers()[0].addresses[0].gateway,
        [GatewayKind::HEADERS]
    );
    assert_nothing_secret(&app, temp.path(), "");
}

#[tokio::test]
async fn an_unsupported_sign_in_method_is_its_own_state() {
    let server = Server::new(Gate::Negotiate);
    let address = listen(&server, true).await;
    let app = App::new();
    let setup = app.add_server();
    setup.probe(&address).await;
    let AddServerState::GatewayUnsupported { origin, .. } = state_of(&setup) else {
        panic!("{:?}", state_of(&setup))
    };
    assert_eq!(origin, address);
    assert_eq!(
        setup.submit_password(GATE_USER, GATE_PASSWORD, false).await,
        Err(GatewaySubmitError::NotAsked)
    );
}

#[tokio::test]
async fn gateway_credentials_stay_with_their_own_address() {
    let server = Server::new(Gate::Portal);
    let public = listen(&server, true).await;
    let lan = listen(&server, false).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);

    let setup = app.add_server();
    setup.probe(&public).await;
    setup
        .submit_cookies(&[portal_cookie(COOKIE_VALUE)], false)
        .await
        .unwrap();
    sign_in(&app, ready(state_of(&setup))).await;
    // The same server found on its LAN address: the address is added, without gateway credentials.
    let setup = app.add_server();
    setup.probe(&lan).await;
    sign_in(&app, ready(state_of(&setup))).await;
    let saved = &app.servers()[0];
    assert_eq!(saved.addresses.len(), 2);
    assert_eq!(saved.addresses[0].gateway, [GatewayKind::COOKIES]);
    assert!(saved.addresses[1].gateway.is_empty());
    assert!(stored_gateway(&app, &secrets, 1).is_none());
    drop(app);

    // Restored over both addresses, on two networks: the LAN address never gets the gateway's cookie.
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    for key in ["home", "away"] {
        app.network_changed(NetworkFacts {
            connectivity: Connectivity::Full,
            network_key: Some(NetworkKey::new(key)),
            ..NetworkFacts::default()
        });
        let client = app.jellyfin(&jellyfin("1")).unwrap();
        assert!(client.current_user().await.is_ok());
    }
    let received = server.received();
    assert!(received.iter().any(|r| !r.gated));
    for request in received.iter().filter(|r| !r.gated) {
        assert!(request.header("cookie").is_none(), "{request:?}");
    }
    assert_nothing_secret(&app, temp.path(), "");
}

#[tokio::test]
async fn rotated_cookies_are_saved() {
    let server = Server::new(Gate::Portal);
    let address = listen(&server, true).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    setup
        .submit_cookies(&[portal_cookie(COOKIE_VALUE)], false)
        .await
        .unwrap();
    let client = sign_in(&app, ready(state_of(&setup))).await;

    *server.rotate_to.lock().unwrap() = Some(ROTATED_VALUE.into());
    assert!(client.current_user().await.is_ok());
    // Saved on the provider's next status change (or within a minute).
    app.network_changed(NetworkFacts {
        connectivity: Connectivity::Full,
        network_key: Some(NetworkKey::new("home")),
        ..NetworkFacts::default()
    });
    eventually(|| {
        stored_gateway(&app, &secrets, 0).is_some_and(|text| text.contains(ROTATED_VALUE))
    })
    .await;
    // The old value no longer works; a restart uses the saved new one.
    server
        .cookies
        .lock()
        .unwrap()
        .retain(|v| v == ROTATED_VALUE);
    drop((client, app));
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(
        status.signed_in && !status.gateway_sign_in_needed,
        "{status:?}"
    );
    assert_eq!(status.user.map(|u| u.name).as_deref(), Some("Example User"));
    assert_nothing_secret(&app, temp.path(), "");
}

/// A server signed in through a cookie portal, and its session's client.
async fn signed_in_through_portal(
    dir: &Path,
    secrets: &Arc<MemorySecrets>,
) -> (Arc<Server>, App, Arc<JellyfinClient>) {
    let server = Server::new(Gate::Portal);
    let address = listen(&server, true).await;
    let app = open(dir, secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    setup
        .submit_cookies(&[portal_cookie(COOKIE_VALUE)], false)
        .await
        .unwrap();
    let client = sign_in(&app, ready(state_of(&setup))).await;
    (server, app, client)
}

fn token_kept(app: &App, secrets: &MemorySecrets) -> bool {
    secrets
        .get(&current_store::SecretKey::new(
            jellyfin("1"),
            current_model::UserId::new(USER_ID),
        ))
        .unwrap()
        .is_some()
        && app.servers()[0].active_user.is_some()
}

#[tokio::test]
async fn a_jellyfin_401_through_the_gateway_still_ends_the_session() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_through_portal(temp.path(), &secrets).await;
    server.revoke_all();
    // The confirming probe passed the gateway with its cookie and found the server: the 401 was Jellyfin's, and the token check
    // that follows got 401 too.
    assert_eq!(client.current_user().await, Err(AuthError::SessionRevoked));
    eventually(|| !app.capabilities().borrow().servers[0].signed_in).await;
    eventually(|| !token_kept(&app, &secrets)).await;
    assert!(!client.has_token());
    let probe = server
        .received()
        .into_iter()
        .rev()
        .find(|r| r.path == "/System/Info/Public")
        .unwrap();
    assert!(probe.header("cookie").is_some());
    assert!(!probe.all().contains("MediaBrowser"));
}

#[tokio::test]
async fn an_expired_gateway_session_keeps_the_jellyfin_session_until_signed_in_again() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_through_portal(temp.path(), &secrets).await;
    // The gateway forgets its session: requests get its sign-in page, which is no JSON. Once the probe found the gateway's
    // answer, the request says so, not that the page could not be read.
    server.cookies.lock().unwrap().clear();
    let result = client.current_user().await;
    assert_eq!(result, Err(AuthError::Gateway(GatewayAnswer::SignIn)));
    eventually(|| app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in, "{status:?}");
    assert!(client.has_token());
    assert!(token_kept(&app, &secrets));

    // Signing in to the gateway again resumes the session.
    server.cookies.lock().unwrap().push(RENEWED_VALUE.into());
    let flow = app.sign_in_to_gateway(&jellyfin("1")).unwrap();
    flow.start().await;
    assert!(
        matches!(
            *flow.state().borrow(),
            GatewaySignInState::WebSignIn {
                redirected: false,
                ..
            }
        ),
        "{:?}",
        *flow.state().borrow()
    );
    flow.submit_cookies(&[portal_cookie(RENEWED_VALUE), provider_cookie()])
        .await
        .unwrap();
    assert_eq!(
        *flow.state().borrow(),
        GatewaySignInState::Done { remembered: true }
    );
    eventually(|| !app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    assert!(client.current_user().await.is_ok());
    let stored = stored_gateway(&app, &secrets, 0).unwrap();
    assert!(stored.contains(RENEWED_VALUE) && !stored.contains(PROVIDER_COOKIE));
    assert_nothing_secret(&app, temp.path(), &format!("{flow:?}"));
}

/// A portal on another host redirects there once its session ends: the request answers the gateway's sign-in, not the redirect, so
/// a screen shows "Sign in to the gateway again" and not "HTTP 302" (Windows relay check, 2026-10-26).
#[tokio::test]
async fn an_expired_gateway_session_that_redirects_answers_the_gateway_sign_in() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_through_portal(temp.path(), &secrets).await;
    server.set_gate(Gate::PortalRedirect);
    server.cookies.lock().unwrap().clear();
    let result = client.current_user().await;
    assert_eq!(result, Err(AuthError::Gateway(GatewayAnswer::SignIn)));
    eventually(|| app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    assert!(app.capabilities().borrow().servers[0].signed_in);
    assert!(client.has_token());
    assert!(token_kept(&app, &secrets));
}

fn info_probes(server: &Server) -> usize {
    server
        .received()
        .iter()
        .filter(|r| r.path == "/System/Info/Public")
        .count()
}

/// Asks for the current user until the answer is not `Unconfirmed` (at most 5 s): the check has ended.
async fn after_the_check(client: &JellyfinClient) -> Result<current_model::User, AuthError> {
    for _ in 0..500 {
        let result = client.current_user().await;
        if result != Err(AuthError::Unconfirmed) {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("still unconfirmed after 5 s");
}

/// A server signed in without a gateway.
async fn signed_in_directly(
    dir: &Path,
    secrets: &Arc<MemorySecrets>,
) -> (Arc<Server>, App, Arc<JellyfinClient>) {
    let server = Server::new(Gate::Open);
    let address = listen(&server, true).await;
    let app = open(dir, secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    let client = sign_in(&app, ready(state_of(&setup))).await;
    (server, app, client)
}

#[tokio::test]
async fn a_401_whose_probe_gets_another_answer_runs_one_probe_and_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_directly(temp.path(), &secrets).await;
    let before = info_probes(&server);
    server.info_fails.store(true, Ordering::SeqCst);
    server.me_fails_once.store(true, Ordering::SeqCst);
    assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
    // Once the check is done, requests go out again and work.
    assert!(after_the_check(&client).await.is_ok());
    assert_eq!(info_probes(&server), before + 1);
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(
        status.signed_in && !status.gateway_sign_in_needed,
        "{status:?}"
    );
    assert!(token_kept(&app, &secrets));
}

#[tokio::test]
async fn a_401_whose_probe_reaches_nothing_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_directly(temp.path(), &secrets).await;
    server.info_down.store(true, Ordering::SeqCst);
    server.me_fails_once.store(true, Ordering::SeqCst);
    assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
    // The probe reached nothing: the session and its token stay, and requests are not held back once the server is back.
    let result = after_the_check(&client).await;
    assert_ne!(result, Err(AuthError::SessionRevoked));
    server.info_down.store(false, Ordering::SeqCst);
    app.returned_to_foreground();
    eventually(|| {
        app.capabilities().borrow().servers[0].reachability
            == current_model::Reachability::Reachable
    })
    .await;
    assert!(after_the_check(&client).await.is_ok());
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(
        status.signed_in && !status.gateway_sign_in_needed,
        "{status:?}"
    );
    assert!(token_kept(&app, &secrets));
}

#[tokio::test]
async fn concurrent_401s_start_one_probe() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_directly(temp.path(), &secrets).await;
    let before = info_probes(&server);
    server.info_slow.store(true, Ordering::SeqCst);
    server.revoke_all();
    let results = futures_join(&client, 5).await;
    // A request that got the 401 waits for the check and learns the session ended; one sent while the check ran is held back.
    assert!(
        results
            .iter()
            .all(|r| matches!(r, Err(AuthError::Unconfirmed | AuthError::SessionRevoked))),
        "{results:?}"
    );
    assert!(
        results.contains(&Err(AuthError::SessionRevoked)),
        "{results:?}"
    );
    eventually(|| !app.capabilities().borrow().servers[0].signed_in).await;
    eventually(|| !token_kept(&app, &secrets)).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(info_probes(&server), before + 1);
}

/// `count` requests for the current user at once.
async fn futures_join(
    client: &Arc<JellyfinClient>,
    count: usize,
) -> Vec<Result<current_model::User, AuthError>> {
    let tasks: Vec<_> = (0..count)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.current_user().await })
        })
        .collect();
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    results
}

#[tokio::test]
async fn a_blocked_page_with_a_passing_probe_never_ends_the_session() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_through_portal(temp.path(), &secrets).await;
    let before = info_probes(&server);
    server.me_blocked.store(true, Ordering::SeqCst);
    // A 403 page is no 401: the request is refused, and the check that runs ends without touching the session.
    assert_eq!(client.current_user().await, Err(AuthError::Refused));
    eventually(|| info_probes(&server) == before + 1).await;
    server.me_blocked.store(false, Ordering::SeqCst);
    assert!(after_the_check(&client).await.is_ok());
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(
        status.signed_in && !status.gateway_sign_in_needed,
        "{status:?}"
    );
    assert!(client.has_token() && token_kept(&app, &secrets));
}

#[tokio::test]
async fn a_failed_settings_write_leaves_no_gateway_for_the_next_server() {
    let gated = Server::new(Gate::Basic);
    let gated_address = listen(&gated, true).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&gated_address).await;
    setup
        .submit_password(GATE_USER, GATE_PASSWORD, false)
        .await
        .unwrap();
    let found = ready(state_of(&setup));

    // The settings cannot be written: a folder stands where the file goes.
    let file = temp.path().join(current_store::SETTINGS_FILE);
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    std::fs::write(file.join("keep"), "").unwrap();
    let flow = app.sign_in(found).unwrap();
    flow.with_password("Example User", PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::Failed { .. }), "{state:?}");
    assert!(app.servers().is_empty());
    // Nothing is left under the key and address id the next server gets.
    let key = GatewayKey::new(jellyfin("1"), current_model::AddressId::new(1));
    assert!(
        get_gateway_secret(secrets.as_ref(), &key)
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(&file).unwrap();

    // Another server, without a gateway, gets that key and id: it never receives the first server's password.
    let other = Server::new(Gate::Open);
    *other.id.lock().unwrap() = "fedcba9876543210fedcba9876543210".into();
    let other_address = listen(&other, true).await;
    let setup = app.add_server();
    setup.probe(&other_address).await;
    let client = sign_in(&app, ready(state_of(&setup))).await;
    assert_eq!(app.servers()[0].server, jellyfin("1"));
    assert!(client.current_user().await.is_ok());
    for request in other.received() {
        assert!(!request.all().contains(BASIC), "{request:?}");
    }
}

#[tokio::test]
async fn an_address_without_a_gateway_never_moves_the_header_back() {
    let server = Server::new(Gate::Basic);
    let public = listen(&server, true).await;
    let lan = listen(&server, false).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&public).await;
    setup
        .submit_password(GATE_USER, GATE_PASSWORD, false)
        .await
        .unwrap();
    sign_in(&app, ready(state_of(&setup))).await;
    assert_eq!(
        app.servers()[0].header_placement,
        AuthHeader::X_EMBY_AUTHORIZATION
    );
    // The LAN address signs in with `Authorization`: the server's setting stays, for the gated address.
    let setup = app.add_server();
    setup.probe(&lan).await;
    sign_in(&app, ready(state_of(&setup))).await;
    assert_eq!(app.servers()[0].addresses.len(), 2);
    assert_eq!(
        app.servers()[0].header_placement,
        AuthHeader::X_EMBY_AUTHORIZATION
    );
}

#[tokio::test]
async fn a_gateway_challenge_mid_session_asks_for_the_password_again() {
    let server = Server::new(Gate::Basic);
    let address = listen(&server, true).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let setup = app.add_server();
    setup.probe(&address).await;
    setup
        .submit_password(GATE_USER, GATE_PASSWORD, false)
        .await
        .unwrap();
    let client = sign_in(&app, ready(state_of(&setup))).await;
    // The gateway answers with a challenge: never a session end (the 401 rule's first part).
    server.set_gate(Gate::Negotiate);
    assert!(matches!(
        client.current_user().await,
        Err(AuthError::Gateway(_))
    ));
    eventually(|| app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    assert!(client.has_token() && token_kept(&app, &secrets));

    server.set_gate(Gate::Basic);
    let flow = app.sign_in_to_gateway(&jellyfin("1")).unwrap();
    flow.start().await;
    // The stored password passes again: nothing to ask, the session resumes.
    assert_eq!(
        *flow.state().borrow(),
        GatewaySignInState::Done { remembered: true }
    );
    eventually(|| !app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    assert_nothing_secret(&app, temp.path(), &format!("{flow:?}"));
}

#[tokio::test]
async fn an_expired_gateway_at_restore_shows_sign_in_to_the_gateway_not_unreachable() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (server, app, client) = signed_in_through_portal(temp.path(), &secrets).await;
    drop((client, app));
    // Overnight, the gateway's session ended.
    server.cookies.lock().unwrap().clear();
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    eventually(|| app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in, "{status:?}");
    assert!(token_kept(&app, &secrets));
    // Probed once, not in a loop.
    let probes = || {
        server
            .received()
            .iter()
            .filter(|r| r.path == "/System/Info/Public")
            .count()
    };
    let seen = probes();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(probes() <= seen + 1, "{} after {seen}", probes());
}
