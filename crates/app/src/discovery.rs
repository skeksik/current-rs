//! Finding servers on the local network for the "add a server" screen, next to the typed address (ADR 0016).
//!
//! A search broadcasts Jellyfin's discovery request, then probes every reply with ADR 0012's prober and keeps only the servers
//! whose probe answers with the id the reply claimed: a reply alone is never shown. Picking a result hands it to [`AddServer`],
//! where the plain-HTTP rule applies as for a typed address.
//!
//! [`AddServer`]: crate::AddServer

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Duration;

use current_jellyfin::{Candidate, DiscoveryConfig, DiscoveryError, FoundServer, Prober, discover};
use current_model::{PublicServerInfo, ServerKey, ServerRef, ServiceType};
use current_net::{ClientConfig, RequestLog};
use tokio::sync::watch;

use crate::add_server::NewServer;

/// How many replies are probed at once, so that a flood of bogus replies cannot start dozens of connections.
pub const PROBES_AT_ONCE: usize = 8;

/// A server found on the local network and confirmed by probing it. Debug leaves out its addresses.
#[derive(Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub(crate) found: FoundServer,
}

impl DiscoveredServer {
    /// What the server answered to the probe (not what its discovery reply claimed).
    pub fn info(&self) -> &PublicServerInfo {
        &self.found.info
    }

    /// The server as "add a server" would add it, with the address it answered on.
    pub fn server(&self) -> NewServer {
        NewServer::from_found(&self.found)
    }

    /// Whether the user must confirm plain HTTP before the address is used (ADR 0012).
    pub fn needs_confirmation(&self) -> bool {
        self.found.needs_confirmation()
    }
}

impl std::fmt::Debug for DiscoveredServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveredServer")
            .field("id", &self.found.info.id)
            .field("name", &self.found.info.name)
            .field("version", &self.found.info.version)
            .finish_non_exhaustive()
    }
}

/// Where a search is. The UI shows the results as they come in, and "no servers found" for `Done` with none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DiscoveryState {
    /// No search yet, or reset.
    #[default]
    Idle,
    /// Searching; `found` grows as replies are confirmed.
    Searching { found: Vec<DiscoveredServer> },
    /// The search ended.
    Done { found: Vec<DiscoveredServer> },
    /// The search could not run, for example because the system does not permit it or there is no network.
    Failed { reason: DiscoveryError },
}

/// The search's state and actions. A newer search or a reset supersedes a search still running: its results are dropped.
#[derive(Debug)]
pub struct Discovery {
    prober: Prober,
    config: DiscoveryConfig,
    state: watch::Sender<DiscoveryState>,
    generation: AtomicU64,
}

impl Discovery {
    /// A search on the limited broadcast address whose probes are logged to `log` as `jellyfin/discovery`. The probes have
    /// shorter timeouts than setup's (2 s to connect, 5 s per request): the server just answered from the local network.
    pub fn new(log: Arc<RequestLog>) -> Self {
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("discovery"));
        let config = ClientConfig {
            timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(2),
        };
        Self::with_config(
            Prober::with_config(server, log, config),
            DiscoveryConfig::default(),
        )
    }

    pub fn with_config(prober: Prober, config: DiscoveryConfig) -> Self {
        Self {
            prober,
            config,
            state: watch::Sender::new(DiscoveryState::Idle),
            generation: AtomicU64::new(0),
        }
    }

    /// The state now, and every change after.
    pub fn state(&self) -> watch::Receiver<DiscoveryState> {
        self.state.subscribe()
    }

    /// Searches, probes the replies ([`PROBES_AT_ONCE`] at a time) and ends in `Done` or `Failed`, unless a newer search or a reset
    /// came first.
    pub async fn search(&self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.state
            .send_replace(DiscoveryState::Searching { found: Vec::new() });
        let candidates = match discover(&self.config).await {
            Ok(candidates) => candidates,
            Err(reason) => {
                self.update(generation, |state| {
                    *state = DiscoveryState::Failed { reason };
                });
                return;
            }
        };
        for_each_limited(candidates, PROBES_AT_ONCE, |candidate| async move {
            if !self.is_current(generation) {
                return;
            }
            if let Some(found) = self.confirm(&candidate).await {
                self.update(generation, |state| {
                    if let DiscoveryState::Searching { found: list } = state {
                        list.push(DiscoveredServer { found });
                    }
                });
            }
        })
        .await;
        self.update(generation, |state| {
            if let DiscoveryState::Searching { found } = state {
                *state = DiscoveryState::Done {
                    found: std::mem::take(found),
                };
            }
        });
    }

    /// Back to `Idle`, dropping any search still running.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.state.send_if_modified(|state| {
            let changed = *state != DiscoveryState::Idle;
            *state = DiscoveryState::Idle;
            changed
        });
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    /// Changes the state if `generation` is still the latest search. Checked under the channel's lock, so a reset or newer search
    /// in between always wins.
    fn update(&self, generation: u64, change: impl FnOnce(&mut DiscoveryState)) {
        self.state.send_if_modified(|state| {
            if self.is_current(generation) {
                change(state);
                true
            } else {
                false
            }
        });
    }

    /// The candidate's first address that leads to a usable server with the id its reply claimed.
    async fn confirm(&self, candidate: &Candidate) -> Option<FoundServer> {
        for address in candidate.addresses() {
            if let Ok(found) = self.prober.probe_expecting(address, &candidate.id).await {
                return Some(found);
            }
        }
        None
    }
}

/// Runs `run` on every item, at most `limit` at once, on the calling task (the crate holds no runtime to spawn on).
async fn for_each_limited<T, F, Fut>(items: Vec<T>, limit: usize, mut run: F)
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut items = items.into_iter();
    let mut running: Vec<Pin<Box<Fut>>> = Vec::new();
    loop {
        while running.len() < limit.max(1) {
            match items.next() {
                Some(item) => running.push(Box::pin(run(item))),
                None => break,
            }
        }
        if running.is_empty() {
            return;
        }
        // Polls every running future; returns once at least one finished, to start the next items.
        poll_fn(|cx| {
            let before = running.len();
            running.retain_mut(|future| future.as_mut().poll(cx).is_pending());
            if running.len() < before {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;

    use current_model::ServerId;
    use current_net::Url;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, UdpSocket};

    use super::*;
    use crate::{AddServer, AddServerState};

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const INFO: &str = r#"{"ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"0123456789abcdef0123456789abcdef","StartupWizardCompleted":true}"#;

    /// A Jellyfin HTTP stand-in on 127.0.0.1; returns its port.
    async fn http_stand_in() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = socket.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{INFO}",
                        INFO.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        port
    }

    /// A discovery stand-in on 127.0.0.1 that answers each request with `replies` (address, id).
    async fn udp_stand_in(replies: Vec<(String, String)>) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 512];
            while let Ok((_, from)) = socket.recv_from(&mut buffer).await {
                for (address, id) in &replies {
                    let reply =
                        format!(r#"{{"Address":"{address}","Id":"{id}","Name":"Claimed"}}"#);
                    let _ = socket.send_to(reply.as_bytes(), from).await;
                }
            }
        });
        address
    }

    fn discovery(log: Arc<RequestLog>, target: SocketAddr) -> Discovery {
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("discovery"));
        Discovery::with_config(
            Prober::new(server, log),
            DiscoveryConfig {
                targets: vec![target],
                window: Duration::from_millis(300),
                resend_after: None,
            },
        )
    }

    fn done(discovery: &Discovery) -> Vec<DiscoveredServer> {
        match discovery.state().borrow().clone() {
            DiscoveryState::Done { found } => found,
            state => panic!("{state:?}"),
        }
    }

    #[tokio::test]
    async fn only_replies_whose_probe_matches_are_shown() {
        let port = http_stand_in().await;
        let target = udp_stand_in(vec![
            (format!("http://127.0.0.1:{port}"), ID.into()),
            // Another id than the server at that address answers.
            (
                format!("http://127.0.0.1:{port}"),
                "ffffffffffffffffffffffffffffffff".into(),
            ),
            // Nothing there.
            (
                "http://127.0.0.1:1".into(),
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
            ),
        ])
        .await;
        let discovery = discovery(Arc::new(RequestLog::default()), target);
        discovery.search().await;
        let found = done(&discovery);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].info().id, ServerId::new(ID));
        // The probed name, not the claimed one.
        assert_eq!(found[0].info().name, "Test Server");
        assert_eq!(
            found[0].server().address.url,
            format!("http://127.0.0.1:{port}")
        );
        assert!(!found[0].needs_confirmation());
    }

    #[tokio::test]
    async fn the_reply_source_is_probed_before_the_claimed_ip() {
        let port = http_stand_in().await;
        let target = udp_stand_in(vec![(format!("http://192.168.1.10:{port}"), ID.into())]).await;
        let discovery = discovery(Arc::new(RequestLog::default()), target);
        discovery.search().await;
        let found = done(&discovery);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].server().address.url,
            format!("http://127.0.0.1:{port}")
        );
    }

    #[tokio::test]
    async fn nothing_found_ends_done_and_empty() {
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let discovery = discovery(
            Arc::new(RequestLog::default()),
            silent.local_addr().unwrap(),
        );
        let rx = discovery.state();
        discovery.search().await;
        assert!(done(&discovery).is_empty());
        assert!(rx.has_changed().unwrap());
    }

    #[tokio::test]
    async fn a_reset_drops_a_running_search() {
        let port = http_stand_in().await;
        let target = udp_stand_in(vec![(format!("http://127.0.0.1:{port}"), ID.into())]).await;
        let discovery = Arc::new(discovery(Arc::new(RequestLog::default()), target));
        let search = tokio::spawn({
            let discovery = discovery.clone();
            async move { discovery.search().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(matches!(
            *discovery.state().borrow(),
            DiscoveryState::Searching { .. }
        ));
        discovery.reset();
        search.await.unwrap();
        assert_eq!(*discovery.state().borrow(), DiscoveryState::Idle);
    }

    #[tokio::test]
    async fn picking_a_result_readies_add_server() {
        let port = http_stand_in().await;
        let target = udp_stand_in(vec![(format!("http://127.0.0.1:{port}"), ID.into())]).await;
        let discovery = discovery(Arc::new(RequestLog::default()), target);
        discovery.search().await;
        let found = done(&discovery);
        let flow = AddServer::new(Arc::new(RequestLog::default()));
        flow.pick(&found[0]);
        let AddServerState::Ready { server } = flow.state().borrow().clone() else {
            panic!("{:?}", *flow.state().borrow())
        };
        assert_eq!(server, found[0].server());
    }

    #[test]
    fn a_public_http_result_needs_confirmation_when_picked() {
        let address = Url::parse("http://media.example.com:8096/").unwrap();
        let result = DiscoveredServer {
            found: FoundServer {
                info: PublicServerInfo {
                    id: ServerId::new(ID),
                    name: "Test Server".into(),
                    version: "12.2.0".into(),
                    product: "Jellyfin Server".into(),
                    local_address: None,
                    setup_completed: true,
                },
                transport: current_net::transport(&address),
                address,
                https_failed_tls: false,
            },
        };
        assert!(result.needs_confirmation());
        let flow = AddServer::new(Arc::new(RequestLog::default()));
        flow.pick(&result);
        assert!(matches!(
            *flow.state().borrow(),
            AddServerState::ConfirmPlainHttp { .. }
        ));
    }

    #[tokio::test]
    async fn no_address_in_state_or_diagnostics() {
        let port = http_stand_in().await;
        let target = udp_stand_in(vec![(format!("http://192.168.1.10:{port}"), ID.into())]).await;
        let log = Arc::new(RequestLog::default());
        let discovery = discovery(log.clone(), target);
        discovery.search().await;
        let state = format!("{:?}", *discovery.state().borrow());
        let diagnostics = log.to_text();
        assert!(state.contains("Test Server"), "{state}");
        assert!(
            diagnostics.contains("jellyfin/discovery GET /System/Info/Public -> 200"),
            "{diagnostics}"
        );
        for text in [&state, &diagnostics] {
            for needle in [
                "127.0.0.1",
                "192.168.1.10",
                &port.to_string(),
                &target.port().to_string(),
            ] {
                assert!(!text.contains(needle), "{needle} in {text}");
            }
        }
    }

    #[tokio::test]
    async fn at_most_the_limit_runs_at_once() {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        for_each_limited((0..20).collect(), 3, |_: i32| {
            let (running, peak) = (running.clone(), peak.clone());
            async move {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                running.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .await;
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(running.load(Ordering::SeqCst), 0);
    }
}
