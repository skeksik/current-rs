//! The relay, its routes and its listener (ADR 0020, "Routes and who may use it", "Lifecycle"): one per app, listening on 127.0.0.1
//! while at least one route exists, on a port the OS picks each time it starts.

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use current_net::RuntimeHandle;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinHandle;

use crate::RelayError;
use crate::events::{RelayEvents, RouteEvents};
use crate::path::Prefixes;
use crate::serve;
use crate::token::Token;
use crate::upstream::Upstream;

/// The most connections a relay serves at once; more wait in the OS's backlog until one closes. A guess to check on devices (ADR
/// 0020, "Concurrency and time").
pub const MAX_CONNECTIONS: usize = 16;

/// How long a client has to send a request's headers on a kept-alive connection (ADR 0020, "Concurrency and time").
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a new connection may stay without a request. A player asks at once; a connection that does not holds one of the
/// [`MAX_CONNECTIONS`] slots for nothing.
pub const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection's read and write buffer limit. The smallest hyper allows is 8 KiB; a player's request head is far smaller.
const MAX_BUF_SIZE: usize = 64 * 1024;

/// The most headers a request may have; hyper answers 431 beyond it.
const MAX_HEADERS: usize = 32;

/// A pause after a failed accept (out of file handles, say) before the next.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// The loopback relay. Cheap to clone; all clones are the same relay.
#[derive(Clone)]
pub struct Relay {
    shared: Arc<Shared>,
}

/// What a new route serves.
pub struct RouteConfig {
    /// The server its requests go to.
    pub upstream: Arc<dyn Upstream>,
    /// The paths it lets through, relative to the server's address, each ending in `/`: `/Videos/{itemId}/`, `/Audio/{itemId}/`.
    pub prefixes: Vec<String>,
    /// Where its events go.
    pub events: Arc<dyn RelayEvents>,
}

/// A route: one playback's way through the relay. Dropping it (or [`Route::remove`]) removes it: its open connections are aborted
/// and, if it was the last, the listener stops. Debug shows the port only.
pub struct Route {
    shared: Arc<Shared>,
    entry: Arc<RouteEntry>,
    port: u16,
}

pub(crate) struct Shared {
    runtime: RuntimeHandle,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    listener: Option<Listener>,
    routes: Vec<Arc<RouteEntry>>,
    next_id: u64,
}

struct Listener {
    port: u16,
    stop: watch::Sender<bool>,
    accept: JoinHandle<()>,
}

pub(crate) struct RouteEntry {
    id: u64,
    pub(crate) token: Token,
    pub(crate) upstream: Arc<dyn Upstream>,
    pub(crate) prefixes: Prefixes,
    pub(crate) events: Arc<RouteEvents>,
    connections: Mutex<Connections>,
}

/// The connections serving a route, each by the sender that aborts it.
#[derive(Default)]
struct Connections {
    closed: bool,
    cancels: Vec<watch::Sender<bool>>,
}

/// One accepted connection.
pub(crate) struct Conn {
    shared: Weak<Shared>,
    port: u16,
    cancel: watch::Sender<bool>,
    /// The route its first admitted request was for: a connection serves one route only.
    bound: Mutex<Option<u64>>,
    /// Whether any request has arrived.
    seen: AtomicBool,
}

/// What [`Conn::bind`] found.
pub(crate) enum Bind {
    Bound,
    /// The connection serves another route.
    Other,
    /// The route was removed meanwhile.
    Removed,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section here leaves the state consistent, so a panic elsewhere does not make it unusable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Relay {
    /// A relay whose listener and connections run on `runtime`. Nothing listens until the first route.
    pub fn new(runtime: &RuntimeHandle) -> Self {
        Self {
            shared: Arc::new(Shared {
                runtime: runtime.clone(),
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Adds a route, starting the listener if none runs.
    ///
    /// # Errors
    /// [`RelayError::InvalidPrefix`] for a prefix that is not a clean absolute path ending in `/`, [`RelayError::Random`] when the
    /// OS gives no random bytes, [`RelayError::Listen`] when no port could be opened on 127.0.0.1.
    pub fn add_route(&self, config: RouteConfig) -> Result<Route, RelayError> {
        let prefixes = Prefixes::new(config.prefixes)?;
        let token = Token::random()?;
        let mut state = lock(&self.shared.state);
        let port = match &state.listener {
            Some(listener) => listener.port,
            None => {
                let listener = start(&self.shared)?;
                let port = listener.port;
                state.listener = Some(listener);
                port
            }
        };
        let entry = Arc::new(RouteEntry {
            id: state.next_id,
            token,
            upstream: config.upstream,
            prefixes,
            events: Arc::new(RouteEvents::new(config.events)),
            connections: Mutex::default(),
        });
        state.next_id += 1;
        state.routes.push(entry.clone());
        Ok(Route {
            shared: self.shared.clone(),
            entry,
            port,
        })
    }

    /// The port the relay listens on, if it listens.
    pub fn port(&self) -> Option<u16> {
        lock(&self.shared.state)
            .listener
            .as_ref()
            .map(|listener| listener.port)
    }
}

impl Shared {
    /// The routes, for a request to find its own among.
    pub(crate) fn routes(&self) -> Vec<Arc<RouteEntry>> {
        lock(&self.state).routes.clone()
    }

    fn remove(&self, id: u64) {
        let (entry, listener) = {
            let mut state = lock(&self.state);
            let Some(at) = state.routes.iter().position(|route| route.id == id) else {
                return;
            };
            let entry = state.routes.remove(at);
            let listener = if state.routes.is_empty() {
                state.listener.take()
            } else {
                None
            };
            (entry, listener)
        };
        entry.close();
        if let Some(listener) = listener {
            listener.stop.send_replace(true);
            listener.accept.abort();
        }
    }
}

impl RouteEntry {
    /// Registers a connection's abort; false when the route was removed meanwhile.
    fn attach(&self, cancel: &watch::Sender<bool>) -> bool {
        let mut connections = lock(&self.connections);
        if connections.closed {
            return false;
        }
        // Connections that ended dropped their receiver.
        connections
            .cancels
            .retain(|cancel| cancel.receiver_count() > 0);
        connections.cancels.push(cancel.clone());
        true
    }

    fn close(&self) {
        let mut connections = lock(&self.connections);
        connections.closed = true;
        for cancel in connections.cancels.drain(..) {
            cancel.send_replace(true);
        }
    }
}

impl Conn {
    pub(crate) fn shared(&self) -> Option<Arc<Shared>> {
        self.shared.upgrade()
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Binds the connection to `route` on its first admitted request.
    pub(crate) fn bind(&self, route: &RouteEntry) -> Bind {
        let mut bound = lock(&self.bound);
        match *bound {
            Some(id) if id == route.id => Bind::Bound,
            Some(_) => Bind::Other,
            None if route.attach(&self.cancel) => {
                *bound = Some(route.id);
                Bind::Bound
            }
            None => Bind::Removed,
        }
    }

    /// Aborts the connection.
    pub(crate) fn close(&self) {
        self.cancel.send_replace(true);
    }

    pub(crate) fn saw_request(&self) {
        self.seen.store(true, Ordering::Relaxed);
    }
}

impl Route {
    /// The port the route is served on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The route's token, for log redaction (it is one of the stream's secrets, ADR 0020 "Why the player can hold nothing secret").
    pub fn token(&self) -> &str {
        self.entry.token.as_str()
    }

    /// The URL a player gets for `path_and_query` on the server (`/Videos/{id}/stream?static=true`, relative to its address, without
    /// `ApiKey`): `http://127.0.0.1:<port>/<token><path_and_query>`. It holds the token: treat it as a secret.
    pub fn url(&self, path_and_query: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{}{path_and_query}",
            self.port,
            self.entry.token.as_str()
        )
    }

    /// Removes the route; the same as dropping it.
    pub fn remove(self) {}
}

impl Drop for Route {
    fn drop(&mut self) {
        self.shared.remove(self.entry.id);
    }
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Route")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("port", &self.port())
            .finish_non_exhaustive()
    }
}

/// Opens 127.0.0.1 on a port of the OS's choice and starts accepting.
fn start(shared: &Arc<Shared>) -> Result<Listener, RelayError> {
    let socket =
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| RelayError::Listen)?;
    socket
        .set_nonblocking(true)
        .map_err(|_| RelayError::Listen)?;
    let port = socket.local_addr().map_err(|_| RelayError::Listen)?.port();
    let listener = {
        let _runtime = shared.runtime.enter();
        TcpListener::from_std(socket).map_err(|_| RelayError::Listen)?
    };
    let (stop, stopped) = watch::channel(false);
    let accept = shared
        .runtime
        .spawn(accept(listener, port, Arc::downgrade(shared), stopped));
    Ok(Listener { port, stop, accept })
}

async fn accept(
    listener: TcpListener,
    port: u16,
    shared: Weak<Shared>,
    mut stopped: watch::Receiver<bool>,
) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let permit = tokio::select! {
            permit = slots.clone().acquire_owned() => permit,
            _ = stopped.wait_for(|stop| *stop) => return,
        };
        let Ok(permit) = permit else {
            return;
        };
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopped.wait_for(|stop| *stop) => return,
        };
        match accepted {
            Ok((stream, _)) => {
                tokio::spawn(connection(
                    stream,
                    port,
                    shared.clone(),
                    stopped.clone(),
                    permit,
                ));
            }
            Err(_) => tokio::time::sleep(ACCEPT_RETRY).await,
        }
    }
}

/// Serves one connection until the client closes it, its route is removed or the listener stops.
async fn connection(
    stream: TcpStream,
    port: u16,
    shared: Weak<Shared>,
    mut stopped: watch::Receiver<bool>,
    _slot: OwnedSemaphorePermit,
) {
    // Small writes (a playlist, a header) go out at once; failing to set it costs latency only.
    let _ = stream.set_nodelay(true);
    let (cancel, mut cancelled) = watch::channel(false);
    let conn = Arc::new(Conn {
        shared,
        port,
        cancel,
        bound: Mutex::new(None),
        seen: AtomicBool::new(false),
    });
    let first = conn.clone();
    let first_request = async move {
        tokio::time::sleep(FIRST_REQUEST_TIMEOUT).await;
        if first.seen.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
    };
    let service = service_fn(move |request| {
        let conn = conn.clone();
        async move { Ok::<_, Infallible>(serve::handle(request, &conn).await) }
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_BUF_SIZE)
        .max_headers(MAX_HEADERS)
        .keep_alive(true);
    let serving = builder.serve_connection(TokioIo::new(stream), service);
    // Leaving the select drops the connection: its socket closes and any upstream body it was passing on is dropped with it.
    tokio::select! {
        _ = serving => {}
        _ = stopped.wait_for(|stop| *stop) => {}
        _ = cancelled.wait_for(|cancel| *cancel) => {}
        () = first_request => {}
    }
}
