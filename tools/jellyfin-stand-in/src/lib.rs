//! A hand-written Jellyfin stand-in on 127.0.0.1, for the bridge tests (Rust, C# and Swift). It answers only what server setup,
//! sign-in, restoring a session and sign-out ask, with the shapes `current-jellyfin` reads. Every name, id and credential in it is
//! fake. Test code only: nothing shipped depends on it.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// The stand-in's server id.
pub const SERVER_ID: &str = "57a2d1e0000000000000000000000001";
/// The stand-in's server name.
pub const SERVER_NAME: &str = "Stand-in Server";
/// The one user that can sign in.
pub const USER_NAME: &str = "stand-in-user";
pub const USER_ID: &str = "57a2d1e0000000000000000000000002";
/// The one password that works. Obviously not a real one.
pub const PASSWORD: &str = "not-a-real-password";
/// The Quick Connect code the stand-in hands out. Its requests are approved at the first check.
pub const QUICK_CONNECT_CODE: &str = "123456";
/// The gateway's user name and password in [`Gate::Basic`].
pub const GATE_USER: &str = "stand-in-gate-user";
pub const GATE_PASSWORD: &str = "not-a-real-gate-password";
/// `stand-in-gate-user:not-a-real-gate-password` in base64.
const GATE_BASIC: &str = "c3RhbmQtaW4tZ2F0ZS11c2VyOm5vdC1hLXJlYWwtZ2F0ZS1wYXNzd29yZA==";
/// The realm of [`Gate::Basic`]'s challenge.
pub const GATE_REALM: &str = "Stand-in Gate";
/// The cookie [`Gate::Portal`] wants, and the value it takes until [`Controls::set_gate_cookie`] changes it.
pub const GATE_COOKIE: &str = "stand_in_gate";
pub const GATE_COOKIE_VALUE: &str = "not-a-real-gate-cookie";
/// Where [`Gate::Portal`]'s sign-in link goes: it sets the cookie.
pub const GATE_SIGN_IN_PATH: &str = "/gate/sign-in";
/// The header [`Gate::Header`] wants.
pub const GATE_HEADER: &str = "x-stand-in-gate";
pub const GATE_HEADER_VALUE: &str = "not-a-real-gate-header";
const QUICK_CONNECT_SECRET: &str =
    "57a2d1e0000000000000000000000000000000000000000000000000000000ff";

/// A running stand-in. It stops when dropped. Its controls ([`Controls`]) are reachable through it.
pub struct StandIn {
    controls: Controls,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StandIn").finish_non_exhaustive()
    }
}

/// An authentication gateway in front of the stand-in (ADR 0017), as a reverse proxy would answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gate {
    /// None.
    #[default]
    Off,
    /// HTTP Basic: 401 with a challenge without [`GATE_USER`]'s password. It does not pass its `Authorization` on, so Jellyfin's
    /// fields then come from `X-Emby-Authorization`.
    Basic,
    /// A portal: an HTML sign-in page without the [`GATE_COOKIE`] cookie. Its link, [`GATE_SIGN_IN_PATH`], sets the cookie and
    /// redirects back to `/`, as a portal does after a sign-in: for the sign-in window, by hand or in the Windows smoke run.
    Portal,
    /// 403 without the [`GATE_HEADER`] header.
    Header,
}

#[derive(Default)]
struct State {
    gate: Mutex<Gate>,
    /// The cookie value [`Gate::Portal`] takes; [`GATE_COOKIE_VALUE`] when unset.
    gate_cookie: Mutex<Option<String>>,
    /// Live tokens.
    tokens: Mutex<Vec<String>>,
    issued: AtomicUsize,
    quick_connect_off: AtomicBool,
    /// Quick Connect requests stay unapproved.
    quick_connect_pending: AtomicBool,
    /// Paths whose requests wait until released.
    held: Mutex<HashSet<String>>,
    released: Notify,
    /// The path of every request, in order.
    paths: Mutex<Vec<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Each change is one push or one replacement: a poisoned lock still holds a usable value.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StandIn {
    /// Starts listening on a free port of 127.0.0.1, on the current tokio runtime.
    ///
    /// # Errors
    /// The port could not be bound.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let state = Arc::new(State::default());
        let serving = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, serving.clone()));
            }
        });
        Ok(Self {
            controls: Controls { state, address },
            task,
        })
    }

    /// A handle that changes the stand-in from another thread.
    pub fn controls(&self) -> Controls {
        self.controls.clone()
    }
}

impl std::ops::Deref for StandIn {
    type Target = Controls;

    fn deref(&self) -> &Controls {
        &self.controls
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Changes a running stand-in and reads what it received, from any thread.
#[derive(Clone)]
pub struct Controls {
    state: Arc<State>,
    address: String,
}

impl std::fmt::Debug for Controls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controls").finish_non_exhaustive()
    }
}

impl Controls {
    /// Where it listens, e.g. `http://127.0.0.1:49152`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Ends every session, as an administrator revoking the device would: the next authenticated request gets 401.
    pub fn revoke_all(&self) {
        revoke_all(&self.state);
    }

    /// Turns Quick Connect off or on.
    pub fn set_quick_connect(&self, enabled: bool) {
        self.state
            .quick_connect_off
            .store(!enabled, Ordering::SeqCst);
    }

    /// While set, Quick Connect requests stay unapproved: the app keeps waiting and polling.
    pub fn set_quick_connect_pending(&self, pending: bool) {
        self.state
            .quick_connect_pending
            .store(pending, Ordering::SeqCst);
    }

    /// Requests to `path` (e.g. `/Users/AuthenticateByName`) wait, unanswered, until [`Controls::release`].
    pub fn hold(&self, path: &str) {
        lock(&self.state.held).insert(path.to_owned());
    }

    /// Lets held requests to `path` answer.
    pub fn release(&self, path: &str) {
        lock(&self.state.held).remove(path);
        self.state.released.notify_waiters();
    }

    /// The paths of the requests received so far, oldest first.
    pub fn paths(&self) -> Vec<String> {
        lock(&self.state.paths).clone()
    }

    /// How many requests to `path` arrived so far.
    pub fn count(&self, path: &str) -> usize {
        lock(&self.state.paths)
            .iter()
            .filter(|seen| *seen == path)
            .count()
    }

    /// Puts `gate` in front of the stand-in.
    pub fn set_gate(&self, gate: Gate) {
        *lock(&self.state.gate) = gate;
    }

    /// The cookie value [`Gate::Portal`] takes from now on (the gateway's session expired and a new one began).
    pub fn set_gate_cookie(&self, value: &str) {
        *lock(&self.state.gate_cookie) = Some(value.to_owned());
    }

    /// How many sessions are live.
    pub fn live_sessions(&self) -> usize {
        lock(&self.state.tokens).len()
    }
}

fn revoke_all(state: &State) {
    lock(&state.tokens).clear();
}

/// One request as the stand-in read it.
struct Received {
    path: String,
    authorization: String,
    emby_authorization: String,
    cookie: String,
    gate_header: String,
    body: String,
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(received) = read_request(&mut socket).await else {
        return;
    };
    lock(&state.paths).push(received.path.clone());
    loop {
        // Registered before the check, so a release between the two is not missed.
        let released = state.released.notified();
        if !lock(&state.held).contains(&received.path) {
            break;
        }
        released.await;
    }
    let response = match gate(&state, &received) {
        Some(blocked) => blocked,
        None => {
            let (status, body) = answer(&state, &received);
            format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    };
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The gateway's answer when it does not let `received` through.
fn gate(state: &State, received: &Received) -> Option<String> {
    let reply = |status: u16, header: &str, body: &str| {
        format!(
            "HTTP/1.1 {status} X\r\n{header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    match *lock(&state.gate) {
        Gate::Off => None,
        Gate::Basic => (received.authorization != format!("Basic {GATE_BASIC}")).then(|| {
            reply(
                401,
                &format!("www-authenticate: Basic realm=\"{GATE_REALM}\"\r\n"),
                "",
            )
        }),
        Gate::Portal => {
            let value = lock(&state.gate_cookie)
                .clone()
                .unwrap_or_else(|| GATE_COOKIE_VALUE.to_owned());
            let wanted = format!("{GATE_COOKIE}={value}");
            if received.path == GATE_SIGN_IN_PATH {
                // The page's link: "signs in" by setting the cookie (HttpOnly, as portals do) and going back to the start.
                return Some(reply(
                    302,
                    &format!("set-cookie: {wanted}; Path=/; HttpOnly\r\nlocation: /\r\n"),
                    "",
                ));
            }
            (!received.cookie.split("; ").any(|pair| pair == wanted)).then(|| {
                reply(
                    200,
                    "content-type: text/html\r\n",
                    &format!(
                        "<!DOCTYPE html><html><body><a href=\"{GATE_SIGN_IN_PATH}\">Sign in</a></body></html>"
                    ),
                )
            })
        }
        Gate::Header => (received.gate_header != GATE_HEADER_VALUE).then(|| reply(403, "", "")),
    }
}

fn answer(state: &State, received: &Received) -> (u16, String) {
    let quick_connect_on = !state.quick_connect_off.load(Ordering::SeqCst);
    match received.path.as_str() {
        "/System/Info/Public" => (200, info()),
        "/Users/AuthenticateByName"
            if received
                .body
                .contains(&format!(r#""Username":"{USER_NAME}""#))
                && received.body.contains(&format!(r#""Pw":"{PASSWORD}""#)) =>
        {
            (200, issue(state))
        }
        "/Users/AuthenticateByName" => (401, "Error processing request.".into()),
        "/QuickConnect/Enabled" => (200, quick_connect_on.to_string()),
        "/QuickConnect/Initiate" | "/QuickConnect/Connect" if !quick_connect_on => {
            (401, String::new())
        }
        "/QuickConnect/Initiate" => (200, quick_connect(false)),
        "/QuickConnect/Connect" => (
            200,
            quick_connect(!state.quick_connect_pending.load(Ordering::SeqCst)),
        ),
        "/Users/AuthenticateWithQuickConnect" if received.body.contains(QUICK_CONNECT_SECRET) => {
            (200, issue(state))
        }
        "/Users/Me" if authorized(state, received) => (200, user()),
        "/Sessions/Logout" if authorized(state, received) => {
            if let Some(token) = token(received) {
                lock(&state.tokens).retain(|live| *live != token);
            }
            (204, String::new())
        }
        "/Users/Me" | "/Sessions/Logout" | "/Users/AuthenticateWithQuickConnect" => {
            (401, String::new())
        }
        _ => (404, String::new()),
    }
}

fn info() -> String {
    format!(
        r#"{{"ServerName":"{SERVER_NAME}","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{SERVER_ID}","StartupWizardCompleted":true}}"#
    )
}

fn user() -> String {
    format!(
        r#"{{"Name":"{USER_NAME}","Id":"{USER_ID}","Policy":{{"IsAdministrator":false,"EnableMediaPlayback":true}}}}"#
    )
}

/// A new token. As Jellyfin does for one device id, the earlier one is logged out.
fn issue(state: &State) -> String {
    let token = format!(
        "57a2d1e0t0ken{:019}",
        state.issued.fetch_add(1, Ordering::SeqCst)
    );
    *lock(&state.tokens) = vec![token.clone()];
    format!(
        r#"{{"User":{},"SessionInfo":{{}},"AccessToken":"{token}","ServerId":"{SERVER_ID}"}}"#,
        user()
    )
}

fn quick_connect(authenticated: bool) -> String {
    format!(
        r#"{{"Authenticated":{authenticated},"Secret":"{QUICK_CONNECT_SECRET}","Code":"{QUICK_CONNECT_CODE}","DeviceId":"d","DeviceName":"Example PC","AppName":"Current","AppVersion":"0.1.0","DateAdded":"2026-10-22T10:00:00Z"}}"#
    )
}

fn token(received: &Received) -> Option<String> {
    // Jellyfin reads `X-Emby-Authorization` when `Authorization` carries no fields of its own (legacy authorization on, and a Basic
    // gateway that does not pass its header on).
    let fields = if received.authorization.starts_with("MediaBrowser") {
        &received.authorization
    } else {
        &received.emby_authorization
    };
    let (_, rest) = fields.split_once("Token=\"")?;
    Some(rest.split('"').next()?.to_owned())
}

fn authorized(state: &State, received: &Received) -> bool {
    token(received).is_some_and(|token| lock(&state.tokens).contains(&token))
}

async fn read_request(socket: &mut TcpStream) -> Option<Received> {
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
        emby_authorization: header("x-emby-authorization"),
        cookie: header("cookie"),
        gate_header: header(GATE_HEADER),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}
