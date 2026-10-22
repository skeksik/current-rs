//! A hand-written Jellyfin stand-in on 127.0.0.1, for the bridge tests (Rust, C# and Swift). It answers only what server setup,
//! sign-in, restoring a session and sign-out ask, with the shapes `current-jellyfin` reads. Every name, id and credential in it is
//! fake. Test code only: nothing shipped depends on it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
const QUICK_CONNECT_SECRET: &str =
    "57a2d1e0000000000000000000000000000000000000000000000000000000ff";

/// A running stand-in. It stops when dropped.
pub struct StandIn {
    address: String,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StandIn").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    /// Live tokens.
    tokens: Mutex<Vec<String>>,
    issued: AtomicUsize,
    quick_connect_off: AtomicBool,
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
            address,
            state,
            task,
        })
    }

    /// Where it listens, e.g. `http://127.0.0.1:49152`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Ends every session, as an administrator revoking the device would: the next authenticated request gets 401.
    pub fn revoke_all(&self) {
        revoke_all(&self.state);
    }

    /// A handle that can revoke sessions from another thread.
    pub fn controls(&self) -> Controls {
        Controls {
            state: self.state.clone(),
            address: self.address.clone(),
        }
    }

    /// Turns Quick Connect off or on.
    pub fn set_quick_connect(&self, enabled: bool) {
        self.state
            .quick_connect_off
            .store(!enabled, Ordering::SeqCst);
    }

    /// The paths of the requests received so far, oldest first.
    pub fn paths(&self) -> Vec<String> {
        lock(&self.state.paths).clone()
    }

    /// How many sessions are live.
    pub fn live_sessions(&self) -> usize {
        lock(&self.state.tokens).len()
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Changes a running stand-in from outside its runtime.
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
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn revoke_all(&self) {
        revoke_all(&self.state);
    }

    pub fn set_quick_connect(&self, enabled: bool) {
        self.state
            .quick_connect_off
            .store(!enabled, Ordering::SeqCst);
    }
}

fn revoke_all(state: &State) {
    lock(&state.tokens).clear();
}

/// One request as the stand-in read it.
struct Received {
    path: String,
    authorization: String,
    body: String,
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(received) = read_request(&mut socket).await else {
        return;
    };
    lock(&state.paths).push(received.path.clone());
    let (status, body) = answer(&state, &received);
    let response = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
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
        "/QuickConnect/Connect" => (200, quick_connect(true)),
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
    let (_, rest) = received.authorization.split_once("Token=\"")?;
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
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}
