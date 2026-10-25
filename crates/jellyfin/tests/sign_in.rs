//! Sign-in against hand-written Jellyfin stand-ins on 127.0.0.1. Responses follow Jellyfin 12.2 (ADR 0015).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, JellyfinClient, QuickConnectSecret, SessionEvent,
    WaitClock,
};
use current_model::{Permission, ServerId, ServerKey, ServerRef, ServiceType, UserId};
use current_net::{FixedAddress, NetError, Request, RequestLog, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const PASSWORD: &str = "pa55-word-value";
const QC_SECRET: &str = "5ecre7000000000000000000000000000000000000000000000000000000beef";
const USER_ID: &str = "5d1c0000000000000000000000000001";

/// One request as the stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    query: String,
    authorization: String,
    body: String,
}

type Handler = Arc<dyn Fn(&Received) -> (u16, String) + Send + Sync>;

/// A server on 127.0.0.1 that answers each request with what `handler` returns, and keeps every request.
struct StandIn {
    address: Url,
    received: Arc<Mutex<Vec<Received>>>,
}

impl StandIn {
    async fn start(handler: impl Fn(&Received) -> (u16, String) + Send + Sync + 'static) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request.clone());
                    let (status, body) = handler(&request);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            address: Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
            received,
        }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn paths(&self) -> Vec<String> {
        self.received().into_iter().map(|r| r.path).collect()
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
    let header = |name: &str| {
        head.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while data.len() < head_end + length {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    }
    let mut first = head.lines().next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Received {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        authorization: header("authorization"),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

fn user_json() -> String {
    format!(
        r#"{{"Name":"Example User","ServerId":"0123456789abcdef0123456789abcdef","Id":"{USER_ID}","HasPassword":true,"Policy":{{"IsAdministrator":false,"IsDisabled":false,"EnableContentDownloading":true,"EnableMediaPlayback":true}}}}"#
    )
}

fn auth_result() -> String {
    format!(
        r#"{{"User":{},"SessionInfo":{{"Id":"s1"}},"AccessToken":"{TOKEN}","ServerId":"0123456789abcdef0123456789abcdef"}}"#,
        user_json()
    )
}

fn quick_connect_json(authenticated: bool) -> String {
    format!(
        r#"{{"Authenticated":{authenticated},"Secret":"{QC_SECRET}","Code":"123456","DeviceId":"d","DeviceName":"Office PC","AppName":"Current","AppVersion":"0.1.0","DateAdded":"2026-10-22T10:00:00Z"}}"#
    )
}

fn identity() -> ClientIdentity {
    ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef")
}

fn client(stand_in: &StandIn, log: &Arc<RequestLog>) -> JellyfinClient {
    JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        Arc::new(FixedAddress(stand_in.address.clone())),
        identity(),
        log.clone(),
    )
    .unwrap()
}

/// Checks that nothing secret reached the request log or a value's Debug output.
fn assert_no_secret(log: &RequestLog, debug: &str) {
    let text = format!("{}\n{debug}", log.to_text());
    for secret in [TOKEN, PASSWORD, QC_SECRET, "127.0.0.1"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

/// Jellyfin's sign-in: 200 for the right password, 401 for a wrong one, 403 for a refused user.
fn sign_in_server(received: &Received) -> (u16, String) {
    match received.path.as_str() {
        "/Users/AuthenticateByName" if received.body.contains(PASSWORD) => (200, auth_result()),
        "/Users/AuthenticateByName" if received.body.contains("refused") => {
            (403, "Error processing request.".into())
        }
        "/Users/AuthenticateByName" => (401, "Error processing request.".into()),
        _ => (404, String::new()),
    }
}

#[tokio::test]
async fn a_password_sign_in_returns_the_user_and_token() {
    let server = StandIn::start(sign_in_server).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let signed_in = client
        .sign_in_by_name("Example User", PASSWORD)
        .await
        .unwrap();
    assert_eq!(signed_in.token, AccessToken::new(TOKEN));
    assert_eq!(signed_in.user.id, UserId::new(USER_ID));
    assert_eq!(signed_in.user.name, "Example User");
    assert!(signed_in.user.has_permission(&Permission::DOWNLOAD));
    assert_eq!(
        signed_in.server_id,
        Some(ServerId::new("0123456789abcdef0123456789abcdef"))
    );
    // Signing in does not start using the token by itself.
    assert!(!client.has_token());

    let request = &server.received()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.body,
        format!(r#"{{"Username":"Example User","Pw":"{PASSWORD}"}}"#)
    );
    assert_eq!(
        request.authorization,
        format!(
            r#"MediaBrowser Client="Current", Device="Office%20PC", DeviceId="0123456789abcdef0123456789abcdef", Version="{}""#,
            current_model::version()
        )
    );
    assert_no_secret(&log, &format!("{signed_in:?} {client:?}"));
    assert!(
        log.to_text()
            .contains("jellyfin/1 POST /Users/AuthenticateByName -> 200")
    );
}

#[tokio::test]
async fn a_wrong_password_or_a_refused_user_is_told_apart() {
    let server = StandIn::start(sign_in_server).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let wrong = client.sign_in_by_name("Example User", "wrong").await;
    assert_eq!(wrong, Err(AuthError::WrongCredentials));
    let refused = client.sign_in_by_name("Example User", "refused").await;
    assert_eq!(refused, Err(AuthError::Refused));
    assert_no_secret(&log, &format!("{wrong:?} {refused:?}"));
}

#[tokio::test]
async fn an_unreachable_server_is_a_network_error() {
    let log = Arc::new(RequestLog::default());
    let client = JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        Arc::new(FixedAddress(Url::parse("http://127.0.0.1:1").unwrap())),
        identity(),
        log.clone(),
    )
    .unwrap();
    let result = client.sign_in_by_name("Example User", PASSWORD).await;
    assert_eq!(result, Err(AuthError::Network(NetError::Unreachable)));
    assert_no_secret(&log, &format!("{result:?}"));
}

/// Quick Connect that is approved after `polls_before_approval` state checks.
fn quick_connect_server(polls_before_approval: usize) -> impl Fn(&Received) -> (u16, String) {
    let polls = AtomicUsize::new(0);
    move |received| match received.path.as_str() {
        "/QuickConnect/Enabled" => (200, "true".into()),
        "/QuickConnect/Initiate" => (200, quick_connect_json(false)),
        "/QuickConnect/Connect" if received.query == format!("secret={QC_SECRET}") => {
            let approved = polls.fetch_add(1, Ordering::SeqCst) >= polls_before_approval;
            (200, quick_connect_json(approved))
        }
        "/Users/AuthenticateWithQuickConnect" if received.body.contains(QC_SECRET) => {
            (200, auth_result())
        }
        _ => (404, "Unknown secret".into()),
    }
}

/// The Quick Connect wait's time in a test: a pause moves the clock on at once, so a test counts checks and never waits for real
/// time. Pauses from the `hold_from`th on (counted from 1) wait for [`TestClock::release`].
struct TestClock {
    now: Mutex<Duration>,
    pauses: Mutex<Vec<Duration>>,
    started: watch::Sender<usize>,
    hold_from: watch::Sender<usize>,
}

impl TestClock {
    fn new() -> Self {
        Self::holding_from(usize::MAX)
    }

    fn holding_from(pause: usize) -> Self {
        Self {
            now: Mutex::new(Duration::ZERO),
            pauses: Mutex::new(Vec::new()),
            started: watch::Sender::new(0),
            hold_from: watch::Sender::new(pause),
        }
    }

    /// The pauses the wait asked for, in order.
    fn pauses(&self) -> Vec<Duration> {
        self.pauses.lock().unwrap().clone()
    }

    /// Returns once `count` pauses have started.
    async fn paused(&self, count: usize) {
        let _ = self.started.subscribe().wait_for(|&n| n >= count).await;
    }

    fn release(&self) {
        self.hold_from.send_replace(usize::MAX);
    }
}

impl WaitClock for TestClock {
    fn now(&self) -> Duration {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, duration: Duration) {
        self.pauses.lock().unwrap().push(duration);
        let mut number = 0;
        self.started.send_modify(|started| {
            *started += 1;
            number = *started;
        });
        let _ = self
            .hold_from
            .subscribe()
            .wait_for(|&from| number < from)
            .await;
        *self.now.lock().unwrap() += duration;
    }
}

#[tokio::test]
async fn quick_connect_polls_until_approved_then_signs_in() {
    let server = StandIn::start(quick_connect_server(2)).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    assert_eq!(client.quick_connect_enabled().await, Ok(true));
    let code = client.quick_connect_initiate().await.unwrap();
    assert_eq!(code.code, "123456");
    assert_eq!(code.secret, QuickConnectSecret::new(QC_SECRET));
    let signed_in = client
        .quick_connect_wait(
            &code.secret,
            Duration::from_millis(20),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(signed_in.token, AccessToken::new(TOKEN));
    assert_eq!(
        server.paths(),
        [
            "/QuickConnect/Enabled",
            "/QuickConnect/Initiate",
            "/QuickConnect/Connect",
            "/QuickConnect/Connect",
            "/QuickConnect/Connect",
            "/Users/AuthenticateWithQuickConnect",
        ]
    );
    assert_no_secret(&log, &format!("{code:?} {signed_in:?}"));
    assert!(
        log.to_text()
            .contains("GET /QuickConnect/Connect?secret=[redacted] -> 200")
    );
}

#[tokio::test]
async fn quick_connect_turned_off_is_reported() {
    let server = StandIn::start(|received: &Received| match received.path.as_str() {
        "/QuickConnect/Enabled" => (200, "false".into()),
        _ => (401, "Quick connect is disabled".into()),
    })
    .await;
    let client = client(&server, &Arc::new(RequestLog::default()));
    assert_eq!(client.quick_connect_enabled().await, Ok(false));
    assert_eq!(
        client.quick_connect_initiate().await,
        Err(AuthError::QuickConnectDisabled)
    );
}

#[tokio::test]
async fn quick_connect_expires_when_the_server_forgets_or_time_runs_out() {
    // The server forgot the request (10 minutes passed, or it restarted): 404, at the first check.
    let server = StandIn::start(|_: &Received| (404, "Unknown secret".into())).await;
    let client = client(&server, &Arc::new(RequestLog::default()));
    let secret = QuickConnectSecret::new(QC_SECRET);
    let clock = TestClock::new();
    assert_eq!(
        client
            .quick_connect_wait_on(
                &clock,
                &secret,
                Duration::from_millis(10),
                Duration::from_secs(5)
            )
            .await,
        Err(AuthError::QuickConnectExpired)
    );
    assert_eq!(server.paths(), ["/QuickConnect/Connect"]);
    assert!(clock.pauses().is_empty());

    // Never approved: the wait ends at its lifetime. Checks at 0, 20, 40, 60 and 80 ms; the next would be at the lifetime.
    let server = StandIn::start(quick_connect_server(usize::MAX)).await;
    let client = self::client(&server, &Arc::new(RequestLog::default()));
    let clock = TestClock::new();
    let interval = Duration::from_millis(20);
    let result = client
        .quick_connect_wait_on(&clock, &secret, interval, Duration::from_millis(100))
        .await;
    assert_eq!(result, Err(AuthError::QuickConnectExpired));
    assert_eq!(server.paths(), ["/QuickConnect/Connect"; 5]);
    assert_eq!(clock.pauses(), [interval; 4]);
}

#[tokio::test]
async fn dropping_the_wait_stops_polling() {
    let server = StandIn::start(quick_connect_server(usize::MAX)).await;
    let client = client(&server, &Arc::new(RequestLog::default()));
    let secret = QuickConnectSecret::new(QC_SECRET);
    // The third pause holds: three checks were answered by then, and none is on the wire.
    let clock = TestClock::holding_from(3);
    let wait = client.quick_connect_wait_on(
        &clock,
        &secret,
        Duration::from_millis(20),
        Duration::from_secs(60),
    );
    tokio::select! {
        result = wait => panic!("the wait ended: {result:?}"),
        () = clock.paused(3) => {}
    }
    assert_eq!(server.paths(), ["/QuickConnect/Connect"; 3]);
    // Dropped while held. Released, polling that outlived the future would go on at once: give it time to reach the server.
    clock.release();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.paths().len(), 3);
}

#[tokio::test]
async fn quick_connect_rides_out_a_server_restart() {
    // Two checks fail with 503 (the server restarting), then the request is approved.
    let failures = Mutex::new(2);
    let approve = quick_connect_server(0);
    let server = StandIn::start(move |received: &Received| {
        let mut failures = failures.lock().unwrap();
        if received.path == "/QuickConnect/Connect" && *failures > 0 {
            *failures -= 1;
            return (503, String::new());
        }
        approve(received)
    })
    .await;
    let client = client(&server, &Arc::new(RequestLog::default()));
    let secret = QuickConnectSecret::new(QC_SECRET);
    let clock = TestClock::new();
    let interval = Duration::from_millis(10);
    let signed_in = client
        .quick_connect_wait_on(&clock, &secret, interval, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(signed_in.token, AccessToken::new(TOKEN));
    assert_eq!(
        server.paths(),
        [
            "/QuickConnect/Connect",
            "/QuickConnect/Connect",
            "/QuickConnect/Connect",
            "/Users/AuthenticateWithQuickConnect",
        ]
    );
    assert_eq!(clock.pauses(), [interval; 2]);

    // A server that keeps failing: the wait ends at its lifetime with the last error. Checks at 0, 10, 20, 30, 40 and 50 ms.
    let server = StandIn::start(|_: &Received| (503, String::new())).await;
    let client = self::client(&server, &Arc::new(RequestLog::default()));
    let clock = TestClock::new();
    let result = client
        .quick_connect_wait_on(&clock, &secret, interval, Duration::from_millis(60))
        .await;
    assert_eq!(
        result,
        Err(AuthError::Network(NetError::Status {
            status: 503,
            message: None
        }))
    );
    assert_eq!(server.paths(), ["/QuickConnect/Connect"; 6]);
    assert_eq!(clock.pauses(), [interval; 5]);
}

#[tokio::test]
async fn the_current_user_needs_a_valid_token() {
    let server = StandIn::start(|received: &Received| {
        if received
            .authorization
            .contains(&format!("Token=\"{TOKEN}\""))
        {
            (200, user_json())
        } else {
            (401, String::new())
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    assert_eq!(client.current_user().await, Err(AuthError::NotSignedIn));
    assert!(server.received().is_empty());

    client.set_token(AccessToken::new(TOKEN));
    let user = client.current_user().await.unwrap();
    assert_eq!(user.name, "Example User");
    assert!(
        server.received()[0]
            .authorization
            .ends_with(&format!(", Token=\"{TOKEN}\""))
    );
    assert_no_secret(&log, &format!("{user:?} {client:?}"));
}

#[tokio::test]
async fn without_a_hook_a_401_forgets_the_token_but_a_403_does_not() {
    let status = Arc::new(Mutex::new(403));
    let server = StandIn::start({
        let status = status.clone();
        move |_: &Received| (*status.lock().unwrap(), String::new())
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    client.set_token(AccessToken::new(TOKEN));

    assert_eq!(client.current_user().await, Err(AuthError::Refused));
    assert!(client.has_token());

    *status.lock().unwrap() = 401;
    let result = client.current_user().await;
    assert_eq!(result, Err(AuthError::SessionRevoked));
    assert!(!client.has_token());
    assert_no_secret(&log, &format!("{result:?} {client:?}"));

    // Without a token, nothing is sent.
    assert_eq!(client.current_user().await, Err(AuthError::NotSignedIn));
    assert_eq!(server.received().len(), 2);
}

/// A hook that records the events it hears.
fn recorder(client: &JellyfinClient) -> Arc<Mutex<Vec<SessionEvent>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    client.on_event({
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    });
    events
}

#[tokio::test]
async fn with_a_hook_a_401_keeps_the_token_until_the_app_decides() {
    let server = StandIn::start(|_: &Received| (401, String::new())).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let events = recorder(&client);
    client.set_token(AccessToken::new(TOKEN));

    let result = client.current_user().await;
    assert_eq!(result, Err(AuthError::Unconfirmed));
    assert!(client.has_token());
    assert_eq!(
        *events.lock().unwrap(),
        [SessionEvent::Unconfirmed {
            token: AccessToken::new(TOKEN),
            unauthorized: true,
        }]
    );
    assert_no_secret(&log, &format!("{result:?} {client:?} {events:?}"));

    // While the app checks, requests join the check: nothing is sent and the hook does not run again.
    assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
    assert_eq!(server.received().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);

    // Resumed (a gateway's answer): requests go out again and a new answer starts a new check.
    client.resume();
    assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
    assert_eq!(server.received().len(), 2);
    assert_eq!(events.lock().unwrap().len(), 2);

    // An older token is never revoked over a newer one.
    assert!(!client.revoke(&AccessToken::new("an-older-token")));
    assert!(client.has_token());
    // Confirmed as Jellyfin's 401: the token is forgotten.
    assert!(client.revoke(&AccessToken::new(TOKEN)));
    assert!(!client.has_token());
    assert_eq!(client.current_user().await, Err(AuthError::NotSignedIn));
}

#[tokio::test]
async fn a_page_or_a_redirect_where_json_was_expected_is_checked_too() {
    for status in [200, 403] {
        let server =
            StandIn::start(move |_: &Received| (status, "<html>sign in</html>".to_owned())).await;
        let client = client(&server, &Arc::new(RequestLog::default()));
        let events = recorder(&client);
        client.set_token(AccessToken::new(TOKEN));
        // The request answers as it would without a check: a page is no 401, so it can never end the session.
        let result = client.current_user().await;
        if status == 403 {
            assert_eq!(result, Err(AuthError::Refused));
        } else {
            assert!(
                matches!(result, Err(AuthError::Network(NetError::Decode { .. }))),
                "{result:?}"
            );
        }
        assert!(client.has_token());
        assert_eq!(
            *events.lock().unwrap(),
            [SessionEvent::Unconfirmed {
                token: AccessToken::new(TOKEN),
                unauthorized: false,
            }]
        );
        // Held back while the check runs; a new token ends the check.
        assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
        client.set_token(AccessToken::new("a-newer-token"));
        let _ = client.current_user().await;
        assert_eq!(server.received().len(), 2, "{status}");
    }
    // A plain 403 is Jellyfin refusing one request: no check.
    let server = StandIn::start(|_: &Received| (403, String::new())).await;
    let client = client(&server, &Arc::new(RequestLog::default()));
    let events = recorder(&client);
    client.set_token(AccessToken::new(TOKEN));
    assert_eq!(client.current_user().await, Err(AuthError::Refused));
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sign_out_tells_the_server_and_forgets_the_token_whatever_it_answers() {
    for (status, expected) in [
        (204, Ok(())),
        (401, Ok(())),
        (
            500,
            Err(AuthError::Network(NetError::Status {
                status: 500,
                message: None,
            })),
        ),
    ] {
        let server = StandIn::start(move |_: &Received| (status, String::new())).await;
        let client = client(&server, &Arc::new(RequestLog::default()));
        let events = recorder(&client);
        client.set_token(AccessToken::new(TOKEN));
        assert_eq!(client.sign_out().await, expected, "{status}");
        assert!(!client.has_token());
        assert!(events.lock().unwrap().is_empty());
        let request = &server.received()[0];
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/Sessions/Logout")
        );
        assert!(request.authorization.contains(TOKEN));
    }

    let client = JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        Arc::new(FixedAddress(Url::parse("http://127.0.0.1:1").unwrap())),
        identity(),
        Arc::new(RequestLog::default()),
    )
    .unwrap();
    client.set_token(AccessToken::new(TOKEN));
    assert_eq!(
        client.sign_out().await,
        Err(AuthError::Network(NetError::Unreachable))
    );
    assert!(!client.has_token());
}

#[tokio::test]
async fn authenticated_requests_carry_the_token_only_in_the_header() {
    let server = StandIn::start(|_: &Received| (200, "{}".into())).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    client.set_token(AccessToken::new(TOKEN));
    client
        .send_authenticated(Request::get("/Items").query("limit", "5"))
        .await
        .unwrap();
    let request = &server.received()[0];
    assert_eq!(request.query, "limit=5");
    assert!(request.authorization.contains(TOKEN));
    assert_no_secret(&log, "");
}
