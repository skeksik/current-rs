//! `ServerClient` against a hand-written stand-in server on 127.0.0.1.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_model::{ServerKey, ServerRef, ServiceType};
use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{
    AddressProvider, ClientConfig, FixedAddress, NetError, Outcome, Request, RequestAuth,
    RequestLog, ServerClient, Url,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SECRET: &str = "s3cret-token-value";

/// What the stand-in server does with each connection.
#[derive(Clone)]
enum Reply {
    /// Answers with this status and body.
    With(u16, &'static str),
    /// Reads the request and never answers.
    Silent,
    /// Answers in plain HTTP as soon as the connection opens, without reading (what a TLS client meets at a plain-HTTP port).
    PlainHttp,
}

/// A server on 127.0.0.1 that answers every request the same way and keeps the raw requests it received.
struct StandIn {
    address: Url,
    received: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    async fn start(reply: Reply) -> Self {
        let listener = ok(TcpListener::bind("127.0.0.1:0").await);
        let port = ok(listener.local_addr()).port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let reply = reply.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    if let Reply::PlainHttp = reply {
                        let _ = socket
                            .write_all(b"HTTP/1.1 400 Bad Request\r\nconnection: close\r\n\r\n")
                            .await;
                        let _ = socket.shutdown().await;
                        return;
                    }
                    let head = read_head(&mut socket).await;
                    if let Ok(mut log) = log.lock() {
                        log.push(head);
                    }
                    match reply {
                        Reply::With(status, body) => {
                            let response = format!(
                                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                            let _ = socket.shutdown().await;
                        }
                        Reply::Silent => tokio::time::sleep(Duration::from_secs(30)).await,
                        Reply::PlainHttp => {}
                    }
                });
            }
        });
        Self {
            address: ok(Url::parse(&format!("http://127.0.0.1:{port}/base/"))),
            received,
        }
    }

    fn received(&self) -> Vec<String> {
        self.received.lock().map(|r| r.clone()).unwrap_or_default()
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

fn client(address: &Url, log: &Arc<RequestLog>, timeout: Duration) -> ServerClient {
    let config = ClientConfig {
        timeout,
        connect_timeout: timeout,
    };
    let address: Arc<dyn AddressProvider> = Arc::new(FixedAddress(address.clone()));
    ok(ServerClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        address,
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config,
    ))
}

/// Nothing a user would paste may contain the server's address or the secret.
fn assert_clean(text: &str, address: &Url) {
    assert!(!text.contains(SECRET), "secret leaked: {text}");
    assert!(!text.contains("127.0.0.1"), "address leaked: {text}");
    if let Some(port) = address.port() {
        assert!(!text.contains(&format!(":{port}")), "port leaked: {text}");
    }
}

#[tokio::test]
async fn a_success_is_decoded_and_logged_without_secrets() {
    let server = StandIn::start(Reply::With(200, r#"{"Name":"x"}"#)).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server.address, &log, Duration::from_secs(5));

    let value: serde_json::Value = ok(client
        .send_json(
            Request::get("/Items")
                .query("searchTerm", "Some Title")
                .query("Limit", "5"),
        )
        .await);
    assert_eq!(value["Name"], "x");

    // The server got the credentials, under the base path.
    let received = server.received().join("\n");
    assert!(received.starts_with(
        "GET /base/Items?searchTerm=Some+Title&Limit=5&api_key=s3cret-token-value HTTP/1.1"
    ));
    assert!(received.contains(&format!("x-test-token: {SECRET}")));

    // The log did not: it holds the request as the caller built it, redacted.
    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, Outcome::Status(200));
    let text = log.to_text();
    assert!(
        text.contains("jellyfin/1 GET /Items?searchTerm=[redacted]&Limit=5 -> 200"),
        "{text}"
    );
    assert!(
        !text.contains("api_key") && !text.contains("Some Title"),
        "{text}"
    );
    assert_clean(&text, &server.address);
}

#[tokio::test]
async fn auth_statuses_carry_the_server_message_but_the_log_does_not() {
    let server = StandIn::start(Reply::With(401, r#"{"message":"Token expired"}"#)).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/Users/Me"))
        .await;
    assert_eq!(
        error.as_ref().err(),
        Some(&NetError::Auth {
            status: 401,
            message: Some("Token expired".into())
        })
    );
    let text = log.to_text();
    assert!(text.contains("GET /Users/Me -> 401"), "{text}");
    assert!(!text.contains("Token expired"), "{text}");
    assert_clean(&format!("{error:?}"), &server.address);
}

#[tokio::test]
async fn other_statuses_are_status_errors() {
    let server = StandIn::start(Reply::With(503, r#"{"detail":"Server is starting"}"#)).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/System/Info"))
        .await
        .err();
    assert_eq!(
        error,
        Some(NetError::Status {
            status: 503,
            message: Some("Server is starting".into())
        })
    );
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let server = StandIn::start(Reply::With(302, "")).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/"))
        .await
        .err();
    assert_eq!(
        error,
        Some(NetError::Status {
            status: 302,
            message: None
        })
    );
    assert_eq!(server.received().len(), 1);
}

#[tokio::test]
async fn an_unexpected_body_is_a_decode_error() {
    let server = StandIn::start(Reply::With(200, r#"{"Name":"Some Library Title"}"#)).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send_json::<Vec<u32>>(Request::get("/Items"))
        .await;
    let Err(error) = error else { panic!("decoded") };
    assert!(
        matches!(error, NetError::Decode { line: 1, .. }),
        "{error:?}"
    );
    assert!(!format!("{error} {error:?}").contains("Title"));
    // The transfer itself succeeded.
    assert_eq!(log.entries()[0].outcome, Outcome::Status(200));
}

#[tokio::test]
async fn a_silent_server_times_out() {
    let server = StandIn::start(Reply::Silent).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server.address, &log, Duration::from_secs(30));
    let started = std::time::Instant::now();
    let error = client
        .send(Request::get("/Items").timeout(Duration::from_millis(200)))
        .await
        .err();
    assert_eq!(error, Some(NetError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(log.entries()[0].outcome, Outcome::Error("timeout"));
    assert_clean(&format!("{error:?}"), &server.address);
}

#[tokio::test]
async fn the_client_timeout_applies_without_a_request_timeout() {
    let server = StandIn::start(Reply::Silent).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_millis(200))
        .send(Request::get("/Items"))
        .await
        .err();
    assert_eq!(error, Some(NetError::Timeout));
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    // Bind to get a free port, then close it.
    let listener = ok(TcpListener::bind("127.0.0.1:0").await);
    let address = ok(Url::parse(&format!(
        "http://127.0.0.1:{}/",
        ok(listener.local_addr()).port()
    )));
    drop(listener);
    let log = Arc::new(RequestLog::default());
    let error = client(&address, &log, Duration::from_secs(5))
        .send(Request::get("/Items"))
        .await;
    assert_eq!(error.as_ref().err(), Some(&NetError::Unreachable));
    assert_clean(&format!("{error:?} {}", log.to_text()), &address);
}

#[tokio::test]
async fn a_failed_handshake_is_a_tls_error() {
    // A plain-HTTP server answering a TLS ClientHello.
    let server = StandIn::start(Reply::PlainHttp).await;
    let mut address = server.address.clone();
    ok(address.set_scheme("https"));
    let log = Arc::new(RequestLog::default());
    let error = client(&address, &log, Duration::from_secs(5))
        .send(Request::get("/Items"))
        .await;
    assert_eq!(error.as_ref().err(), Some(&NetError::Tls));
    assert_eq!(log.entries()[0].outcome, Outcome::Error("tls"));
    assert_clean(&format!("{error:?} {}", log.to_text()), &address);
}

#[tokio::test]
async fn every_error_displays_without_the_address() {
    let address = ok(Url::parse("http://127.0.0.1:9/"));
    let errors = [
        NetError::Unreachable,
        NetError::Timeout,
        NetError::Tls,
        NetError::Auth {
            status: 403,
            message: None,
        },
        NetError::Status {
            status: 500,
            message: None,
        },
        NetError::Decode { line: 1, column: 2 },
        NetError::InvalidRequest,
    ];
    for error in errors {
        assert_clean(&format!("{error} {error:?}"), &address);
    }
}
