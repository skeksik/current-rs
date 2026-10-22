//! Sign-in, restore, the 401 rule and sign out, end to end through `App`, against a hand-written Jellyfin stand-in on 127.0.0.1
//! (ADR 0015). Settings go to a temporary folder, secrets to memory.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_app::{AddServerState, App, NewServer, SignIn, SignInError, SignInState};
use current_jellyfin::AuthError;
use current_jellyfin::ProbeError;
use current_model::{Permission, Reachability, ServerKey, ServerRef, ServiceType, UserId};
use current_store::{
    MemorySecrets, Secret, SecretKey, Secrets, SecretsError, SettingsStore, StoreError,
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
    fn info(&self) -> String {
        format!(
            r#"{{"LocalAddress":"http://192.168.1.10:8096","ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{}","StartupWizardCompleted":true}}"#,
            self.id
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

    fn answer(&self, received: &Received) -> (u16, String) {
        match received.path.as_str() {
            "/System/Info/Public" => (200, self.info()),
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let state = server.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let state = state.clone();
            tokio::spawn(async move {
                let Some(received) = read_request(&mut socket).await else {
                    return;
                };
                state.received.lock().unwrap().push(received.clone());
                if received.path == "/Sessions/Logout" {
                    let hold = state.hold_logout.lock().unwrap().clone();
                    if let Some(hold) = hold {
                        hold.notified().await;
                    }
                }
                let (status, body) = state.answer(&received);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (server, address)
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
    // The identity probe ran before the token was sent.
    let paths = server.paths();
    let probe = paths
        .iter()
        .rposition(|p| p == "/System/Info/Public")
        .unwrap();
    let me = paths.iter().rposition(|p| p == "/Users/Me").unwrap();
    assert!(probe < me, "{paths:?}");
    assert!(
        app.diagnostics_text()
            .contains("jellyfin/1 GET /Users/Me -> 200")
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
    assert_eq!(client.current_user().await, Err(AuthError::SessionRevoked));
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

    // Restore found the server unreachable, so there is no live client: signing out is local.
    assert_eq!(
        app.sign_out(&jellyfin("1")).await,
        Err(AuthError::NotSignedIn)
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

    assert_eq!(old.current_user().await, Err(AuthError::SessionRevoked));
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
    assert_eq!(first.current_user().await, Err(AuthError::SessionRevoked));
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
    assert_eq!(
        other.paths(),
        ["/System/Info/Public", "/System/Info/Public"]
    );
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
