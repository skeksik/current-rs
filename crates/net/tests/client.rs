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

mod ports;
use ports::Ports;

const SECRET: &str = "s3cret-token-value";

/// What the stand-in server does with each connection.
#[derive(Clone)]
enum Reply {
    /// Answers with this status and body.
    With(u16, &'static str),
    /// Answers this redirect status to `/start` with this location, and 200 to anything else.
    Redirect(u16, String),
    /// Answers 200 with this body and no content-length: the body ends when the connection closes.
    Unsized(&'static str),
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
        let listener = ports::bind().await;
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
                        log.push(head.clone());
                    }
                    let answer = |status: u16, extra: &str, body: &str| {
                        format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let response = match &reply {
                        Reply::With(status, body) => Some(answer(*status, "", body)),
                        Reply::Redirect(status, location) if head.contains(" /base/start") => {
                            Some(answer(*status, &format!("location: {location}\r\n"), ""))
                        }
                        Reply::Redirect(..) => Some(answer(200, "", "{}")),
                        Reply::Unsized(body) => Some(format!(
                            "HTTP/1.1 200 X\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n{body}"
                        )),
                        Reply::Silent | Reply::PlainHttp => None,
                    };
                    if let Some(response) = response {
                        let _ = socket.write_all(response.as_bytes()).await;
                        let _ = socket.shutdown().await;
                    }
                    match reply {
                        Reply::With(..) | Reply::Redirect(..) | Reply::Unsized(_) => {}
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
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Test s3cret-token-value"),
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
            message: Some("Token expired".into()),
            challenges: Vec::new(),
            page: false,
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

/// Sends `request` to a server that redirects `/start` with `status` to `/base/end?page=2`, and returns what the server received.
async fn follow(status: u16, request: Request) -> (Vec<String>, Arc<RequestLog>) {
    let server = StandIn::start(Reply::Redirect(status, "/base/end?page=2".into())).await;
    let log = Arc::new(RequestLog::default());
    let response = ok(client(&server.address, &log, Duration::from_secs(5))
        .send(request)
        .await);
    assert_eq!(response.status, 200);
    (server.received(), log)
}

#[tokio::test]
async fn a_redirect_on_the_same_server_is_followed_with_credentials() {
    let (received, log) = follow(302, Request::get("/start")).await;
    assert_eq!(received.len(), 2);
    // The credentials are applied again on the new hop: header, Authorization and query pair (added to the location's own query).
    assert!(
        received[1].starts_with(&format!("GET /base/end?page=2&api_key={SECRET} HTTP/1.1")),
        "{}",
        received[1]
    );
    assert!(received[1].contains(&format!("x-test-token: {SECRET}")));
    assert!(received[1].contains(&format!("authorization: Test {SECRET}")));
    // One logged request: the one the caller made.
    assert_eq!(log.entries().len(), 1);
    assert!(!log.to_text().contains(SECRET));
}

#[tokio::test]
async fn a_response_tells_where_it_came_from_without_showing_it() {
    let server = StandIn::start(Reply::Redirect(302, "/base/end?page=2".into())).await;
    let log = Arc::new(RequestLog::default());
    let response = ok(client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/start"))
        .await);
    // The final hop, as the caller would have built it: no credentials.
    assert_eq!(response.url.url().path(), "/base/end");
    assert_eq!(response.url.url().query(), Some("page=2"));
    assert_eq!(response.url.url().host_str(), Some("127.0.0.1"));
    assert_clean(&format!("{response:?}"), &server.address);
    assert_clean(&log.to_text(), &server.address);
}

fn post() -> Request {
    ok(Request::post("/start").json(&serde_json::json!({ "a": 1 })))
}

#[tokio::test]
async fn see_other_and_moved_turn_a_post_into_a_get_without_body() {
    for status in [301, 302, 303] {
        let (received, _) = follow(status, post()).await;
        assert!(
            received[0].starts_with("POST /base/start"),
            "{}",
            received[0]
        );
        assert!(
            received[1].starts_with("GET /base/end"),
            "{status}: {}",
            received[1]
        );
        assert!(
            !received[1].contains("content-type"),
            "{status}: {}",
            received[1]
        );
        assert!(
            !received[1].contains("content-length: 7"),
            "{status}: {}",
            received[1]
        );
    }
}

#[tokio::test]
async fn temporary_and_permanent_redirects_keep_the_method_and_body() {
    for status in [307, 308] {
        let (received, _) = follow(status, post()).await;
        assert!(
            received[1].starts_with("POST /base/end"),
            "{status}: {}",
            received[1]
        );
        assert!(
            received[1].contains("content-type: application/json"),
            "{status}: {}",
            received[1]
        );
        assert!(
            received[1].contains("content-length: 7"),
            "{status}: {}",
            received[1]
        );
    }
}

#[tokio::test]
async fn a_redirect_loop_stops_after_a_few_hops() {
    let server = StandIn::start(Reply::Redirect(302, "/base/start".into())).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/start"))
        .await
        .err();
    assert!(
        matches!(error, Some(NetError::Redirected { status: 302, .. })),
        "{error:?}"
    );
    // The first request and five followed hops.
    assert_eq!(server.received().len(), 6);
}

#[tokio::test]
async fn a_redirect_to_another_server_is_not_followed() {
    let other = StandIn::start(Reply::With(200, "{}")).await;
    let target = ok(other.address.join("end"));
    let server = StandIn::start(Reply::Redirect(302, target.to_string())).await;
    let log = Arc::new(RequestLog::default());
    let error = client(&server.address, &log, Duration::from_secs(5))
        .send(Request::get("/start"))
        .await
        .err();

    let Some(NetError::Redirected {
        status: 302,
        location: Some(location),
    }) = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!(location.url(), &target);
    // The other server got nothing, so neither did the token.
    assert!(other.received().is_empty());
    assert_eq!(log.entries()[0].outcome, Outcome::Error("redirected"));
    assert_clean(
        &format!(
            "{error:?} {} {}",
            error.as_ref().map(ToString::to_string).unwrap_or_default(),
            log.to_text()
        ),
        &other.address,
    );
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
async fn a_body_over_the_limit_is_too_large_with_or_without_a_length() {
    for reply in [Reply::With(200, "0123456789"), Reply::Unsized("0123456789")] {
        let server = StandIn::start(reply).await;
        let log = Arc::new(RequestLog::default());
        let client = client(&server.address, &log, Duration::from_secs(5));
        let error = client.send(Request::get("/list").max_body(9)).await;
        assert!(matches!(error, Err(NetError::TooLarge)), "{error:?}");
        assert_eq!(log.entries()[0].outcome, Outcome::Error("too large"));
        let response = ok(client.send(Request::get("/list").max_body(10)).await);
        assert_eq!(response.body, b"0123456789");
    }
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
    let ports = Ports::hold().await;
    let address = ok(Url::parse(&format!(
        "http://127.0.0.1:{}/",
        ports.refused().await
    )));
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
            challenges: Vec::new(),
            page: false,
        },
        NetError::Status {
            status: 500,
            message: None,
        },
        NetError::Decode { line: 1, column: 2 },
        NetError::Redirected {
            status: 301,
            location: None,
        },
        NetError::InvalidRequest,
    ];
    for error in errors {
        assert_clean(&format!("{error} {error:?}"), &address);
    }
}
