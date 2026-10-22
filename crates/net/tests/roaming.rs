//! `RoamingAddresses` and `ServerClient` against hand-written stand-in servers on several ports of 127.0.0.1 (ADR 0014's test plan).
//!
//! Real time, with short timings and wide margins: assertions are about which address answered, not how fast.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_model::{
    Connectivity, NetworkFacts, NetworkKey, Reachability, ServerAddress, ServerKey, ServerRef,
    ServiceType,
};
use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{
    AddressProvider, ClientConfig, IdentityCheck, IdentityProbe, NetError, Request, RequestAuth,
    RequestLog, RoamingAddresses, RoamingConfig, RuntimeHandle, ServerClient,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket};
use tokio::task::JoinHandle;

const SECRET: &str = "s3cret-token-value";
const SERVER_ID: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ID: &str = "fedcba9876543210fedcba9876543210";
const NETWORK_KEY: &str = "wlan0 gateway 192.168.1.10";

/// What a stand-in does with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Answers `/info` with this id, and 200 to anything else.
    Serve(&'static str),
    /// Answers `/info` with the right id, and closes the connection without answering anything else.
    DropRequests,
    /// Reads every request and closes the connection without answering.
    DropAll,
    /// Reads the request and never answers.
    Silent,
    /// Answers `/info` with the right id, and this status to anything else.
    Status(u16),
}

/// A server on its own port of 127.0.0.1 that keeps the raw requests it received.
struct StandIn {
    port: u16,
    mode: Arc<Mutex<Mode>>,
    received: Arc<Mutex<Vec<String>>>,
    accept: JoinHandle<()>,
}

impl StandIn {
    async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mode = Arc::new(Mutex::new(mode));
        let received = Arc::new(Mutex::new(Vec::new()));
        let (shared_mode, log) = (mode.clone(), received.clone());
        let accept = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mode = *shared_mode.lock().unwrap();
                let log = log.clone();
                tokio::spawn(async move {
                    let head = read_head(&mut socket).await;
                    log.lock().unwrap().push(head.clone());
                    let info = head.starts_with("GET /info ");
                    let id = match mode {
                        Mode::Serve(id) => Some(id),
                        Mode::Status(status) if !info => {
                            let response = format!(
                                "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                            let _ = socket.shutdown().await;
                            return;
                        }
                        Mode::Status(_) => Some(SERVER_ID),
                        Mode::DropRequests if info => Some(SERVER_ID),
                        Mode::DropRequests | Mode::DropAll => {
                            let _ = socket.shutdown().await;
                            return;
                        }
                        Mode::Silent => {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            return;
                        }
                    };
                    let body = if info {
                        format!(r#"{{"Id":"{}"}}"#, id.unwrap_or_default())
                    } else {
                        "{}".to_owned()
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            port,
            mode,
            received,
            accept,
        }
    }

    /// A port where nothing listens, so connections are refused: a socket bound to it but never listening. The socket must be kept
    /// while the port is in use, so that no other test can take the port.
    fn refused() -> (u16, TcpSocket) {
        let socket = TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        (socket.local_addr().unwrap().port(), socket)
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    /// Stops listening: new connections are refused.
    async fn stop(&self) {
        self.accept.abort();
        while !self.accept.is_finished() {
            tokio::task::yield_now().await;
        }
    }

    fn received(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }

    /// How many requests for `path` (with any query) arrived with `method`.
    fn count(&self, method: &str, path: &str) -> usize {
        let start = format!("{method} {path}");
        self.received()
            .iter()
            .filter(|head| {
                head.strip_prefix(&start)
                    .is_some_and(|rest| rest.starts_with([' ', '?']))
            })
            .count()
    }
}

fn address(port: u16) -> ServerAddress {
    ServerAddress {
        url: format!("http://127.0.0.1:{port}"),
        plain_http_confirmed: false,
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

/// Adds a token as a header and as a query value, the way service crates will.
struct TokenAuth;

impl RequestAuth for TokenAuth {
    fn apply(&self, headers: &mut HeaderMap, query: &mut Vec<(String, String)>) {
        headers.insert(
            HeaderName::from_static("x-test-token"),
            HeaderValue::from_static(SECRET),
        );
        query.push(("api_key".to_owned(), SECRET.to_owned()));
    }
}

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

/// A roaming provider over `ports` and a client with credentials using it.
fn setup(ports: &[u16]) -> (Arc<RoamingAddresses>, ServerClient, Arc<RequestLog>) {
    let log = Arc::new(RequestLog::default());
    let addresses: Vec<ServerAddress> = ports.iter().map(|port| address(*port)).collect();
    let roaming = RoamingAddresses::new(
        server(),
        &addresses,
        Arc::new(TestIdentity),
        log.clone(),
        RoamingConfig {
            stagger: Duration::from_millis(100),
            probe_timeout: Duration::from_secs(2),
            ..RoamingConfig::default()
        },
        &RuntimeHandle::current(),
    )
    .unwrap();
    let client = ServerClient::new(
        server(),
        roaming.clone(),
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &ClientConfig {
            timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
        },
    )
    .unwrap();
    (roaming, client, log)
}

fn position(roaming: &RoamingAddresses) -> Option<u8> {
    roaming.current().and_then(|selected| selected.position())
}

#[tokio::test]
async fn the_race_skips_dead_and_wrong_addresses() {
    let (refused, _reserved) = StandIn::refused();
    let silent = StandIn::start(Mode::Silent).await;
    let other = StandIn::start(Mode::Serve(OTHER_ID)).await;
    let right = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (roaming, client, log) = setup(&[refused, silent.port, other.port, right.port]);

    client.send(Request::get("/Items")).await.unwrap();
    assert_eq!(position(&roaming), Some(4));
    let status = roaming.status().borrow().clone();
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.different_server, [3]);

    // Credentials reached the right server only, and no probe carried them.
    assert_eq!(right.count("GET", "/Items"), 1);
    assert!(right.received().iter().any(|head| head.contains(SECRET)));
    for stand_in in [&silent, &other, &right] {
        for head in stand_in.received() {
            if head.starts_with("GET /info ") || stand_in.port != right.port {
                assert!(!head.contains(SECRET), "{head}");
                assert!(!head.to_lowercase().contains("x-test-token"), "{head}");
            }
        }
    }

    let text = log.to_text();
    assert!(
        text.contains("jellyfin/1 address 4 GET /info -> 200"),
        "{text}"
    );
    assert!(
        text.contains("jellyfin/1 address 3 GET /info -> 200"),
        "{text}"
    );
    // Probes still running when the race is won are cancelled and leave no line. On the Windows runner a refused connection to
    // 127.0.0.1 took about 2 s (seen in CI, cause not verified), so address 1's probe may be one of them: no line is asserted for it.
    assert!(
        text.contains("jellyfin/1 address 4 GET /Items -> 200"),
        "{text}"
    );
}

#[tokio::test]
async fn a_get_is_retried_once_on_the_new_address_after_a_connect_failure() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (roaming, client, log) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();
    assert_eq!(position(&roaming), Some(1));

    first.stop().await;
    client.send(Request::get("/Items")).await.unwrap();
    assert_eq!(position(&roaming), Some(2));
    assert_eq!(second.count("GET", "/Items"), 1);
    let text = log.to_text();
    assert!(
        text.contains("address 1 GET /Items -> unreachable"),
        "{text}"
    );
    assert!(text.contains("address 2 GET /Items -> 200"), "{text}");
}

#[tokio::test]
async fn a_post_that_was_never_sent_is_retried() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (_roaming, client, _) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();

    // Refused: the request never left, so even a POST may go to the new address.
    first.stop().await;
    client.send(Request::post("/Items")).await.unwrap();
    assert_eq!(first.count("POST", "/Items"), 0);
    assert_eq!(second.count("POST", "/Items"), 1);
}

#[tokio::test]
async fn a_post_after_a_dropped_response_is_not_retried() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (roaming, client, _) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();

    first.set(Mode::DropAll);
    let result = client.send(Request::post("/Items")).await;
    assert_eq!(result.map(|_| ()), Err(NetError::Unreachable));
    assert_eq!(first.count("POST", "/Items"), 1);
    assert_eq!(second.count("POST", "/Items"), 0);

    // The failure still moved the server to the other address.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let next = roaming.wait(deadline).await;
    assert_eq!(next.and_then(|s| s.position()), Some(2));
}

#[tokio::test]
async fn a_get_after_a_dropped_response_is_retried() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (_roaming, client, _) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();

    first.set(Mode::DropAll);
    client.send(Request::get("/Items")).await.unwrap();
    assert_eq!(first.count("GET", "/Items"), 2);
    assert_eq!(second.count("GET", "/Items"), 1);
}

#[tokio::test]
async fn there_is_no_second_retry() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::DropRequests).await;
    let (_roaming, client, _) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();

    first.set(Mode::DropAll);
    let result = client.send(Request::get("/Items")).await;
    assert_eq!(result.map(|_| ()), Err(NetError::Unreachable));
    assert_eq!(first.count("GET", "/Items"), 2);
    assert_eq!(second.count("GET", "/Items"), 1);
}

#[tokio::test]
async fn a_status_or_decode_error_is_the_server_answering_not_a_reason_to_race() {
    let first = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let second = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (roaming, client, _) = setup(&[first.port, second.port]);
    client.send(Request::get("/Items")).await.unwrap();
    let probes = first.count("GET", "/info");

    // The stand-in answers `{}`, which is not a string: the server answered, so the address is fine.
    let decoded = client.send_json::<String>(Request::get("/Items")).await;
    assert!(matches!(decoded, Err(NetError::Decode { .. })));
    for status in [401, 500] {
        first.set(Mode::Status(status));
        let result = client.send(Request::get("/Items")).await;
        match result {
            Err(NetError::Auth { status: got, .. } | NetError::Status { status: got, .. }) => {
                assert_eq!(got, status);
            }
            other => panic!("{other:?}"),
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(position(&roaming), Some(1));
    assert_eq!(first.count("GET", "/info"), probes);
    assert_eq!(second.count("GET", "/info"), 0);
    assert_eq!(second.count("GET", "/Items"), 0);
}

#[tokio::test]
async fn no_retry_when_the_race_picks_the_same_address() {
    let only = StandIn::start(Mode::DropRequests).await;
    let (roaming, client, _) = setup(&[only.port]);
    let result = client.send(Request::get("/Items")).await;
    assert_eq!(result.map(|_| ()), Err(NetError::Unreachable));
    // The race after the failure checked the address again and picked it: the request was not sent twice.
    assert_eq!(only.count("GET", "/Items"), 1);
    assert_eq!(only.count("GET", "/info"), 2);
    assert_eq!(position(&roaming), Some(1));
}

#[tokio::test]
async fn no_log_line_error_or_debug_output_names_an_address_or_the_network() {
    let (refused, _reserved) = StandIn::refused();
    let right = StandIn::start(Mode::Serve(SERVER_ID)).await;
    let (roaming, client, log) = setup(&[refused, right.port]);
    roaming.network_changed(&NetworkFacts {
        connectivity: Connectivity::Full,
        network_key: Some(NetworkKey::new(NETWORK_KEY)),
        ..NetworkFacts::default()
    });
    client.send(Request::get("/Items")).await.unwrap();
    right.stop().await;
    let error = client.send(Request::get("/Items")).await.unwrap_err();
    let selected = roaming.current();

    let outputs = [
        log.to_text(),
        format!("{error} {error:?}"),
        format!("{roaming:?} {client:?} {selected:?}"),
        format!("{:?}", roaming.status().borrow()),
    ];
    for output in outputs {
        for secret in [
            "127.0.0.1",
            &format!(":{refused}"),
            &format!(":{}", right.port),
            NETWORK_KEY,
            "192.168",
            SECRET,
        ] {
            assert!(!output.contains(secret), "{secret} in {output}");
        }
    }
}
