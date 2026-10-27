//! `ServerClient::open_socket` (ADR 0023) against stand-ins on 127.0.0.1: the upgrade carries the service's and the gateway's
//! credentials and the socket carries text both ways; a wrong `Sec-WebSocket-Accept`, a refusal and a redirect are errors; the log
//! names no secret.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_model::{ServerKey, ServerRef, ServiceType};
use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{
    AuthorizationUse, BasicCredentials, ClientConfig, ExtraHeader, FixedAddress, Gateway,
    GatewayAddress, GatewayCookie, GatewayCredentials, NetError, Outcome, Request, RequestAuth,
    RequestLog, ServerClient, Url,
};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request as Upgrade, Response};

const TOKEN: &str = "s3cret-token-value";
const USER: &str = "gate-user";
const PASSWORD: &str = "gate-pa55word";
/// `gate-user:gate-pa55word` in base64.
const BASIC: &str = "Z2F0ZS11c2VyOmdhdGUtcGE1NXdvcmQ=";
const COOKIE_NAME: &str = "gate_session_name";
const COOKIE_VALUE: &str = "gate-cookie-value";
const HEADER_NAME: &str = "x-gate-key";
const HEADER_VALUE: &str = "gate-header-value";

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{error:?}"),
    }
}

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

fn config() -> ClientConfig {
    ClientConfig {
        timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(5),
    }
}

struct TokenAuth;

impl RequestAuth for TokenAuth {
    fn apply(&self, headers: &mut HeaderMap, _query: &mut Vec<(String, String)>) {
        headers.insert(
            HeaderName::from_static("x-test-token"),
            HeaderValue::from_static(TOKEN),
        );
    }
}

fn gateway() -> Arc<Gateway> {
    Arc::new(Gateway::new(ok(GatewayCredentials::new(
        Some(ok(BasicCredentials::new(USER, PASSWORD))),
        vec![ok(ExtraHeader::new(
            HEADER_NAME,
            HEADER_VALUE,
            AuthorizationUse::Service,
        ))],
        vec![ok(GatewayCookie::new(
            COOKIE_NAME,
            COOKIE_VALUE,
            "127.0.0.1",
        ))],
    ))))
}

fn url(port: u16) -> Url {
    ok(Url::parse(&format!("http://127.0.0.1:{port}/")))
}

/// A listener on a free port of 127.0.0.1. No test here frees a port another relies on, so no port lock is needed.
async fn bind() -> TcpListener {
    ok(TcpListener::bind("127.0.0.1:0").await)
}

/// A WebSocket server for one connection: it keeps the upgrade's headers, sends "hello", and keeps the first text it receives.
// tungstenite's callback returns its own large error type: not ours to box.
#[allow(clippy::result_large_err)]
async fn echo_server() -> (
    u16,
    Arc<Mutex<Vec<(String, String)>>>,
    Arc<Mutex<Option<String>>>,
) {
    let listener = bind().await;
    let port = ok(listener.local_addr()).port();
    let headers = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(None));
    let (seen, got) = (headers.clone(), received.clone());
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let callback = |request: &Upgrade, response: Response| {
            let mut seen = seen.lock().unwrap();
            for (name, value) in request.headers() {
                seen.push((
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or_default().to_owned(),
                ));
            }
            Ok(response)
        };
        let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
            return;
        };
        let _ = socket.send(Message::text("hello")).await;
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message {
                *got.lock().unwrap() = Some(text.as_str().to_owned());
                break;
            }
        }
    });
    (port, headers, received)
}

/// A server that answers the upgrade with `answer`, as written.
async fn answering(answer: &'static str) -> u16 {
    let listener = bind().await;
    let port = ok(listener.local_addr()).port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer).await;
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    port
}

#[tokio::test]
async fn the_upgrade_carries_every_credential_and_the_socket_carries_text_both_ways() {
    let (port, headers, received) = echo_server().await;
    let log = Arc::new(RequestLog::new(16));
    let client = ok(ServerClient::new(
        server(),
        Arc::new(GatewayAddress::new(url(port), gateway(), false)),
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config(),
    ));
    let mut socket = ok(client.open_socket(Request::get("/socket")).await);
    assert_eq!(ok(socket.next_text().await), "hello");
    ok(socket.send_text("back".to_owned()).await);
    for _ in 0..200 {
        if received.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(received.lock().unwrap().as_deref(), Some("back"));

    let headers = headers.lock().unwrap().clone();
    let value = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(value("x-test-token").as_deref(), Some(TOKEN));
    assert_eq!(value(HEADER_NAME).as_deref(), Some(HEADER_VALUE));
    assert_eq!(value("authorization"), Some(format!("Basic {BASIC}")));
    assert!(
        value("cookie")
            .is_some_and(|cookie| cookie.contains(&format!("{COOKIE_NAME}={COOKIE_VALUE}"))),
        "{headers:?}"
    );

    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, Outcome::Status(101));
    let text = log.to_text();
    for secret in [TOKEN, PASSWORD, BASIC, COOKIE_VALUE, HEADER_VALUE] {
        assert!(!text.contains(secret), "{text}");
    }
}

fn plain(port: u16) -> ServerClient {
    ok(ServerClient::new(
        server(),
        Arc::new(FixedAddress(url(port))),
        Some(Arc::new(TokenAuth)),
        Arc::new(RequestLog::new(16)),
        &config(),
    ))
}

#[tokio::test]
async fn an_answer_that_is_not_an_upgrade_is_an_error() {
    let wrong_key = answering(
        "HTTP/1.1 101 Switching Protocols\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: bm90LWEta2V5\r\n\r\n",
    )
    .await;
    let refused =
        answering("HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
    let moved = answering(
        "HTTP/1.1 302 Found\r\nlocation: /sign-in\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    )
    .await;
    let plain_ok =
        answering("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;

    let error = plain(wrong_key).open_socket(Request::get("/socket")).await;
    assert!(
        matches!(error, Err(NetError::Status { status: 101, .. })),
        "{error:?}"
    );
    let error = plain(refused).open_socket(Request::get("/socket")).await;
    assert!(
        matches!(error, Err(NetError::Auth { status: 403, .. })),
        "{error:?}"
    );
    let error = plain(moved).open_socket(Request::get("/socket")).await;
    assert!(
        matches!(error, Err(NetError::Redirected { status: 302, .. })),
        "{error:?}"
    );
    let error = plain(plain_ok).open_socket(Request::get("/socket")).await;
    assert!(
        matches!(error, Err(NetError::Status { status: 200, .. })),
        "{error:?}"
    );
}
