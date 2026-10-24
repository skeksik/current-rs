//! Sign-in, restore, the 401 rule, sign out and removing a server, end to end through `App`, against a hand-written Jellyfin
//! stand-in on 127.0.0.1 (ADR 0015). Settings go to a temporary folder, secrets to memory.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_app::{AddServerState, App, NewServer, Removal, SignIn, SignInError, SignInState};
use current_jellyfin::AuthError;
use current_jellyfin::ProbeError;
use current_model::{
    GatewayKind, Permission, Reachability, ServerAddress, ServerKey, ServerRef, ServiceType, UserId,
};
use current_net::NetError;
use current_store::{
    GatewayKey, GatewaySecret, MemorySecrets, Secret, SecretKey, Secrets, SecretsError,
    SettingsStore, StoreError, get_gateway_secret, set_gateway_secret,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SERVER_ID: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ID: &str = "fedcba9876543210fedcba9876543210";
const USER_ID: &str = "5d1c0000000000000000000000000001";
const OTHER_USER_ID: &str = "5d1c0000000000000000000000000002";
const PASSWORD: &str = "pa55-word-value";
const WRONG_PASSWORD: &str = "wr0ng-pa55-value";
const QC_SECRET: &str = "5ecre7000000000000000000000000000000000000000000000000000000beef";

/// One request as the stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    path: String,
    authorization: String,
    body: String,
}

/// What the stand-in Jellyfin server knows.
#[derive(Default)]
struct Server {
    id: String,
    /// Live tokens and their user's id.
    tokens: Mutex<HashMap<String, String>>,
    issued: AtomicUsize,
    /// `/Users/Me` answers 403 (the token is valid, the request refused).
    refuse: AtomicBool,
    /// Sign-ins report this server id instead of the server's own.
    report_id: Mutex<Option<String>>,
    /// `/Sessions/Logout` waits for this before it answers.
    hold_logout: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// Quick Connect state checks answered "not yet" before the approval.
    polls_before_approval: AtomicUsize,
    received: Mutex<Vec<Received>>,
}

impl Server {
    fn info(id: &str) -> String {
        format!(
            r#"{{"LocalAddress":"http://192.168.1.10:8096","ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{id}","StartupWizardCompleted":true}}"#
        )
    }

    fn user(id: &str) -> String {
        let name = if id == USER_ID {
            "Example User"
        } else {
            "Another User"
        };
        format!(
            r#"{{"Name":"{name}","Id":"{id}","Policy":{{"IsAdministrator":true,"EnableMediaPlayback":true}}}}"#
        )
    }

    /// A new token for `user`. As Jellyfin does for one device id, the user's earlier tokens are logged out.
    fn issue(&self, user: &str) -> String {
        let token = format!("t0ken{:027}", self.issued.fetch_add(1, Ordering::SeqCst));
        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|_, owner| owner != user);
        tokens.insert(token.clone(), user.to_owned());
        let id = self
            .report_id
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(self.id.clone());
        format!(
            r#"{{"User":{},"SessionInfo":{{}},"AccessToken":"{token}","ServerId":"{id}"}}"#,
            Self::user(user)
        )
    }

    fn token_user(&self, received: &Received) -> Option<String> {
        let token = Self::token(received)?;
        self.tokens.lock().unwrap().get(&token).cloned()
    }

    fn revoke_user(&self, user: &str) {
        self.tokens.lock().unwrap().retain(|_, owner| owner != user);
    }

    fn token(received: &Received) -> Option<String> {
        let (_, rest) = received.authorization.split_once("Token=\"")?;
        Some(rest.split('"').next()?.to_owned())
    }

    fn authorized(&self, received: &Received) -> bool {
        Self::token(received).is_some_and(|token| self.tokens.lock().unwrap().contains_key(&token))
    }

    fn answer(&self, received: &Received, id: Option<String>) -> (u16, String) {
        match received.path.as_str() {
            "/System/Info/Public" => (200, Self::info(id.as_deref().unwrap_or(&self.id))),
            "/Users/AuthenticateByName" if received.body.contains(PASSWORD) => {
                let user = if received.body.contains("Another User") {
                    OTHER_USER_ID
                } else {
                    USER_ID
                };
                (200, self.issue(user))
            }
            "/Users/AuthenticateByName" => (401, "Error processing request.".into()),
            "/QuickConnect/Enabled" => (200, "true".into()),
            "/QuickConnect/Initiate" => (200, quick_connect(false)),
            "/QuickConnect/Connect" => {
                let left = self.polls_before_approval.load(Ordering::SeqCst);
                if left > 0 {
                    self.polls_before_approval.store(left - 1, Ordering::SeqCst);
                }
                (200, quick_connect(left == 0))
            }
            "/Users/AuthenticateWithQuickConnect" if received.body.contains(QC_SECRET) => {
                (200, self.issue(USER_ID))
            }
            "/Users/Me" if self.authorized(received) && self.refuse.load(Ordering::SeqCst) => {
                (403, String::new())
            }
            "/Users/Me" if self.authorized(received) => (
                200,
                Self::user(&self.token_user(received).unwrap_or_default()),
            ),
            "/Sessions/Logout" if self.authorized(received) => {
                if let Some(token) = Self::token(received) {
                    self.tokens.lock().unwrap().remove(&token);
                }
                (204, String::new())
            }
            "/Users/Me" | "/Sessions/Logout" => (401, String::new()),
            _ => (404, String::new()),
        }
    }

    fn paths(&self) -> Vec<String> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.path.clone())
            .collect()
    }

    fn with_token(&self) -> usize {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.authorization.contains("Token="))
            .count()
    }
}

fn quick_connect(authenticated: bool) -> String {
    format!(
        r#"{{"Authenticated":{authenticated},"Secret":"{QC_SECRET}","Code":"654321","DeviceId":"d","DeviceName":"Office PC","AppName":"Current","AppVersion":"0.1.0","DateAdded":"2026-10-22T10:00:00Z"}}"#
    )
}

/// Starts the stand-in; returns its state and address.
async fn start(id: &str) -> (Arc<Server>, String) {
    let server = Arc::new(Server {
        id: id.to_owned(),
        ..Server::default()
    });
    let (_, address) = listen(&server).await;
    (server, address)
}

/// One address of a stand-in: several can lead to the same server state, as a LAN address and a public name do.
#[derive(Default)]
struct Endpoint {
    /// Answers the identity probe with this server id instead of the server's own: another server at this address.
    id: Mutex<Option<String>>,
    /// Reads each request, records it and closes the connection without an answer.
    down: AtomicBool,
    /// Reads requests and never answers.
    hang: AtomicBool,
    /// Never answers `/Users/Me`.
    hang_me: AtomicBool,
    received: Mutex<Vec<Received>>,
}

impl Endpoint {
    fn with_token(&self) -> usize {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.authorization.contains("Token="))
            .count()
    }
}

/// Opens another address for `server`; returns it with its own state.
async fn listen(server: &Arc<Server>) -> (Arc<Endpoint>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let endpoint = Arc::new(Endpoint::default());
    let state = server.clone();
    let at = endpoint.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let state = state.clone();
            let at = at.clone();
            tokio::spawn(async move {
                let Some(received) = read_request(&mut socket).await else {
                    return;
                };
                at.received.lock().unwrap().push(received.clone());
                if at.down.load(Ordering::SeqCst) {
                    return;
                }
                state.received.lock().unwrap().push(received.clone());
                if at.hang.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                // `/Users/Me` waits while `hang_me` is set, then answers.
                while received.path == "/Users/Me" && at.hang_me.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                if received.path == "/Sessions/Logout" {
                    let hold = state.hold_logout.lock().unwrap().clone();
                    if let Some(hold) = hold {
                        hold.notified().await;
                    }
                }
                let id = at.id.lock().unwrap().clone();
                let (status, body) = state.answer(&received, id);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (endpoint, address)
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
    let target = head.lines().next()?.split(' ').nth(1)?;
    Some(Received {
        path: target.split('?').next()?.to_owned(),
        authorization: header("authorization"),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

fn jellyfin(key: &str) -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key))
}

fn secret_key() -> SecretKey {
    SecretKey::new(jellyfin("1"), UserId::new(USER_ID))
}

fn open(dir: &Path, secrets: &Arc<MemorySecrets>) -> App {
    let secrets: Arc<dyn Secrets> = secrets.clone();
    App::open(dir, secrets, "Office PC").unwrap().0
}

/// Runs server setup on `address` and returns the server it found.
async fn found(app: &App, address: &str) -> NewServer {
    let setup = app.add_server();
    setup.probe(address).await;
    let state = setup.state().borrow().clone();
    let AddServerState::Ready { server } = state else {
        panic!("{state:?}")
    };
    server
}

/// Waits (at most 2 s) until `check` passes.
async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 2 s");
}

/// Everything a user could paste into a bug report, or that the UI holds: none of it may contain a secret.
fn assert_nothing_secret(app: &App, dir: &Path, extra: &str, server: &Server) {
    let settings = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let text = format!(
        "{}\n{settings}\n{extra}\n{app:?}\n{:?}",
        app.diagnostics_text(),
        *app.capabilities().borrow()
    );
    let mut secrets: Vec<String> = (0..server.issued.load(Ordering::SeqCst))
        .map(|n| format!("t0ken{n:027}"))
        .collect();
    secrets.extend([
        PASSWORD.to_owned(),
        WRONG_PASSWORD.to_owned(),
        QC_SECRET.to_owned(),
    ]);
    for secret in secrets {
        assert!(!text.contains(&secret), "{secret} in {text}");
    }
}

async fn signed_in_with_password(app: &App, address: &str) -> SignIn {
    let flow = app.sign_in(found(app, address).await).unwrap();
    flow.with_password("Example User", PASSWORD).await;
    flow
}

#[tokio::test]
async fn a_password_sign_in_saves_the_server_the_user_and_the_token() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let flow = app.sign_in(found(&app, &address).await).unwrap();
    let mut states = vec![format!("{:?}", *flow.state().borrow())];
    let mut rx = flow.state();
    let collect = tokio::spawn(async move {
        let mut seen = Vec::new();
        while rx.changed().await.is_ok() {
            seen.push(format!("{:?}", *rx.borrow_and_update()));
        }
        seen
    });
    flow.with_password("Example User", PASSWORD).await;

    let state = flow.state().borrow().clone();
    let SignInState::SignedIn {
        server: saved,
        user,
        remembered,
    } = &state
    else {
        panic!("{state:?}")
    };
    assert_eq!(saved, &jellyfin("1"));
    assert_eq!(user.server, jellyfin("1"));
    assert_eq!(user.name, "Example User");
    assert!(user.has_permission(&Permission::ADMIN));
    assert!(remembered);

    // The settings hold the server, its address and the active user; the keyring holds the token.
    let settings = app.settings().unwrap().get();
    assert_eq!(settings.servers.len(), 1);
    let record = &settings.servers[0];
    assert_eq!(record.server, jellyfin("1"));
    assert_eq!(record.id.as_str(), SERVER_ID);
    assert_eq!(record.name, "Test Server");
    assert_eq!(record.addresses[0].url, address);
    assert_eq!(record.users[0].name, "Example User");
    assert_eq!(record.active_user, Some(UserId::new(USER_ID)));
    let token = secrets.get(&secret_key()).unwrap().unwrap();
    assert!(server.tokens.lock().unwrap().contains_key(token.expose()));

    // The capabilities state shows the server signed in, and its client works.
    let capabilities = app.capabilities().borrow().clone();
    let status = capabilities.server(&jellyfin("1")).unwrap();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.version.as_deref(), Some("12.2.0"));
    let client = app.jellyfin(&jellyfin("1")).unwrap();
    assert_eq!(client.current_user().await.unwrap().name, "Example User");

    drop(flow);
    states.extend(collect.await.unwrap());
    assert!(states.iter().any(|s| s == "SigningIn"), "{states:?}");
    assert_nothing_secret(&app, temp.path(), &states.join("\n"), &server);
    assert!(
        app.diagnostics_text()
            .contains("jellyfin/setup POST /Users/AuthenticateByName -> 200")
    );
}

#[tokio::test]
async fn a_wrong_password_fails_and_saves_nothing() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.with_password("Example User", WRONG_PASSWORD).await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Auth(AuthError::WrongCredentials)
        }
    );
    assert!(app.settings().unwrap().get().servers.is_empty());
    assert!(app.capabilities().borrow().servers.is_empty());
    assert_nothing_secret(&app, temp.path(), &format!("{flow:?}"), &server);
    // The wrong password did reach the server, in the request body only.
    assert!(
        server.received.lock().unwrap()[1]
            .body
            .contains(WRONG_PASSWORD)
    );
}

#[tokio::test]
async fn quick_connect_shows_the_code_then_signs_in() {
    let (server, address) = start(SERVER_ID).await;
    server.polls_before_approval.store(2, Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let mut flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.set_quick_connect_timing(Duration::from_millis(20), Duration::from_secs(60));
    flow.check_quick_connect().await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Ready {
            quick_connect: Some(true)
        }
    );

    let flow = Arc::new(flow);
    let mut rx = flow.state();
    let task = tokio::spawn({
        let flow = flow.clone();
        async move { flow.with_quick_connect().await }
    });
    let waiting = rx
        .wait_for(|state| matches!(state, SignInState::QuickConnectWaiting { .. }))
        .await
        .unwrap()
        .clone();
    assert_eq!(
        waiting,
        SignInState::QuickConnectWaiting {
            code: "654321".into()
        }
    );
    task.await.unwrap();
    let state = flow.state().borrow().clone();
    assert!(
        matches!(
            state,
            SignInState::SignedIn {
                remembered: true,
                ..
            }
        ),
        "{state:?}"
    );
    assert!(secrets.get(&secret_key()).unwrap().is_some());
    assert_nothing_secret(
        &app,
        temp.path(),
        &format!("{state:?} {waiting:?}"),
        &server,
    );
}

#[tokio::test]
async fn cancelling_quick_connect_stops_polling() {
    let (server, address) = start(SERVER_ID).await;
    server
        .polls_before_approval
        .store(usize::MAX, Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let mut flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.set_quick_connect_timing(Duration::from_millis(20), Duration::from_secs(60));
    let flow = Arc::new(flow);
    let task = tokio::spawn({
        let flow = flow.clone();
        async move { flow.with_quick_connect().await }
    });
    eventually(|| {
        server
            .paths()
            .iter()
            .filter(|p| *p == "/QuickConnect/Connect")
            .count()
            >= 2
    })
    .await;
    flow.cancel();
    task.await.unwrap();
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Ready {
            quick_connect: None
        }
    );
    let polls = server.paths().len();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.paths().len(), polls);
    assert!(app.settings().unwrap().get().servers.is_empty());
}

#[tokio::test]
async fn dropping_a_quick_connect_sign_in_stops_polling() {
    let (server, address) = start(SERVER_ID).await;
    server
        .polls_before_approval
        .store(usize::MAX, Ordering::SeqCst);
    let app = App::new();
    let mut flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.set_quick_connect_timing(Duration::from_millis(20), Duration::from_secs(60));
    let flow = Arc::new(flow);
    let task = tokio::spawn({
        let flow = flow.clone();
        async move { flow.with_quick_connect().await }
    });
    eventually(|| {
        server
            .paths()
            .iter()
            .filter(|p| *p == "/QuickConnect/Connect")
            .count()
            >= 2
    })
    .await;
    task.abort();
    let _ = task.await;
    drop(flow);
    let polls = server.paths().len();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.paths().len(), polls);
}

#[tokio::test]
async fn an_unapproved_quick_connect_code_expires() {
    let (server, address) = start(SERVER_ID).await;
    server
        .polls_before_approval
        .store(usize::MAX, Ordering::SeqCst);
    let app = App::new();
    let mut flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.set_quick_connect_timing(Duration::from_millis(20), Duration::from_millis(100));
    flow.with_quick_connect().await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Auth(AuthError::QuickConnectExpired)
        }
    );
}

#[tokio::test]
async fn a_restart_restores_the_session() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    drop(app);

    let app = open(temp.path(), &secrets);
    // Before the check: signed in as the saved user, reachability unknown, no live client yet.
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Unknown);
    assert_eq!(status.user.unwrap().name, "Example User");
    assert!(app.jellyfin(&jellyfin("1")).is_none());

    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.version.as_deref(), Some("12.2.0"));
    assert!(status.user.unwrap().has_permission(&Permission::ADMIN));
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    assert_eq!(status.address, Some(1));
    // The provider's identity probe ran before the token was sent.
    let paths = server.paths();
    let probe = paths
        .iter()
        .rposition(|p| p == "/System/Info/Public")
        .unwrap();
    let me = paths.iter().rposition(|p| p == "/Users/Me").unwrap();
    assert!(probe < me, "{paths:?}");
    assert!(
        app.diagnostics_text()
            .contains("jellyfin/1 address 1 GET /Users/Me -> 200")
    );
    assert_nothing_secret(&app, temp.path(), "", &server);
}

#[tokio::test]
async fn a_revoked_session_is_signed_out_at_restore() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    drop(app);
    server.revoke_user(USER_ID);

    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(!status.signed_in);
    assert_eq!(status.user, None);
    eventually(|| secrets.get(&secret_key()).unwrap().is_none()).await;
    eventually(|| {
        app.settings().unwrap().get().servers[0]
            .active_user
            .is_none()
    })
    .await;
    // The server and user stay saved for the next sign-in.
    let saved = &app.settings().unwrap().get().servers[0];
    assert_eq!(saved.users.len(), 1);
    assert!(app.jellyfin(&jellyfin("1")).is_none());
}

#[tokio::test]
async fn an_unreachable_server_stays_signed_in_and_offline() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    app.settings()
        .unwrap()
        .update(|settings| settings.servers[0].addresses[0].url = "http://127.0.0.1:1".into())
        .unwrap();
    drop(app);

    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Unreachable);
    assert!(secrets.get(&secret_key()).unwrap().is_some());
    assert_nothing_secret(&app, temp.path(), "", &server);
}

#[tokio::test]
async fn no_token_goes_to_a_different_server() {
    let (server, address) = start(SERVER_ID).await;
    let (other, other_address) = start(OTHER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    app.settings()
        .unwrap()
        .update(|settings| settings.servers[0].addresses[0].url = other_address.clone())
        .unwrap();
    drop(app);

    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Unreachable);
    assert_eq!(other.paths(), ["/System/Info/Public"]);
    assert_eq!(other.with_token(), 0);
    assert!(secrets.get(&secret_key()).unwrap().is_some());
    drop(server);
}

#[tokio::test]
async fn a_401_mid_session_signs_out_and_deletes_the_token() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    let client = app.jellyfin(&jellyfin("1")).unwrap();
    let mut capabilities = app.capabilities();
    capabilities.borrow_and_update();

    server.revoke_user(USER_ID);
    assert_eq!(client.current_user().await, Err(AuthError::Unconfirmed));
    capabilities.changed().await.unwrap();
    let status = capabilities.borrow().servers[0].clone();
    assert!(!status.signed_in);
    assert_eq!(status.user, None);
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    eventually(|| secrets.get(&secret_key()).unwrap().is_none()).await;
    eventually(|| {
        app.settings().unwrap().get().servers[0]
            .active_user
            .is_none()
    })
    .await;
    // Persisted: a restart stays signed out.
    let reread = SettingsStore::open(temp.path()).unwrap().0.get();
    assert_eq!(reread.servers[0].active_user, None);
}

#[tokio::test]
async fn sign_out_tells_the_server_and_deletes_the_token() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;

    assert_eq!(app.sign_out(&jellyfin("1")).await, Ok(()));
    assert_eq!(
        server.paths().last().map(String::as_str),
        Some("/Sessions/Logout")
    );
    assert!(server.tokens.lock().unwrap().is_empty());
    assert!(secrets.get(&secret_key()).unwrap().is_none());
    let saved = &app.settings().unwrap().get().servers[0];
    assert_eq!(saved.active_user, None);
    assert_eq!(saved.users.len(), 1);
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(!status.signed_in);
    assert!(app.jellyfin(&jellyfin("1")).is_none());

    // Signing in again to the saved server reuses its key.
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Example User", PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(
        matches!(&state, SignInState::SignedIn { server, .. } if *server == jellyfin("1")),
        "{state:?}"
    );
    assert_eq!(app.settings().unwrap().get().servers.len(), 1);
}

#[tokio::test]
async fn sign_out_deletes_the_token_even_when_the_server_is_gone() {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    app.settings()
        .unwrap()
        .update(|settings| settings.servers[0].addresses[0].url = "http://127.0.0.1:1".into())
        .unwrap();
    drop(app);
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;

    // Restore found the server unreachable but kept the session: the server cannot be told, and the app signs out anyway.
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    assert_eq!(
        app.sign_out(&jellyfin("1")).await,
        Err(AuthError::Network(NetError::Unreachable))
    );
    assert!(secrets.get(&secret_key()).unwrap().is_none());
    assert_eq!(app.settings().unwrap().get().servers[0].active_user, None);
    assert!(!app.capabilities().borrow().servers[0].signed_in);
}

/// A keyring that is always locked.
#[derive(Debug)]
struct NoKeyring;

impl Secrets for NoKeyring {
    fn get(&self, _: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        Err(SecretsError::Unavailable)
    }
    fn set(&self, _: &SecretKey, _: &Secret) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }
    fn delete(&self, _: &SecretKey) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }
}

#[tokio::test]
async fn without_a_keyring_the_sign_in_lasts_for_the_run() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, _) = App::open(temp.path(), Arc::new(NoKeyring), "").unwrap();
    let flow = signed_in_with_password(&app, &address).await;
    let state = flow.state().borrow().clone();
    assert!(
        matches!(
            state,
            SignInState::SignedIn {
                remembered: false,
                ..
            }
        ),
        "{state:?}"
    );
    assert!(app.capabilities().borrow().servers[0].signed_in);
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    assert_nothing_secret(&app, temp.path(), &format!("{state:?}"), &server);
    // The empty device name became the platform's name.
    let request = server.received.lock().unwrap()[1].clone();
    assert!(
        request
            .authorization
            .contains(&format!("Device=\"{}\"", current_jellyfin::platform_name())),
        "{}",
        request.authorization
    );
    drop(app);

    // Next run: nothing to restore from, so signed out, with the user still saved.
    let (app, _) = App::open(temp.path(), Arc::new(NoKeyring), "").unwrap();
    app.restore_sessions().await;
    assert!(!app.capabilities().borrow().servers[0].signed_in);
    assert!(
        app.settings().unwrap().get().servers[0]
            .active_user
            .is_some()
    );
}

/// The stored token for `user` on server 1.
fn stored(secrets: &MemorySecrets, user: &str) -> Option<String> {
    secrets
        .get(&SecretKey::new(jellyfin("1"), UserId::new(user)))
        .unwrap()
        .map(|secret| secret.expose().to_owned())
}

/// Lets background work (the 401 hook's save) run.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn a_401_on_an_old_client_leaves_a_newer_session_alone() {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    let old = app.jellyfin(&jellyfin("1")).unwrap();

    // Signing in again as the same user: the server logs out the old token, the app keeps the new one.
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Example User", PASSWORD).await;
    assert!(matches!(
        *flow.state().borrow(),
        SignInState::SignedIn { .. }
    ));
    let token = stored(&secrets, USER_ID).unwrap();

    assert_eq!(old.current_user().await, Err(AuthError::Unconfirmed));
    settle().await;
    assert!(app.capabilities().borrow().servers[0].signed_in);
    assert_eq!(stored(&secrets, USER_ID), Some(token));
    assert_eq!(
        app.settings().unwrap().get().servers[0].active_user,
        Some(UserId::new(USER_ID))
    );
    let current = app.jellyfin(&jellyfin("1")).unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    assert_eq!(current.current_user().await.unwrap().name, "Example User");
}

#[tokio::test]
async fn a_401_on_another_users_old_client_leaves_the_new_user_alone() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    let first = app.jellyfin(&jellyfin("1")).unwrap();

    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Another User", PASSWORD).await;
    assert!(matches!(
        *flow.state().borrow(),
        SignInState::SignedIn { .. }
    ));

    server.revoke_user(USER_ID);
    assert_eq!(first.current_user().await, Err(AuthError::Unconfirmed));
    settle().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.user.unwrap().id, UserId::new(OTHER_USER_ID));
    assert!(stored(&secrets, OTHER_USER_ID).is_some());
    assert_eq!(
        app.settings().unwrap().get().servers[0].active_user,
        Some(UserId::new(OTHER_USER_ID))
    );
}

#[tokio::test]
async fn a_403_mid_session_changes_nothing() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    // The session's provider has selected its address.
    eventually(|| app.capabilities().borrow().servers[0].address == Some(1)).await;
    let before = app.capabilities().borrow().clone();
    let token = stored(&secrets, USER_ID);

    server.refuse.store(true, Ordering::SeqCst);
    let client = app.jellyfin(&jellyfin("1")).unwrap();
    assert_eq!(client.current_user().await, Err(AuthError::Refused));
    settle().await;
    assert_eq!(*app.capabilities().borrow(), before);
    assert_eq!(stored(&secrets, USER_ID), token);
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    assert!(
        app.settings().unwrap().get().servers[0]
            .active_user
            .is_some()
    );
}

#[tokio::test]
async fn signing_in_again_checks_the_server_before_sending_the_password() {
    let (server, address) = start(SERVER_ID).await;
    let (other, other_address) = start(OTHER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    app.sign_out(&jellyfin("1")).await.unwrap();
    app.settings()
        .unwrap()
        .update(|settings| settings.servers[0].addresses[0].url = other_address.clone())
        .unwrap();
    drop(app);

    let app = open(temp.path(), &secrets);
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Example User", PASSWORD).await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Probe(ProbeError::DifferentServer)
        }
    );
    flow.with_quick_connect().await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Probe(ProbeError::DifferentServer)
        }
    );
    // The provider checked the address once and skips it from then on: no password and no Quick Connect request reached it.
    assert_eq!(other.paths(), ["/System/Info/Public"]);
    assert_eq!(other.with_token(), 0);
    assert!(stored(&secrets, USER_ID).is_none());
    drop(server);
}

#[tokio::test]
async fn a_sign_in_that_reports_another_server_is_not_saved() {
    let (server, address) = start(SERVER_ID).await;
    *server.report_id.lock().unwrap() = Some(OTHER_ID.to_owned());
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let flow = signed_in_with_password(&app, &address).await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Probe(ProbeError::DifferentServer)
        }
    );
    assert!(app.settings().unwrap().get().servers.is_empty());
    assert!(stored(&secrets, USER_ID).is_none());
}

#[tokio::test]
async fn settings_from_a_newer_version_refuse_a_sign_in_before_it_starts() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("settings.json"),
        r#"{"version": 99, "device_id": "abc"}"#,
    )
    .unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let flow = app.sign_in(found(&app, &address).await).unwrap();
    flow.with_password("Example User", PASSWORD).await;
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Failed {
            error: SignInError::Save(StoreError::NewerVersion { found: 99 })
        }
    );
    assert_eq!(server.paths(), ["/System/Info/Public"]);
}

#[tokio::test]
async fn a_sign_out_does_not_undo_a_sign_in_that_finished_meanwhile() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = Arc::new(open(temp.path(), &secrets));
    signed_in_with_password(&app, &address).await;

    // The sign-out waits for the server (held below) while a new sign-in completes.
    *server.hold_logout.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
    let release = server.hold_logout.lock().unwrap().clone().unwrap();
    let sign_out = tokio::spawn({
        let app = app.clone();
        async move { app.sign_out(&jellyfin("1")).await }
    });
    eventually(|| server.paths().last().map(String::as_str) == Some("/Sessions/Logout")).await;
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Example User", PASSWORD).await;
    assert!(matches!(
        *flow.state().borrow(),
        SignInState::SignedIn { .. }
    ));
    release.notify_one();
    let _ = sign_out.await.unwrap();

    assert!(app.capabilities().borrow().servers[0].signed_in);
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    assert!(stored(&secrets, USER_ID).is_some());
    assert!(
        app.settings().unwrap().get().servers[0]
            .active_user
            .is_some()
    );
}

#[tokio::test]
async fn a_restore_while_signing_out_does_not_sign_back_in() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = Arc::new(open(temp.path(), &secrets));
    signed_in_with_password(&app, &address).await;

    // The server takes its time to answer the sign-out; a restore runs meanwhile.
    *server.hold_logout.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
    let release = server.hold_logout.lock().unwrap().clone().unwrap();
    let sign_out = tokio::spawn({
        let app = app.clone();
        async move { app.sign_out(&jellyfin("1")).await }
    });
    eventually(|| server.paths().last().map(String::as_str) == Some("/Sessions/Logout")).await;
    app.restore_sessions().await;
    release.notify_one();
    assert_eq!(sign_out.await.unwrap(), Ok(()));

    assert!(!app.capabilities().borrow().servers[0].signed_in);
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    assert!(stored(&secrets, USER_ID).is_none());
    assert_eq!(app.settings().unwrap().get().servers[0].active_user, None);
}

#[tokio::test]
async fn the_saved_servers_are_published_on_every_change() {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    let mut saved = app.saved_servers();
    assert!(saved.borrow_and_update().is_empty());

    signed_in_with_password(&app, &address).await;
    assert!(saved.has_changed().unwrap());
    let list = saved.borrow_and_update().clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].active_user, Some(UserId::new(USER_ID)));

    app.sign_out(&jellyfin("1")).await.unwrap();
    assert!(saved.has_changed().unwrap());
    assert_eq!(saved.borrow_and_update()[0].active_user, None);
    assert_eq!(*saved.borrow(), app.servers());
}

/// Saves `addresses` as server 1's list, in this order.
fn set_addresses(app: &App, addresses: &[&str]) {
    app.settings()
        .unwrap()
        .update(|settings| {
            settings.servers[0].addresses = addresses
                .iter()
                .map(|url| current_model::ServerAddress::new((*url).to_owned(), false))
                .collect();
        })
        .unwrap();
}

fn network(key: &str) -> current_model::NetworkFacts {
    current_model::NetworkFacts {
        connectivity: current_model::Connectivity::Full,
        network_key: Some(current_model::NetworkKey::new(key)),
        ..current_model::NetworkFacts::default()
    }
}

#[tokio::test]
async fn a_saved_server_restores_on_its_second_address_when_the_first_is_down() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    set_addresses(&app, &["http://127.0.0.1:1", &address]);
    drop(app);

    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.address, Some(2));
    assert_eq!(status.version.as_deref(), Some("12.2.0"));
    assert!(status.user.unwrap().has_permission(&Permission::ADMIN));
    // Address 1's probe may still be failing when address 2 wins (a refused connection takes about 2 s on Windows), so only
    // what went to address 2 is certain; nothing but probes can go to address 1.
    let text = app.diagnostics_text();
    assert!(!text.contains("address 1 GET /Users/Me"), "{text}");
    assert!(
        text.contains("jellyfin/1 address 2 GET /Users/Me -> 200"),
        "{text}"
    );
    assert_nothing_secret(&app, temp.path(), "", &server);
}

#[tokio::test]
async fn an_address_with_another_server_id_never_gets_the_token() {
    let (server, address) = start(SERVER_ID).await;
    let (other, other_address) = start(OTHER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    set_addresses(&app, &[&other_address, &address]);
    drop(app);

    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.address, Some(2));
    let client = app.jellyfin(&jellyfin("1")).unwrap();
    assert_eq!(client.current_user().await.unwrap().name, "Example User");
    assert_eq!(other.paths(), ["/System/Info/Public"]);
    assert_eq!(other.with_token(), 0);
    drop(server);
}

/// A stand-in server with two addresses; returns the server, each address's state and the addresses.
async fn two_addresses() -> (Arc<Server>, [(Arc<Endpoint>, String); 2]) {
    let server = Arc::new(Server {
        id: SERVER_ID.to_owned(),
        ..Server::default()
    });
    let first = listen(&server).await;
    let second = listen(&server).await;
    (server, [first, second])
}

#[tokio::test]
async fn a_network_change_races_again_and_the_session_continues() {
    let (_server, [(first_at, first), (second_at, second)]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    set_addresses(&app, &[&first, &second]);
    drop(app);

    let app = open(temp.path(), &secrets);
    app.network_changed(network("home"));
    app.restore_sessions().await;
    assert_eq!(app.capabilities().borrow().servers[0].address, Some(1));
    let client = app.jellyfin(&jellyfin("1")).unwrap();

    // The same network again is no change: the session goes on at address 1.
    app.network_changed(network("home"));
    assert_eq!(client.current_user().await.unwrap().name, "Example User");
    assert_eq!(second_at.with_token(), 0);

    // On another network the first address leads to another server. The session races again, skips it without sending the
    // token there and continues at the second address with the same client.
    *first_at.id.lock().unwrap() = Some(OTHER_ID.to_owned());
    let sent = first_at.with_token();
    app.network_changed(network("away"));
    assert_eq!(client.current_user().await.unwrap().name, "Example User");
    assert_eq!(first_at.with_token(), sent);
    assert_eq!(second_at.with_token(), 1);
    eventually(|| app.capabilities().borrow().servers[0].address == Some(2)).await;
    let current = app.jellyfin(&jellyfin("1")).unwrap();
    assert!(Arc::ptr_eq(&client, &current));
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
}

#[tokio::test]
async fn the_status_follows_reachability_and_the_user_is_checked_once_reached() {
    let (server, [(first_at, first), _]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    drop(app);

    // Down at the start: signed in, unreachable, the user not confirmed (no permissions known yet).
    first_at.down.store(true, Ordering::SeqCst);
    let sent = first_at.with_token();
    let app = open(temp.path(), &secrets);
    app.restore_sessions().await;
    // The probes reached the address but failed, so no request carried the token.
    assert!(first_at.received.lock().unwrap().len() > sent);
    assert_eq!(first_at.with_token(), sent);
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Unreachable);
    assert_eq!(status.address, None);
    assert!(status.user.unwrap().permissions.is_empty());
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    let checks = || server.paths().iter().filter(|p| *p == "/Users/Me").count();
    let before = checks();

    // Back: the next race reaches it, the status follows and the user is checked.
    first_at.down.store(false, Ordering::SeqCst);
    app.returned_to_foreground();
    eventually(|| {
        let status = app.capabilities().borrow().servers[0].clone();
        status.reachability == Reachability::Reachable
            && status.address == Some(1)
            && status
                .user
                .is_some_and(|user| user.has_permission(&Permission::ADMIN))
    })
    .await;
    assert_eq!(checks(), before + 1);

    // Gone again after a network change: unreachable, still signed in.
    first_at.down.store(true, Ordering::SeqCst);
    app.network_changed(network("away"));
    eventually(|| app.capabilities().borrow().servers[0].reachability == Reachability::Unreachable)
        .await;
    let status = app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert_eq!(status.address, None);
    // Reached again: the user was confirmed already, so no second check.
    first_at.down.store(false, Ordering::SeqCst);
    app.returned_to_foreground();
    eventually(|| app.capabilities().borrow().servers[0].address == Some(1)).await;
    settle().await;
    assert_eq!(checks(), before + 1);
}

#[tokio::test]
async fn signing_in_again_waits_for_an_address_in_signing_in_and_can_be_cancelled() {
    let (server, [(first_at, first), _]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    app.sign_out(&jellyfin("1")).await.unwrap();
    drop(app);

    // The address takes the connection and never answers: the provider's race waits for its probe.
    first_at.hang.store(true, Ordering::SeqCst);
    let before = first_at.received.lock().unwrap().len();
    let app = open(temp.path(), &secrets);
    let flow = Arc::new(app.sign_in_again(&jellyfin("1")).unwrap().unwrap());
    let task = tokio::spawn({
        let flow = flow.clone();
        async move { flow.with_password("Example User", PASSWORD).await }
    });
    eventually(|| first_at.received.lock().unwrap().len() == before + 1).await;
    assert_eq!(*flow.state().borrow(), SignInState::SigningIn);
    flow.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *flow.state().borrow(),
        SignInState::Ready {
            quick_connect: None
        }
    );
    // Only the probe reached the server: no password.
    let paths: Vec<String> = first_at.received.lock().unwrap()[before..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(paths, ["/System/Info/Public"]);
    drop(server);
}

#[tokio::test]
async fn a_dropped_restore_still_confirms_the_user() {
    let (_server, [(first_at, first), _]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    drop(app);

    // The restore is dropped while `/Users/Me` hangs.
    first_at.hang_me.store(true, Ordering::SeqCst);
    let app = open(temp.path(), &secrets);
    let cancelled = tokio::time::timeout(Duration::from_millis(300), app.restore_sessions()).await;
    assert!(cancelled.is_err());
    assert!(app.jellyfin(&jellyfin("1")).is_some());
    let me = || {
        first_at
            .received
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/Users/Me")
            .count()
    };
    let before = me();
    assert!(before >= 1, "the check asked before the drop");

    // Dropping the restore stopped only the waiting: the check it started gets the answer and confirms the user, with no new
    // request. A restore run meanwhile does not start a second check.
    app.restore_sessions().await;
    first_at.hang_me.store(false, Ordering::SeqCst);
    eventually(|| {
        app.capabilities().borrow().servers[0]
            .user
            .as_ref()
            .is_some_and(|user| user.has_permission(&Permission::ADMIN))
    })
    .await;
    assert_eq!(me(), before);
}

#[tokio::test]
async fn a_sign_in_that_adds_an_address_rebuilds_the_provider_and_the_status_follows_it() {
    let (_server, [(first_at, first), (second_at, second)]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    eventually(|| app.capabilities().borrow().servers[0].address == Some(1)).await;

    // Setup finds the same server at a second address; signing in there adds it to the saved server.
    signed_in_with_password(&app, &second).await;
    let saved = app.servers();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].addresses.len(), 2);
    let client = app.jellyfin(&jellyfin("1")).unwrap();

    // The session goes over both addresses: with the first one down, a network change moves it to the second, and the status
    // follows the new provider.
    first_at.down.store(true, Ordering::SeqCst);
    let sent = first_at.with_token();
    app.network_changed(network("away"));
    assert_eq!(client.current_user().await.unwrap().name, "Example User");
    eventually(|| {
        let status = app.capabilities().borrow().servers[0].clone();
        status.reachability == Reachability::Reachable && status.address == Some(2)
    })
    .await;
    assert_eq!(first_at.with_token(), sent);
    assert!(second_at.with_token() > 0);

    // A restore now (its list is the current one) keeps the same provider: the status stays on address 2.
    app.restore_sessions().await;
    settle().await;
    assert_eq!(app.capabilities().borrow().servers[0].address, Some(2));
}

const REMOVED: Removal = Removal::Removed {
    credentials_left: false,
};

#[tokio::test]
async fn removing_a_server_signs_out_every_user_and_forgets_it() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    // Two users signed in from this device, the second one active: each has a token in the keyring and on the server.
    signed_in_with_password(&app, &address).await;
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Another User", PASSWORD).await;
    assert!(matches!(
        *flow.state().borrow(),
        SignInState::SignedIn { .. }
    ));
    assert!(stored(&secrets, USER_ID).is_some() && stored(&secrets, OTHER_USER_ID).is_some());
    assert_eq!(server.tokens.lock().unwrap().len(), 2);
    let mut saved = app.saved_servers();
    saved.borrow_and_update();

    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));

    // Gone on this device at once: list, settings file, capabilities, session and every token.
    assert!(saved.has_changed().unwrap());
    assert!(saved.borrow_and_update().is_empty());
    assert!(app.servers().is_empty());
    assert!(app.settings().unwrap().get().servers.is_empty());
    assert!(app.capabilities().borrow().servers.is_empty());
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    assert!(stored(&secrets, USER_ID).is_none() && stored(&secrets, OTHER_USER_ID).is_none());
    // The server hears of both sessions in the background.
    eventually(|| server.tokens.lock().unwrap().is_empty()).await;
    assert_nothing_secret(&app, temp.path(), "", &server);

    // A restart does not bring it back, and the next server gets a new key.
    drop(app);
    let app = open(temp.path(), &secrets);
    assert!(app.servers().is_empty());
    app.restore_sessions().await;
    assert!(app.capabilities().borrow().servers.is_empty());
    assert_eq!(app.settings().unwrap().get().last_server_key, 1);
    signed_in_with_password(&app, &address).await;
    assert_eq!(app.servers()[0].server, jellyfin("2"));
}

#[tokio::test]
async fn removing_an_unknown_or_removed_server_does_nothing() {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    assert_eq!(
        app.remove_server(&jellyfin("1")).await,
        Ok(Removal::NotSaved)
    );
    signed_in_with_password(&app, &address).await;
    assert_eq!(
        app.remove_server(&jellyfin("9")).await,
        Ok(Removal::NotSaved)
    );
    assert_eq!(app.servers().len(), 1);
    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));
    assert_eq!(
        app.remove_server(&jellyfin("1")).await,
        Ok(Removal::NotSaved)
    );
}

#[tokio::test]
async fn removing_a_server_the_server_cannot_hear_still_removes_it() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    // The server takes its time to answer: the removal does not wait for it.
    *server.hold_logout.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
    let release = server.hold_logout.lock().unwrap().clone().unwrap();
    let removal = tokio::time::timeout(Duration::from_secs(2), app.remove_server(&jellyfin("1")))
        .await
        .unwrap();
    assert_eq!(removal, Ok(REMOVED));
    assert!(app.servers().is_empty());
    assert!(stored(&secrets, USER_ID).is_none());
    eventually(|| server.paths().last().map(String::as_str) == Some("/Sessions/Logout")).await;
    release.notify_one();
}

#[tokio::test]
async fn a_restore_while_removing_does_not_bring_the_server_back() {
    let (_server, [(first_at, first), _]) = two_addresses().await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &first).await;
    drop(app);

    // The restore waits on `/Users/Me` while the server is removed.
    first_at.hang_me.store(true, Ordering::SeqCst);
    let app = Arc::new(open(temp.path(), &secrets));
    let restore = tokio::spawn({
        let app = app.clone();
        async move { app.restore_sessions().await }
    });
    eventually(|| {
        first_at
            .received
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path == "/Users/Me")
    })
    .await;
    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));
    first_at.hang_me.store(false, Ordering::SeqCst);
    restore.await.unwrap();
    app.restore_sessions().await;
    settle().await;

    assert!(app.servers().is_empty());
    assert!(app.capabilities().borrow().servers.is_empty());
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    assert!(stored(&secrets, USER_ID).is_none());
}

#[tokio::test]
async fn a_sign_in_racing_a_removal_leaves_no_token_behind() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = Arc::new(open(temp.path(), &secrets));
    signed_in_with_password(&app, &address).await;
    app.sign_out(&jellyfin("1")).await.unwrap();

    // Whichever wins the session lock, the outcome is the same: either the sign-in saved first and its token is deleted with
    // the server, or it finds the server gone and saves nothing.
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    let sign_in = flow.with_password("Another User", PASSWORD);
    let first = jellyfin("1");
    let removal = app.remove_server(&first);
    let ((), removal) = tokio::join!(sign_in, removal);
    assert_eq!(removal, Ok(REMOVED));
    let state = flow.state().borrow().clone();
    assert!(
        matches!(
            state,
            SignInState::SignedIn { .. }
                | SignInState::Failed {
                    error: SignInError::ServerRemoved
                }
        ),
        "{state:?}"
    );
    assert!(app.servers().is_empty());
    assert!(app.capabilities().borrow().servers.is_empty());
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    assert!(stored(&secrets, OTHER_USER_ID).is_none());
    eventually(|| server.tokens.lock().unwrap().is_empty()).await;
}

#[tokio::test]
async fn a_sign_in_saved_just_before_the_removal_has_its_token_deleted_too() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    signed_in_with_password(&app, &address).await;
    let flow = app.sign_in_again(&jellyfin("1")).unwrap().unwrap();
    flow.with_password("Another User", PASSWORD).await;
    assert!(stored(&secrets, OTHER_USER_ID).is_some());

    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));
    assert!(stored(&secrets, USER_ID).is_none() && stored(&secrets, OTHER_USER_ID).is_none());
    eventually(|| server.tokens.lock().unwrap().is_empty()).await;
}

/// A keyring that reads and stores but cannot delete.
#[derive(Debug, Default)]
struct UndeletableKeyring(MemorySecrets);

impl Secrets for UndeletableKeyring {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        self.0.get(key)
    }
    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretsError> {
        self.0.set(key, secret)
    }
    fn delete(&self, _: &SecretKey) -> Result<(), SecretsError> {
        Err(SecretsError::Failed)
    }
}

#[tokio::test]
async fn a_keyring_that_cannot_delete_still_lets_the_server_go_and_says_so() {
    let (server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, _) = App::open(
        temp.path(),
        Arc::new(UndeletableKeyring::default()),
        "Office PC",
    )
    .unwrap();
    signed_in_with_password(&app, &address).await;
    assert_eq!(
        app.remove_server(&jellyfin("1")).await,
        Ok(Removal::Removed {
            credentials_left: true
        })
    );
    assert!(app.servers().is_empty());
    assert!(app.settings().unwrap().get().servers.is_empty());
    // The token left in the keyring no longer works: the server was told.
    eventually(|| server.tokens.lock().unwrap().is_empty()).await;
}

/// Signs in, then, with the app closed, marks the saved server's first address as having a Basic gateway secret and adds a
/// second address without kinds, and stores a gateway secret for each: the second stands for an orphan left by a crash before
/// the settings save. Returns the reopened app and both keys.
async fn with_gateway_secrets(
    dir: &Path,
    secrets: &Arc<MemorySecrets>,
    address: &str,
) -> (App, [GatewayKey; 2]) {
    signed_in_with_password(&open(dir, secrets), address).await;
    let (settings, _) = SettingsStore::open(dir).unwrap();
    settings
        .update(|s| {
            s.servers[0].addresses[0].gateway = vec![GatewayKind::BASIC];
            s.servers[0].add_address(ServerAddress::new("https://media.example.com", false));
        })
        .unwrap();
    let saved = &settings.get().servers[0];
    let keys =
        [0, 1].map(|index| GatewayKey::new(jellyfin("1"), saved.addresses[index].id.unwrap()));
    for key in &keys {
        set_gateway_secret(secrets.as_ref(), key, &GatewaySecret::new("x".repeat(3000))).unwrap();
    }
    drop(settings);
    (open(dir, secrets), keys)
}

#[tokio::test]
async fn removing_a_server_deletes_every_address_s_gateway_secret() {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let (app, keys) = with_gateway_secrets(temp.path(), &secrets, &address).await;

    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));
    for key in &keys {
        assert_eq!(get_gateway_secret(secrets.as_ref(), key), Ok(None));
    }
    assert!(secrets.accounts().is_empty(), "{:?}", secrets.accounts());
}

#[tokio::test]
async fn a_gateway_secret_that_cannot_be_deleted_counts_only_for_an_address_with_kinds() {
    for (failing, left) in [(0, true), (1, false)] {
        let (_server, address) = start(SERVER_ID).await;
        let temp = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::new());
        let (app, keys) = with_gateway_secrets(temp.path(), &secrets, &address).await;
        secrets.fail_accounts_starting_with(&format!("{}/", keys[failing].account()));

        assert_eq!(
            app.remove_server(&jellyfin("1")).await,
            Ok(Removal::Removed {
                credentials_left: left
            }),
            "failing address {failing}"
        );
        assert!(app.settings().unwrap().get().servers.is_empty());
        // The other address's secret is gone either way.
        assert_eq!(
            get_gateway_secret(secrets.as_ref(), &keys[1 - failing]),
            Ok(None)
        );
    }
}

#[tokio::test]
async fn settings_from_a_newer_version_refuse_a_removal() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("settings.json"),
        format!(
            concat!(
                r#"{{"version": 99, "device_id": "abc", "servers": [{{"service": "jellyfin", "key": "1", "#,
                r#""server_id": "{}", "addresses": [{{"url": "http://127.0.0.1:1"}}]}}]}}"#
            ),
            SERVER_ID
        ),
    )
    .unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let app = open(temp.path(), &secrets);
    assert_eq!(app.servers().len(), 1);
    assert_eq!(
        app.remove_server(&jellyfin("1")).await,
        Err(StoreError::NewerVersion { found: 99 })
    );
    assert_eq!(app.servers().len(), 1);
    assert_eq!(app.capabilities().borrow().servers.len(), 1);
}

/// A keyring whose first read waits until the test lets it go, then answers `Unavailable` (locked) or what it holds.
#[derive(Debug, Default)]
struct GatedKeyring {
    inner: MemorySecrets,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    entered: AtomicBool,
    locked: AtomicBool,
}

impl GatedKeyring {
    fn close(&self) -> std::sync::mpsc::Sender<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }
}

impl Secrets for GatedKeyring {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.store(true, Ordering::SeqCst);
            let _ = gate.recv();
        }
        if self.locked.load(Ordering::SeqCst) {
            return Err(SecretsError::Unavailable);
        }
        self.inner.get(key)
    }
    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretsError> {
        self.inner.set(key, secret)
    }
    fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.inner.delete(key)
    }
}

/// Signs in, then opens the folder again and starts a restore that waits in its keyring read while the server is removed. The
/// read then answers (a token, or `Unavailable` when `locked`) and the restore goes on against a server that is gone.
async fn restore_racing_removal(locked: bool) {
    let (_server, address) = start(SERVER_ID).await;
    let temp = tempfile::tempdir().unwrap();
    let keyring = Arc::new(GatedKeyring::default());
    let secrets: Arc<dyn Secrets> = keyring.clone();
    let (app, _) = App::open(temp.path(), secrets.clone(), "Office PC").unwrap();
    signed_in_with_password(&app, &address).await;
    drop(app);

    let release = keyring.close();
    let app = Arc::new(App::open(temp.path(), secrets, "Office PC").unwrap().0);
    let restore = tokio::spawn({
        let app = app.clone();
        async move { app.restore_sessions().await }
    });
    eventually(|| keyring.entered.load(Ordering::SeqCst)).await;
    assert_eq!(app.remove_server(&jellyfin("1")).await, Ok(REMOVED));
    keyring.locked.store(locked, Ordering::SeqCst);
    release.send(()).unwrap();
    restore.await.unwrap();
    settle().await;

    assert!(app.servers().is_empty());
    assert!(app.capabilities().borrow().servers.is_empty());
    assert!(app.jellyfin(&jellyfin("1")).is_none());
    keyring.locked.store(false, Ordering::SeqCst);
    assert!(keyring.get(&secret_key()).unwrap().is_none());
}

#[tokio::test]
async fn a_restore_that_read_the_token_before_the_removal_does_not_bring_the_server_back() {
    // The restore goes on to build its client, finds the server gone and would mark it unreachable.
    restore_racing_removal(false).await;
}

#[tokio::test]
async fn a_restore_whose_keyring_read_fails_after_the_removal_does_not_bring_the_server_back() {
    // The keyring answers `Unavailable`: the restore would mark the server signed out.
    restore_racing_removal(true).await;
}

/// A keyring with no backend on this platform: nothing was ever stored.
#[derive(Debug)]
struct NoBackend;

impl Secrets for NoBackend {
    fn get(&self, _: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        Err(SecretsError::Unavailable)
    }
    fn set(&self, _: &SecretKey, _: &Secret) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }
    fn delete(&self, _: &SecretKey) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }
    fn has_backend(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn an_unavailable_keyring_reports_credentials_left_only_where_there_is_a_backend() {
    let (_server, address) = start(SERVER_ID).await;
    for (secrets, left) in [
        (Arc::new(NoKeyring) as Arc<dyn Secrets>, true),
        (Arc::new(NoBackend) as Arc<dyn Secrets>, false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (app, _) = App::open(temp.path(), secrets, "Office PC").unwrap();
        signed_in_with_password(&app, &address).await;
        assert_eq!(
            app.remove_server(&jellyfin("1")).await,
            Ok(Removal::Removed {
                credentials_left: left
            })
        );
        assert!(app.servers().is_empty());
    }
}
