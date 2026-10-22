//! Server setup against hand-written stand-in servers on 127.0.0.1.

use std::sync::Arc;

use current_jellyfin::{FoundServer, ProbeError, Prober, same_server};
use current_model::{ServerId, ServerKey, ServerRef, ServiceType};
use current_net::{RequestLog, Transport, Url};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ID: &str = "0123456789abcdef0123456789abcdef";

/// The body of `/System/Info/Public` as Jellyfin 12.2 sends it.
fn info(id: &str, version: &str, wizard_completed: bool) -> String {
    format!(
        r#"{{"LocalAddress":"http://192.168.1.10:8096","ServerName":"Test Server","Version":"{version}","ProductName":"Jellyfin Server","OperatingSystem":"","Id":"{id}","StartupWizardCompleted":{wizard_completed}}}"#
    )
}

/// One answer of the stand-in: status, an optional `Location`, a content type and a body.
#[derive(Clone)]
struct Answer {
    status: u16,
    location: Option<String>,
    content_type: &'static str,
    body: String,
}

fn json(status: u16, body: String) -> Answer {
    Answer {
        status,
        location: None,
        content_type: "application/json",
        body,
    }
}

fn html(status: u16) -> Answer {
    Answer {
        status,
        location: None,
        content_type: "text/html",
        body: "<!DOCTYPE html><html><body>Hello</body></html>".to_owned(),
    }
}

fn redirect(status: u16, location: &str) -> Answer {
    Answer {
        status,
        location: Some(location.to_owned()),
        content_type: "text/plain",
        body: String::new(),
    }
}

/// A server on 127.0.0.1 answering each path it knows (matched without the query) and 404 to anything else. A TLS handshake gets a
/// plain-HTTP error, as an http-only server does.
struct StandIn {
    port: u16,
}

impl StandIn {
    async fn start(routes: Vec<(&'static str, Answer)>) -> Self {
        let listener = ok(TcpListener::bind("127.0.0.1:0").await);
        let port = ok(listener.local_addr()).port();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut first = [0u8; 1];
                    if socket.peek(&mut first).await.is_ok() && first[0] == 0x16 {
                        let _ = socket
                            .write_all(b"HTTP/1.1 400 Bad Request\r\nconnection: close\r\n\r\n")
                            .await;
                        let _ = socket.shutdown().await;
                        return;
                    }
                    let head = read_head(&mut socket).await;
                    let path = head
                        .split(' ')
                        .nth(1)
                        .unwrap_or_default()
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    let answer = routes
                        .iter()
                        .find(|(route, _)| *route == path)
                        .map(|(_, answer)| answer.clone())
                        .unwrap_or_else(|| json(404, String::new()));
                    let location = answer
                        .location
                        .map(|location| format!("location: {location}\r\n"))
                        .unwrap_or_default();
                    let response = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\n{location}connection: close\r\n\r\n{}",
                        answer.status,
                        answer.content_type,
                        answer.body.len(),
                        answer.body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { port }
    }

    fn address(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
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

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{error:?}"),
    }
}

fn prober() -> (Prober, Arc<RequestLog>) {
    let log = Arc::new(RequestLog::default());
    let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("setup"));
    (Prober::new(server, log.clone()), log)
}

async fn probe(input: &str) -> Result<FoundServer, ProbeError> {
    prober().0.probe(input).await
}

/// Nothing a user would paste may hold the stand-in's address.
fn assert_clean(text: &str, server: &StandIn) {
    assert!(!text.contains("127.0.0.1"), "address leaked: {text}");
    assert!(
        !text.contains(&format!(":{}", server.port)),
        "port leaked: {text}"
    );
}

#[tokio::test]
async fn a_jellyfin_server_is_found() {
    let server = StandIn::start(vec![(
        "/System/Info/Public",
        json(200, info(ID, "12.2.0", true)),
    )])
    .await;
    let (prober, log) = prober();
    let found = ok(prober.probe(&server.address("")).await);

    assert_eq!(found.info.id, ServerId::new(ID));
    assert_eq!(found.info.name, "Test Server");
    assert_eq!(found.info.version, "12.2.0");
    assert_eq!(found.info.product, "Jellyfin Server");
    assert_eq!(
        found.info.local_address.as_deref(),
        Some("http://192.168.1.10:8096")
    );
    assert!(found.info.setup_completed);
    assert_eq!(String::from(found.address.clone()), server.address("/"));
    assert_eq!(found.transport, Transport::PlainLocal);
    assert!(!found.needs_confirmation());
    assert!(!found.https_failed_tls);
    let address = found.server_address(false);
    assert_eq!(address.url, server.address(""));
    assert!(!address.plain_http_confirmed);

    let text = log.to_text();
    assert!(
        text.contains("jellyfin/setup GET /System/Info/Public -> 200"),
        "{text}"
    );
    assert_clean(&text, &server);
}

#[tokio::test]
async fn a_server_whose_setup_is_not_finished_says_so() {
    let server = StandIn::start(vec![(
        "/System/Info/Public",
        json(200, info(ID, "12.2.0", false)),
    )])
    .await;
    assert_eq!(
        probe(&server.address("")).await,
        Err(ProbeError::SetupIncomplete)
    );
}

#[tokio::test]
async fn an_older_server_is_too_old() {
    let server = StandIn::start(vec![(
        "/System/Info/Public",
        json(200, info(ID, "10.11.3", true)),
    )])
    .await;
    let error = probe(&server.address("")).await.err();
    assert_eq!(
        error,
        Some(ProbeError::TooOld {
            version: "10.11.3".into()
        })
    );
    let Some(error) = error else { return };
    assert!(error.to_string().contains("12.0.0 or newer"), "{error}");
}

#[tokio::test]
async fn a_starting_or_refusing_server_is_unavailable() {
    let server = StandIn::start(vec![("/System/Info/Public", html(503))]).await;
    assert_eq!(
        probe(&server.address("")).await,
        Err(ProbeError::Unavailable)
    );
}

#[tokio::test]
async fn other_servers_are_not_jellyfin() {
    // A web page where the endpoint should be.
    let page = StandIn::start(vec![("/System/Info/Public", html(200))]).await;
    assert_eq!(probe(&page.address("")).await, Err(ProbeError::NotJellyfin));
    // JSON without the fields Jellyfin always sends.
    let other = StandIn::start(vec![(
        "/System/Info/Public",
        json(200, r#"{"status":"ok"}"#.into()),
    )])
    .await;
    assert_eq!(
        probe(&other.address("")).await,
        Err(ProbeError::NotJellyfin)
    );
    // No such endpoint.
    let empty = StandIn::start(Vec::new()).await;
    assert_eq!(
        probe(&empty.address("")).await,
        Err(ProbeError::NotJellyfin)
    );
}

/// A server with the base path `/jellyfin`: like Jellyfin's `BaseUrlRedirectionMiddleware`, any path outside it is sent to the web
/// client with a relative redirect.
async fn based() -> StandIn {
    StandIn::start(vec![
        ("/System/Info/Public", redirect(302, "../../jellyfin/web/")),
        ("/jellyfin/web/", html(200)),
        (
            "/jellyfin/System/Info/Public",
            json(200, info(ID, "12.2.0", true)),
        ),
    ])
    .await
}

#[tokio::test]
async fn a_base_path_is_found_from_the_servers_redirect() {
    let server = based().await;
    let (prober, log) = prober();
    let found = ok(prober.probe(&server.address("")).await);
    assert_eq!(found.server_address(false).url, server.address("/jellyfin"));
    assert_eq!(found.info.id, ServerId::new(ID));
    // The first probe, then the one under the base path.
    assert_eq!(log.entries().len(), 2);
}

#[tokio::test]
async fn a_pasted_web_client_url_finds_the_base_path() {
    let server = based().await;
    let found = ok(probe(&server.address("/jellyfin/web/index.html#/home")).await);
    assert_eq!(found.server_address(false).url, server.address("/jellyfin"));
}

#[tokio::test]
async fn a_redirect_to_another_host_is_offered_not_followed() {
    let server = StandIn::start(vec![(
        "/System/Info/Public",
        redirect(301, "https://media.example.com/System/Info/Public"),
    )])
    .await;
    let error = probe(&server.address("")).await.err();
    let Some(ProbeError::Redirected {
        status: 301,
        location: Some(location),
    }) = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!(location.url().host_str(), Some("media.example.com"));
    let shown = format!(
        "{error:?} {}",
        error.as_ref().map(ToString::to_string).unwrap_or_default()
    );
    assert!(!shown.contains("media.example.com"), "{shown}");
}

#[tokio::test]
async fn the_same_server_on_two_addresses_has_one_id() {
    let answer = || vec![("/System/Info/Public", json(200, info(ID, "12.2.0", true)))];
    let (a, b) = (
        StandIn::start(answer()).await,
        StandIn::start(answer()).await,
    );
    let other = StandIn::start(vec![(
        "/System/Info/Public",
        json(
            200,
            info("fedcba9876543210fedcba9876543210", "12.2.0", true),
        ),
    )])
    .await;
    let (prober, _) = prober();
    let found_a = ok(prober.probe(&a.address("")).await);
    let found_b = ok(prober.probe(&b.address("")).await);
    let found_other = ok(prober.probe(&other.address("")).await);
    assert!(same_server(&found_a.info, &found_b.info));
    assert!(!same_server(&found_a.info, &found_other.info));

    let expected = ServerId::new(ID);
    let url = |server: &StandIn| ok(Url::parse(&server.address("")));
    assert!(prober.probe_expecting(&url(&b), &expected).await.is_ok());
    assert_eq!(
        prober.probe_expecting(&url(&other), &expected).await,
        Err(ProbeError::DifferentServer)
    );
}

#[tokio::test]
async fn http_found_after_a_failed_https_handshake_says_so() {
    let server = StandIn::start(vec![(
        "/System/Info/Public",
        json(200, info(ID, "12.2.0", true)),
    )])
    .await;
    let (prober, log) = prober();
    // No scheme and an unusual port: https is tried first, and this http-only server fails its handshake.
    let found = ok(prober.probe(&format!("127.0.0.1:{}", server.port)).await);
    assert_eq!(found.transport, Transport::PlainLocal);
    assert!(found.https_failed_tls);
    let entries = log.entries();
    assert_eq!(entries.len(), 2);
    assert_clean(&log.to_text(), &server);
}

#[tokio::test]
async fn the_most_telling_error_is_reported() {
    let server = StandIn::start(vec![("/System/Info/Public", html(503))]).await;
    let (prober, _) = prober();
    let found = prober.probe(&format!("127.0.0.1:{}", server.port)).await;
    // https fails its handshake (Tls), http answers 503 (Unavailable): the 503 is reported.
    assert_eq!(found, Err(ProbeError::Unavailable));
    assert_eq!(
        prober.probe("http://127.0.0.1:1").await,
        Err(ProbeError::Unreachable)
    );
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_tried_once() {
    let (prober, log) = prober();
    // `.invalid` never resolves (RFC 6761). Whatever the first candidate ends with (a proxy may answer for it), no other is tried.
    assert!(prober.probe("media.invalid").await.is_err());
    assert_eq!(log.entries().len(), 1);
}

#[tokio::test]
async fn bad_input_never_reaches_the_network() {
    let (prober, log) = prober();
    assert!(matches!(
        prober.probe("ftp://media.example.com").await,
        Err(ProbeError::InvalidAddress(_))
    ));
    assert!(log.entries().is_empty());
}
