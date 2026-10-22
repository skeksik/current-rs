//! The "add a server" flow: the user types an address, the core probes it and either reports why it cannot be used, asks the user to
//! confirm plain HTTP, or hands back a server ready to add (ADR 0012).
//!
//! In memory only: storing the server is `current-store`'s, signing in is a later step.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use current_jellyfin::{FoundServer, ProbeError, Prober};
use current_model::{PublicServerInfo, ServerAddress, ServerKey, ServerRef, ServiceType};
use current_net::RequestLog;
use tokio::sync::watch;

/// A server the flow found, with the address it answered on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewServer {
    pub info: PublicServerInfo,
    pub address: ServerAddress,
    /// https was tried and failed its handshake before plain http answered: the server may offer https with a certificate this
    /// device does not trust. The UI says so.
    pub https_failed_tls: bool,
}

/// Where the flow is. The UI renders one screen state per variant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AddServerState {
    /// Waiting for an address.
    #[default]
    Idle,
    /// Probing what the user typed.
    Probing { input: String },
    /// No usable server at that address.
    Failed { input: String, error: ProbeError },
    /// A server answered over plain HTTP outside the local network: it is used only if the user confirms (once per address).
    ConfirmPlainHttp { server: NewServer },
    /// A server ready to add.
    Ready { server: NewServer },
}

/// The flow's state and actions. A newer probe or a reset supersedes a probe still running: its result is dropped.
#[derive(Debug)]
pub struct AddServer {
    prober: Prober,
    state: watch::Sender<AddServerState>,
    generation: AtomicU64,
}

impl AddServer {
    /// A flow whose probes are logged to `log` as `jellyfin/setup`.
    pub fn new(log: Arc<RequestLog>) -> Self {
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("setup"));
        Self::with_prober(Prober::new(server, log))
    }

    pub fn with_prober(prober: Prober) -> Self {
        Self {
            prober,
            state: watch::Sender::new(AddServerState::Idle),
            generation: AtomicU64::new(0),
        }
    }

    /// The state now, and every change after.
    pub fn state(&self) -> watch::Receiver<AddServerState> {
        self.state.subscribe()
    }

    /// Probes `input` and moves to `Failed`, `ConfirmPlainHttp` or `Ready`, unless a newer probe or a reset came first.
    pub async fn probe(&self, input: &str) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.send_replace(AddServerState::Probing {
            input: input.to_owned(),
        });
        let next = match self.prober.probe(input).await {
            Ok(found) => settle(found),
            Err(error) => AddServerState::Failed {
                input: input.to_owned(),
                error,
            },
        };
        // Checked and replaced under the channel's lock, so a reset or newer probe in between always wins.
        self.state.send_if_modified(|state| {
            if self.generation.load(Ordering::SeqCst) == generation {
                *state = next;
                true
            } else {
                false
            }
        });
    }

    /// The user agreed to plain HTTP: `ConfirmPlainHttp` becomes `Ready`, with the confirmation kept on the address. Returns whether
    /// there was anything to confirm.
    pub fn confirm_plain_http(&self) -> bool {
        self.state.send_if_modified(|state| match state {
            AddServerState::ConfirmPlainHttp { server } => {
                let mut server = server.clone();
                server.address.plain_http_confirmed = true;
                *state = AddServerState::Ready { server };
                true
            }
            _ => false,
        })
    }

    /// Back to `Idle`, dropping any probe still running.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.state.send_if_modified(|state| {
            let changed = *state != AddServerState::Idle;
            *state = AddServerState::Idle;
            changed
        });
    }
}

/// The state a found server leads to.
fn settle(found: FoundServer) -> AddServerState {
    let server = NewServer {
        address: found.server_address(false),
        https_failed_tls: found.https_failed_tls,
        info: found.info.clone(),
    };
    if found.needs_confirmation() {
        AddServerState::ConfirmPlainHttp { server }
    } else {
        AddServerState::Ready { server }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use current_model::ServerId;
    use current_net::{Transport, Url};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    use super::*;

    const INFO: &str = r#"{"ServerName":"Test Server","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"0123456789abcdef0123456789abcdef","StartupWizardCompleted":true}"#;

    /// A Jellyfin stand-in on 127.0.0.1 that answers once `release` is notified (at once if `None`).
    async fn stand_in(release: Option<Arc<Notify>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let release = release.clone();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = socket.read(&mut buffer).await;
                    if let Some(release) = release {
                        release.notified().await;
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{INFO}",
                        INFO.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn flow() -> AddServer {
        AddServer::new(Arc::new(RequestLog::default()))
    }

    fn found(address: &str) -> FoundServer {
        let address = Url::parse(address).unwrap();
        FoundServer {
            info: PublicServerInfo {
                id: ServerId::new("1"),
                name: "Test Server".into(),
                version: "12.2.0".into(),
                product: "Jellyfin Server".into(),
                local_address: None,
                setup_completed: true,
            },
            transport: current_net::transport(&address),
            address,
            https_failed_tls: false,
        }
    }

    #[tokio::test]
    async fn a_local_server_is_ready_at_once() {
        let address = stand_in(None).await;
        let flow = flow();
        let rx = flow.state();
        flow.probe(&address).await;
        let AddServerState::Ready { server } = rx.borrow().clone() else {
            panic!("{:?}", *rx.borrow())
        };
        assert_eq!(server.address.url, address);
        assert!(!server.address.plain_http_confirmed);
        assert_eq!(server.info.name, "Test Server");
    }

    #[tokio::test]
    async fn a_failure_keeps_the_input() {
        let flow = flow();
        flow.probe("ftp://media.example.com").await;
        assert!(matches!(
            &*flow.state().borrow(),
            AddServerState::Failed { input, error: ProbeError::InvalidAddress(_) } if input == "ftp://media.example.com"
        ));
        flow.probe("http://127.0.0.1:1").await;
        assert!(matches!(
            &*flow.state().borrow(),
            AddServerState::Failed {
                error: ProbeError::Unreachable,
                ..
            }
        ));
    }

    #[test]
    fn plain_http_outside_the_local_network_is_confirmed_once() {
        let state = settle(found("http://media.example.com:8096/"));
        let AddServerState::ConfirmPlainHttp { server } = &state else {
            panic!("{state:?}")
        };
        assert_eq!(server.address.url, "http://media.example.com:8096");
        assert!(!server.address.plain_http_confirmed);

        let flow = flow();
        assert!(!flow.confirm_plain_http());
        flow.state.send_replace(state);
        assert!(flow.confirm_plain_http());
        let AddServerState::Ready { server } = flow.state().borrow().clone() else {
            panic!("not ready")
        };
        assert!(server.address.plain_http_confirmed);
        assert!(!flow.confirm_plain_http());
    }

    #[test]
    fn https_and_local_http_need_no_confirmation() {
        assert!(matches!(
            settle(found("https://media.example.com/jellyfin")),
            AddServerState::Ready { .. }
        ));
        let local = found("http://192.168.1.10:8096/");
        assert_eq!(local.transport, Transport::PlainLocal);
        assert!(matches!(settle(local), AddServerState::Ready { .. }));
    }

    #[tokio::test]
    async fn a_newer_probe_wins_over_a_slower_older_one() {
        let release = Arc::new(Notify::new());
        let slow = stand_in(Some(release.clone())).await;
        let fast = stand_in(None).await;
        let flow = Arc::new(flow());

        let older = tokio::spawn({
            let flow = flow.clone();
            async move { flow.probe(&slow).await }
        });
        // Let the older probe reach the server.
        tokio::time::sleep(Duration::from_millis(100)).await;
        flow.probe(&fast).await;
        release.notify_one();
        older.await.unwrap();

        let AddServerState::Ready { server } = flow.state().borrow().clone() else {
            panic!("not ready")
        };
        assert_eq!(server.address.url, fast);
    }

    #[tokio::test]
    async fn a_reset_drops_a_running_probe() {
        let release = Arc::new(Notify::new());
        let slow = stand_in(Some(release.clone())).await;
        let flow = Arc::new(flow());
        let probe = tokio::spawn({
            let flow = flow.clone();
            async move { flow.probe(&slow).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(
            *flow.state().borrow(),
            AddServerState::Probing { .. }
        ));
        flow.reset();
        release.notify_one();
        probe.await.unwrap();
        assert_eq!(*flow.state().borrow(), AddServerState::Idle);
    }
}
