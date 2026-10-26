//! `ServerClient::send_stream` (ADR 0020, task 1) against hand-written stand-ins on 127.0.0.1: large and paused bodies, ranges, a
//! cookie rotated while a stream is open, refusals, a dropped response, read timeouts and the request log.
//!
//! Real time, with short timings and wide margins.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_model::{ServerKey, ServerRef, ServiceType};
use current_net::header::{self, HeaderMap, HeaderName, HeaderValue};
use current_net::{
    AuthorizationUse, BasicCredentials, ClientConfig, ExtraHeader, FixedAddress, Gateway,
    GatewayAddress, GatewayCookie, GatewayCredentials, GatewayShape, Request, RequestAuth,
    RequestLog, ServerClient, StreamError, StreamResponse, Url,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};

// No test here frees a port, so only `ports::bind` is used.
#[allow(dead_code)]
mod ports;

const TOKEN: &str = "s3cret-token-value";
const USER: &str = "gate-user";
const PASSWORD: &str = "gate-pa55word";
/// `gate-user:gate-pa55word` in base64.
const BASIC: &str = "Z2F0ZS11c2VyOmdhdGUtcGE1NXdvcmQ=";
const COOKIE_NAME: &str = "gate_session_name";
const COOKIE_VALUE: &str = "gate-cookie-value";
const ROTATED_VALUE: &str = "gate-cookie-rotated";
const HEADER_NAME: &str = "x-gate-key";
const HEADER_VALUE: &str = "gate-header-value";
const RANGE_VALUE: &str = "bytes=100-199";
const ETAG_VALUE: &str = "\"etag-value-1\"";

/// Every secret and header value no log line, error or Debug output may show.
const SECRETS: &[&str] = &[
    TOKEN,
    PASSWORD,
    BASIC,
    COOKIE_NAME,
    COOKIE_VALUE,
    ROTATED_VALUE,
    HEADER_VALUE,
    RANGE_VALUE,
    ETAG_VALUE,
];

const MIB: usize = 1024 * 1024;

/// A server on its own port of 127.0.0.1: each connection's request head is kept, then handed with the socket to the handler,
/// which answers one request.
struct StandIn {
    port: u16,
    received: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(String, TcpStream) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let listener = ports::bind().await;
        let port = ok(listener.local_addr()).port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let head = read_head(&mut socket).await;
                    log.lock().unwrap().push(head.clone());
                    handler(head, socket).await;
                });
            }
        });
        Self { port, received }
    }

    fn url(&self) -> Url {
        ok(Url::parse(&format!("http://127.0.0.1:{}/", self.port)))
    }

    fn received(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }
}

async fn read_head(socket: &mut TcpStream) -> String {
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

/// A response head with these extra header lines, closing the connection after the body.
fn head(status: u16, length: usize, headers: &[&str]) -> String {
    let mut text = format!("HTTP/1.1 {status} X\r\ncontent-length: {length}\r\n");
    for header in headers {
        text.push_str(header);
        text.push_str("\r\n");
    }
    text.push_str("connection: close\r\n\r\n");
    text
}

/// A whole answer.
async fn answer(mut socket: TcpStream, status: u16, headers: &[&str], body: &str) {
    let text = head(status, body.len(), headers) + body;
    let _ = socket.write_all(text.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The value of header `name` in a request head, if present (names compared without case).
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

/// The byte at `offset` of the stand-ins' generated bodies.
fn pattern(offset: usize) -> u8 {
    (offset % 251) as u8
}

/// `length` bytes of the pattern starting at `offset`.
fn pattern_bytes(offset: usize, length: usize) -> Vec<u8> {
    (offset..offset + length).map(pattern).collect()
}

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

/// A service token as a header, the way Jellyfin's is.
struct TokenAuth;

impl RequestAuth for TokenAuth {
    fn apply(&self, headers: &mut HeaderMap, _query: &mut Vec<(String, String)>) {
        headers.insert(
            HeaderName::from_static("x-test-token"),
            HeaderValue::from_static(TOKEN),
        );
    }
}

fn plain_client(stand_in: &StandIn, log: &Arc<RequestLog>) -> ServerClient {
    ok(ServerClient::new(
        server(),
        Arc::new(FixedAddress(stand_in.url())),
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config(),
    ))
}

fn gateway_client(
    stand_in: &StandIn,
    gateway: &Arc<Gateway>,
    log: &Arc<RequestLog>,
) -> ServerClient {
    ok(ServerClient::new(
        server(),
        Arc::new(GatewayAddress::new(stand_in.url(), gateway.clone(), false)),
        Some(Arc::new(TokenAuth)),
        log.clone(),
        &config(),
    ))
}

fn everything() -> GatewayCredentials {
    ok(GatewayCredentials::new(
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
    ))
}

/// Reads the rest of the body, checking it against the pattern from `offset`; returns how many bytes there were.
async fn read_pattern(stream: &mut StreamResponse, mut offset: usize) -> usize {
    let start = offset;
    while let Some(chunk) = ok(stream.next_chunk().await) {
        assert_eq!(
            chunk.as_ref(),
            pattern_bytes(offset, chunk.len()),
            "at {offset}"
        );
        offset += chunk.len();
    }
    offset - start
}

/// Writes `length` bytes of the pattern in 64 KiB writes after `head`, counting what the socket took in `written`.
async fn write_pattern(socket: &mut TcpStream, length: usize, written: &AtomicU64) {
    let mut offset = 0;
    while offset < length {
        let size = (64 * 1024).min(length - offset);
        if socket
            .write_all(&pattern_bytes(offset, size))
            .await
            .is_err()
        {
            return;
        }
        offset += size;
        written.store(offset as u64, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_large_body_streams_with_bounded_memory() {
    const LENGTH: usize = 50 * MIB;
    let written = Arc::new(AtomicU64::new(0));
    let counter = written.clone();
    let stand_in = StandIn::start(move |_, mut socket| {
        let counter = counter.clone();
        async move {
            let head = head(200, LENGTH, &["content-type: video/mp4"]);
            let _ = socket.write_all(head.as_bytes()).await;
            write_pattern(&mut socket, LENGTH, &counter).await;
            let _ = socket.shutdown().await;
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let mut stream = ok(client.send_stream(Request::get("/Videos/1/stream")).await);
    assert_eq!(stream.status(), 200);

    // Read 1 MiB, then stop reading: the stand-in's writes stop once the socket buffers are full, far short of the body.
    let mut read = 0;
    while read < MIB {
        let Some(chunk) = ok(stream.next_chunk().await) else {
            panic!("the body ended at {read}")
        };
        assert_eq!(chunk.as_ref(), pattern_bytes(read, chunk.len()));
        assert!(chunk.len() <= MIB, "a chunk of {} bytes", chunk.len());
        read += chunk.len();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stalled = written.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        written.load(Ordering::SeqCst),
        stalled,
        "the stand-in kept writing"
    );
    // Socket buffers on both ends and hyper's read buffer hold the difference; Linux allows a few MiB per socket by default.
    assert!(
        stalled < (LENGTH / 2) as u64,
        "{stalled} bytes written while the client read {read}"
    );

    // Reading again takes the rest, byte for byte.
    assert_eq!(read + read_pattern(&mut stream, read).await, LENGTH);
    assert_eq!(written.load(Ordering::SeqCst), LENGTH as u64);
    // A success is not logged.
    assert_eq!(log.to_text(), "");
}

#[tokio::test]
async fn a_pause_of_the_server_holds_the_reader_without_an_error() {
    let stand_in = StandIn::start(|_, mut socket| async move {
        let written = AtomicU64::new(0);
        let _ = socket.write_all(head(200, 2 * MIB, &[]).as_bytes()).await;
        write_pattern(&mut socket, MIB, &written).await;
        let _ = socket.flush().await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let rest = pattern_bytes(MIB, MIB);
        let _ = socket.write_all(&rest).await;
        let _ = socket.shutdown().await;
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let request = Request::get("/Videos/1/stream").read_timeout(Duration::from_secs(10));
    let mut stream = ok(client.send_stream(request).await);
    let mut offset = 0;
    let mut longest = Duration::ZERO;
    loop {
        let waited = tokio::time::Instant::now();
        let Some(chunk) = ok(stream.next_chunk().await) else {
            break;
        };
        longest = longest.max(waited.elapsed());
        assert_eq!(chunk.as_ref(), pattern_bytes(offset, chunk.len()));
        offset += chunk.len();
    }
    assert_eq!(offset, 2 * MIB);
    // One read waited out the stand-in's pause.
    assert!(longest >= Duration::from_millis(1000), "{longest:?}");
}

#[tokio::test]
async fn a_caller_that_stops_reading_does_not_time_out() {
    let stand_in = StandIn::start(|_, mut socket| async move {
        let written = AtomicU64::new(0);
        let _ = socket.write_all(head(200, 4 * MIB, &[]).as_bytes()).await;
        write_pattern(&mut socket, 4 * MIB, &written).await;
        let _ = socket.shutdown().await;
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let request = Request::get("/Videos/1/stream").read_timeout(Duration::from_millis(300));
    let mut stream = ok(client.send_stream(request).await);
    let first = ok(stream.next_chunk().await).map_or(0, |chunk| chunk.len());
    // A paused player: three read timeouts without reading. The time only runs while a read waits.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(first + read_pattern(&mut stream, first).await, 4 * MIB);
}

#[tokio::test]
async fn range_and_if_range_are_forwarded_and_206_comes_back() {
    let stand_in = StandIn::start(|_, socket| async move {
        let body = String::from_utf8(vec![b'x'; 100]).unwrap();
        answer(
            socket,
            206,
            &[
                "content-type: video/mp4",
                "content-range: bytes 100-199/1000",
                "accept-ranges: bytes",
                &format!("etag: {ETAG_VALUE}"),
                "last-modified: Mon, 26 Oct 2026 10:00:00 GMT",
                "cache-control: no-cache",
                "set-cookie: tracking=value-from-server; Path=/",
                "x-internal: something",
            ],
            &body,
        )
        .await;
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let request = Request::get("/Videos/1/stream")
        .query("static", "true")
        .header(header::RANGE, HeaderValue::from_static(RANGE_VALUE))
        .header(header::IF_RANGE, HeaderValue::from_static(ETAG_VALUE))
        .header(header::ACCEPT, HeaderValue::from_static("*/*"))
        // Dropped: none of these is on the list.
        .header(header::COOKIE, HeaderValue::from_static("player=cookie"))
        .header(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic player"),
        )
        .header(header::USER_AGENT, HeaderValue::from_static("player-agent"))
        .header(
            HeaderName::from_static("x-player"),
            HeaderValue::from_static("1"),
        );
    let mut stream = ok(client.send_stream(request).await);

    let received = stand_in.received();
    assert_eq!(received.len(), 1);
    let sent = &received[0];
    assert!(
        sent.starts_with("GET /Videos/1/stream?static=true "),
        "{sent}"
    );
    assert_eq!(header(sent, "range"), Some(RANGE_VALUE));
    assert_eq!(header(sent, "if-range"), Some(ETAG_VALUE));
    assert_eq!(header(sent, "accept"), Some("*/*"));
    assert_eq!(header(sent, "x-test-token"), Some(TOKEN));
    for dropped in ["cookie", "authorization", "x-player"] {
        assert_eq!(header(sent, dropped), None, "{dropped} in {sent}");
    }
    assert!(!sent.contains("player-agent"), "{sent}");

    assert_eq!(stream.status(), 206);
    assert_eq!(stream.base(), &stand_in.url());
    let headers = stream.headers();
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 100-199/1000");
    assert_eq!(headers[header::CONTENT_LENGTH], "100");
    assert_eq!(headers[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
    assert_eq!(headers[header::ETAG], ETAG_VALUE);
    assert!(headers.contains_key(header::LAST_MODIFIED));
    assert!(headers.contains_key(header::CACHE_CONTROL));
    assert_eq!(headers.len(), 7, "{headers:?}");
    let mut length = 0;
    while let Some(chunk) = ok(stream.next_chunk().await) {
        length += chunk.len();
    }
    assert_eq!(length, 100);
}

#[tokio::test]
async fn a_304_comes_back_with_its_headers() {
    let stand_in = StandIn::start(|_, mut socket| async move {
        let text =
            format!("HTTP/1.1 304 Not Modified\r\netag: {ETAG_VALUE}\r\nconnection: close\r\n\r\n");
        let _ = socket.write_all(text.as_bytes()).await;
        let _ = socket.shutdown().await;
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let request = Request::get("/Videos/1/master.m3u8")
        .header(header::IF_NONE_MATCH, HeaderValue::from_static(ETAG_VALUE));
    let mut stream = ok(client.send_stream(request).await);
    assert_eq!(stream.status(), 304);
    assert_eq!(stream.headers()[header::ETAG], ETAG_VALUE);
    assert_eq!(ok(stream.next_chunk().await), None);
    assert_eq!(
        header(&stand_in.received()[0], "if-none-match"),
        Some(ETAG_VALUE)
    );
}

#[tokio::test]
async fn a_cookie_set_mid_stream_updates_the_gateway() {
    let release = Arc::new(Notify::new());
    let held = release.clone();
    let stand_in = StandIn::start(move |head, mut socket| {
        let held = held.clone();
        async move {
            if head.starts_with("GET /Videos/1/stream ") {
                // The gateway rotates its cookie on a media answer, then the body takes its time.
                let cookie = format!("set-cookie: {COOKIE_NAME}={ROTATED_VALUE}; Path=/; HttpOnly");
                let _ = socket.write_all(head_text(&cookie).as_bytes()).await;
                let _ = socket.write_all(&pattern_bytes(0, 1024)).await;
                held.notified().await;
                let _ = socket.write_all(&pattern_bytes(1024, 1024)).await;
                let _ = socket.shutdown().await;
            } else {
                answer(socket, 200, &["content-type: video/mp2t"], "segment").await;
            }
        }
    })
    .await;
    fn head_text(cookie: &str) -> String {
        head(200, 2048, &["content-type: video/mp4", cookie])
    }
    let gateway = Arc::new(Gateway::new(everything()));
    let log = Arc::new(RequestLog::default());
    let client = gateway_client(&stand_in, &gateway, &log);

    let mut first = ok(client.send_stream(Request::get("/Videos/1/stream")).await);
    assert!(!first.headers().contains_key(header::SET_COOKIE));
    assert!(ok(first.next_chunk().await).is_some());
    // While the first stream is still open, the next request carries the rotated cookie.
    let mut second = ok(client.send_stream(Request::get("/Videos/1/hls/0.ts")).await);
    while ok(second.next_chunk().await).is_some() {}
    let received = stand_in.received();
    let sent = &received[1];
    assert_eq!(
        header(sent, "cookie"),
        Some(format!("{COOKIE_NAME}={ROTATED_VALUE}").as_str()),
        "{sent}"
    );
    // The address's other credentials went along, as for every request.
    assert_eq!(header(sent, HEADER_NAME), Some(HEADER_VALUE));
    assert_eq!(
        header(sent, "authorization"),
        Some(format!("Basic {BASIC}").as_str())
    );
    assert_eq!(
        header(&received[0], "cookie"),
        Some(format!("{COOKIE_NAME}={COOKIE_VALUE}").as_str())
    );
    let (credentials, _) = gateway.snapshot();
    assert_eq!(credentials.cookies()[0].value(), ROTATED_VALUE);

    release.notify_one();
    assert_eq!(read_pattern(&mut first, 1024).await, 1024);
}

#[tokio::test]
async fn a_redirect_is_a_refusal_with_nothing_followed() {
    let stand_in = StandIn::start(|head, socket| async move {
        if head.starts_with("GET /Videos/1/stream ") {
            answer(socket, 302, &["location: /Videos/1/elsewhere"], "").await;
        } else {
            answer(socket, 200, &[], "followed").await;
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let result = client.send_stream(Request::get("/Videos/1/stream")).await;
    assert_eq!(
        result.map(|_| ()),
        Err(StreamError::Refused {
            status: 302,
            shape: GatewayShape::SignIn
        })
    );
    assert_eq!(stand_in.received().len(), 1);
    assert!(
        log.to_text().contains("GET /Videos/1/stream -> 302"),
        "{}",
        log.to_text()
    );
}

#[tokio::test]
async fn refusals_carry_the_gateway_shape_and_other_statuses_their_status() {
    let stand_in = StandIn::start(|head, socket| async move {
        let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
        match path.as_str() {
            "/basic" => {
                answer(
                    socket,
                    401,
                    &["www-authenticate: Basic realm=\"Private Realm\""],
                    "",
                )
                .await
            }
            "/portal" => {
                answer(
                    socket,
                    403,
                    &["content-type: text/html"],
                    "<html>sign in</html>",
                )
                .await
            }
            "/headers" => answer(socket, 403, &[], "").await,
            "/page" => {
                answer(
                    socket,
                    200,
                    &["content-type: text/html; charset=utf-8"],
                    "<html></html>",
                )
                .await
            }
            "/missing" => answer(socket, 404, &[], "").await,
            _ => answer(socket, 500, &[], "").await,
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let error = async |path: &str| client.send_stream(Request::get(path)).await.err();
    assert_eq!(
        error("/basic").await,
        Some(StreamError::Refused {
            status: 401,
            shape: GatewayShape::Password {
                realm: Some("Private Realm".to_owned())
            }
        })
    );
    assert_eq!(
        error("/portal").await,
        Some(StreamError::Refused {
            status: 403,
            shape: GatewayShape::SignIn
        })
    );
    assert_eq!(
        error("/headers").await,
        Some(StreamError::Refused {
            status: 403,
            shape: GatewayShape::Refused
        })
    );
    assert_eq!(
        error("/page").await,
        Some(StreamError::Refused {
            status: 200,
            shape: GatewayShape::SignIn
        })
    );
    assert_eq!(
        error("/missing").await,
        Some(StreamError::Status { status: 404 })
    );
    assert_eq!(
        error("/broken").await,
        Some(StreamError::Status { status: 500 })
    );
    assert_eq!(
        client.send_stream(Request::post("/basic")).await.err(),
        Some(StreamError::InvalidRequest)
    );
    let text = log.to_text();
    for line in [
        "GET /basic -> 401",
        "GET /portal -> 403",
        "GET /headers -> 403",
        "GET /page -> sign-in page",
        "GET /missing -> 404",
        "GET /broken -> 500",
        "POST /basic -> invalid request",
    ] {
        assert!(text.contains(line), "{line} not in {text}");
    }
    assert!(!text.contains("Private Realm"), "{text}");
}

#[tokio::test]
async fn a_dropped_response_closes_the_upstream_connection() {
    let (closed_tx, closed_rx) = oneshot::channel::<()>();
    let closed_tx = Arc::new(Mutex::new(Some(closed_tx)));
    let stand_in = StandIn::start(move |_, socket| {
        let closed_tx = closed_tx.clone();
        async move {
            let (mut reader, mut writer) = socket.into_split();
            let written = AtomicU64::new(0);
            tokio::spawn(async move {
                let _ = writer.write_all(head(200, 100 * MIB, &[]).as_bytes()).await;
                let mut offset = 0;
                // Writes until the client goes away.
                while writer
                    .write_all(&pattern_bytes(offset, 64 * 1024))
                    .await
                    .is_ok()
                {
                    offset += 64 * 1024;
                    written.store(offset as u64, Ordering::SeqCst);
                }
            });
            // The client sends nothing more, so a read ends only when it closes the connection.
            let mut buffer = [0u8; 64];
            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            if let Some(tx) = closed_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let mut stream = ok(client.send_stream(Request::get("/Videos/1/stream")).await);
    assert!(ok(stream.next_chunk().await).is_some());
    drop(stream);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), closed_rx)
            .await
            .map(|r| r.is_ok()),
        Ok(true),
        "the upstream connection stayed open"
    );
}

#[tokio::test]
async fn a_read_timeout_fires_on_a_silent_server() {
    let stand_in = StandIn::start(|head, mut socket| async move {
        if head.starts_with("GET /silent-head ") {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        let _ = socket.write_all(head_with_body().as_bytes()).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;
    fn head_with_body() -> String {
        head(200, 1000, &["content-type: video/mp4"]) + "0123456789"
    }
    let log = Arc::new(RequestLog::default());
    let client = plain_client(&stand_in, &log);
    let request = Request::get("/Videos/1/stream").read_timeout(Duration::from_millis(300));
    let mut stream = ok(client.send_stream(request).await);
    assert_eq!(
        ok(stream.next_chunk().await).as_deref(),
        Some(&b"0123456789"[..])
    );
    let waited = tokio::time::Instant::now();
    assert_eq!(stream.next_chunk().await, Err(StreamError::Timeout));
    assert!(
        waited.elapsed() >= Duration::from_millis(250),
        "{:?}",
        waited.elapsed()
    );
    // The error stays.
    assert_eq!(stream.next_chunk().await, Err(StreamError::Timeout));
    let text = log.to_text();
    assert_eq!(
        text.matches("GET /Videos/1/stream -> timeout").count(),
        1,
        "{text}"
    );

    // No headers within the request's time is a timeout too.
    let waited = tokio::time::Instant::now();
    let request = Request::get("/silent-head").timeout(Duration::from_millis(300));
    assert_eq!(
        client.send_stream(request).await.err(),
        Some(StreamError::Timeout)
    );
    assert!(
        waited.elapsed() < Duration::from_secs(5),
        "{:?}",
        waited.elapsed()
    );
    assert!(log.to_text().contains("GET /silent-head -> timeout"));
}

#[tokio::test]
async fn no_log_line_or_debug_output_holds_a_credential_or_header_value() {
    let stand_in = StandIn::start(|head, socket| async move {
        let cookie = format!("set-cookie: {COOKIE_NAME}={ROTATED_VALUE}; Path=/");
        if head.starts_with("GET /refused?") {
            answer(
                socket,
                401,
                &["www-authenticate: Basic realm=\"r\"", &cookie],
                "",
            )
            .await;
        } else {
            answer(
                socket,
                200,
                &[
                    "content-type: video/mp4",
                    &format!("etag: {ETAG_VALUE}"),
                    &cookie,
                ],
                "body",
            )
            .await;
        }
    })
    .await;
    let gateway = Arc::new(Gateway::new(everything()));
    let log = Arc::new(RequestLog::default());
    let client = gateway_client(&stand_in, &gateway, &log);
    let request = |path: &str| {
        Request::get(path)
            .query("api_key", TOKEN)
            .header(header::RANGE, HeaderValue::from_static(RANGE_VALUE))
            .header(header::IF_RANGE, HeaderValue::from_static(ETAG_VALUE))
    };
    let mut shown = String::new();
    let mut stream = ok(client.send_stream(request("/ok")).await);
    shown.push_str(&format!("{stream:?}"));
    while ok(stream.next_chunk().await).is_some() {}
    let error = client.send_stream(request("/refused")).await.err();
    assert!(
        matches!(error, Some(StreamError::Refused { status: 401, .. })),
        "{error:?}"
    );
    shown.push_str(&format!("{error:?}"));
    if let Some(error) = &error {
        shown.push_str(&error.to_string());
    }
    shown.push_str(&format!("{:?}", request("/ok")));
    let text = log.to_text();
    assert!(
        text.contains("GET /refused?api_key=[redacted] -> 401"),
        "{text}"
    );
    // The success left no line.
    assert!(!text.contains("/ok"), "{text}");
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} in {text}");
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}
