//! The relay against a fake upstream, driven by a raw HTTP/1.1 client on 127.0.0.1 so that every byte the player would send is ours
//! to choose (ADR 0020, task 2).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{GatewayShape, StreamError, Url};
use current_relay::{
    Bytes, Relay, RelayEvent, Route, RouteConfig, StreamMethod, Upstream, UpstreamBody,
    UpstreamFuture, UpstreamRequest, UpstreamResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

const BASE: &str = "https://media.example.com/jf/";
const WAIT: Duration = Duration::from_secs(10);

type Answer = dyn Fn(&UpstreamRequest) -> Result<UpstreamResponse, StreamError> + Send + Sync;

/// An upstream that records what it was sent and answers with a closure.
struct Fake {
    sent: Mutex<Vec<UpstreamRequest>>,
    answer: Box<Answer>,
}

impl Fake {
    fn new(
        answer: impl Fn(&UpstreamRequest) -> Result<UpstreamResponse, StreamError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        })
    }

    fn sent(&self) -> Vec<UpstreamRequest> {
        self.sent.lock().unwrap().clone()
    }
}

impl Upstream for Fake {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
        let answer = (self.answer)(&request);
        self.sent.lock().unwrap().push(request);
        Box::pin(async move { answer })
    }
}

/// A body handed out in chunks of `size` bytes.
struct Chunks {
    data: Bytes,
    size: usize,
}

impl UpstreamBody for Chunks {
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>> {
        let take = self.size.min(self.data.len());
        let chunk = (take > 0).then(|| self.data.split_to(take));
        Box::pin(async move {
            // Let other connections run between chunks, as a network read would.
            tokio::task::yield_now().await;
            Ok(chunk)
        })
    }
}

/// A body that never ends, counting the chunks taken and noting when it is dropped.
struct Endless {
    pulled: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}

const ENDLESS_CHUNK: usize = 64 * 1024;

impl UpstreamBody for Endless {
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>> {
        self.pulled.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::task::yield_now().await;
            Ok(Some(Bytes::from(vec![b'x'; ENDLESS_CHUNK])))
        })
    }
}

impl Drop for Endless {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
struct Probe {
    pulled: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}

impl Probe {
    fn body(&self) -> Box<dyn UpstreamBody> {
        Box::new(Endless {
            pulled: self.pulled.clone(),
            dropped: self.dropped.clone(),
        })
    }
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    map
}

fn answer(status: u16, pairs: &[(&'static str, &str)], body: impl Into<Bytes>) -> UpstreamResponse {
    UpstreamResponse {
        status,
        headers: headers(pairs),
        base: Url::parse(BASE).unwrap(),
        body: Box::new(Chunks {
            data: body.into(),
            size: 7 * 1024,
        }),
    }
}

fn ok_media() -> Arc<Fake> {
    Fake::new(|_| Ok(answer(200, &[("content-type", "video/mp4")], "media")))
}

struct Setup {
    relay: Relay,
    route: Route,
    events: mpsc::UnboundedReceiver<RelayEvent>,
}

fn route_on(relay: &Relay, upstream: Arc<Fake>) -> (Route, mpsc::UnboundedReceiver<RelayEvent>) {
    let (sink, events) = mpsc::unbounded_channel();
    let route = relay
        .add_route(RouteConfig {
            upstream,
            prefixes: vec!["/Videos/abc/".into(), "/Audio/abc/".into()],
            events: Arc::new(sink),
        })
        .unwrap();
    (route, events)
}

fn setup(upstream: Arc<Fake>) -> Setup {
    let relay = Relay::new(&tokio::runtime::Handle::current());
    let (route, events) = route_on(&relay, upstream);
    Setup {
        relay,
        route,
        events,
    }
}

fn drain(events: &mut mpsc::UnboundedReceiver<RelayEvent>) -> Vec<RelayEvent> {
    let mut all = Vec::new();
    while let Ok(event) = events.try_recv() {
        all.push(event);
    }
    all
}

/// A parsed answer.
#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .headers
            .iter()
            .map(|(key, _)| key.to_ascii_lowercase())
            .collect();
        names.sort();
        names
    }
}

/// Sends `raw` on a new connection and reads until the relay closes it.
async fn exchange(port: u16, raw: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(raw).await.unwrap();
    let mut bytes = Vec::new();
    timeout(WAIT, stream.read_to_end(&mut bytes))
        .await
        .expect("the relay answers and closes")
        .unwrap();
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Reply {
    let end = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a complete head");
    let head = std::str::from_utf8(&bytes[..end]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_owned(), value.trim().to_owned())
        })
        .collect();
    let mut body = bytes[end + 4..].to_vec();
    let chunked = headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("transfer-encoding") && value == "chunked");
    if chunked {
        body = dechunk(&body);
    }
    Reply {
        status,
        headers,
        body,
    }
}

fn dechunk(mut bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line = bytes.windows(2).position(|w| w == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&bytes[..line]).unwrap(), 16).unwrap();
        bytes = &bytes[line + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
}

fn request(method: &str, target: &str, port: u16, extra: &str) -> Vec<u8> {
    format!(
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n{extra}\r\n"
    )
    .into_bytes()
}

async fn get(setup: &Setup, path: &str, extra: &str) -> Reply {
    let port = setup.route.port();
    exchange(
        port,
        &request("GET", &path_on(&setup.route, path), port, extra),
    )
    .await
}

/// `path` on the route: `/<token><path>`.
fn path_on(route: &Route, path: &str) -> String {
    format!("/{}{path}", route.token())
}

#[tokio::test]
async fn every_refusal_comes_before_anything_goes_upstream() {
    let upstream = ok_media();
    let setup = setup(upstream.clone());
    let port = setup.route.port();
    let good = path_on(&setup.route, "/Videos/abc/stream");
    let other_token = "B".repeat(setup.route.token().len());
    let cases: Vec<(&str, Vec<u8>, &[u16])> = vec![
        (
            "wrong token",
            request("GET", &format!("/{other_token}/Videos/abc/stream"), port, ""),
            &[404],
        ),
        ("no token", request("GET", "/Videos/abc/stream", port, ""), &[404]),
        (
            "a token's prefix",
            request("GET", &format!("/{}/Videos/abc/stream", &setup.route.token()[..21]), port, ""),
            &[404],
        ),
        ("outside the prefixes", request("GET", &path_on(&setup.route, "/Users/abc"), port, ""), &[404]),
        ("another item", request("GET", &path_on(&setup.route, "/Videos/abd/stream"), port, ""), &[404]),
        (
            "climbing out",
            request("GET", &path_on(&setup.route, "/Videos/abc/../../Users"), port, ""),
            &[404],
        ),
        (
            "climbing out, encoded",
            request("GET", &path_on(&setup.route, "/Videos/abc/%2e%2e/%2E%2E/Users"), port, ""),
            &[404],
        ),
        (
            "a foreign host",
            format!("GET {good} HTTP/1.1\r\nHost: media.example.com\r\nConnection: close\r\n\r\n").into_bytes(),
            &[400],
        ),
        (
            "localhost by name",
            format!("GET {good} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").into_bytes(),
            &[400],
        ),
        (
            "another port",
            format!("GET {good} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port.wrapping_add(1))
                .into_bytes(),
            &[400],
        ),
        (
            "two hosts",
            format!("GET {good} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .into_bytes(),
            &[400],
        ),
        (
            "no host",
            format!("GET {good} HTTP/1.0\r\n\r\n").into_bytes(),
            &[400],
        ),
        ("an origin", request("GET", &good, port, "Origin: http://media.example.com\r\n"), &[403]),
        ("a null origin", request("GET", &good, port, "Origin: null\r\n"), &[403]),
        ("POST", request("POST", &good, port, "Content-Length: 0\r\n"), &[405]),
        ("PUT", request("PUT", &good, port, ""), &[405]),
        ("DELETE", request("DELETE", &good, port, ""), &[405]),
        ("OPTIONS", request("OPTIONS", &good, port, ""), &[405]),
        ("OPTIONS *", request("OPTIONS", "*", port, ""), &[400, 405]),
        (
            "absolute form",
            request("GET", &format!("http://127.0.0.1:{port}{good}"), port, ""),
            &[400],
        ),
        (
            "absolute form elsewhere",
            request("GET", &format!("http://192.168.1.10{good}"), port, ""),
            &[400],
        ),
        ("CONNECT", request("CONNECT", "192.168.1.10:443", port, ""), &[400, 405]),
        ("a body", request("GET", &good, port, "Content-Length: 5\r\n\r\nhello"), &[400]),
        (
            "an encoded slash",
            request("GET", &path_on(&setup.route, "/Videos/abc%2F..%2F..%2FUsers"), port, ""),
            &[404],
        ),
        ("an encoded NUL", request("GET", &path_on(&setup.route, "/Videos/abc/a%00b"), port, ""), &[404]),
        ("a parameter climbing out", request("GET", &path_on(&setup.route, "/Videos/abc/..;/..;/Users"), port, ""), &[404]),
        (
            "a host without the port",
            format!("GET {good} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").into_bytes(),
            &[400],
        ),
        ("a query that is not UTF-8", request("GET", &format!("{good}?x=%ff"), port, ""), &[400]),
        ("a broken escape", request("GET", &format!("{good}?x=%zz"), port, ""), &[400]),
        (
            "a length and a chunked body",
            request(
                "GET",
                &good,
                port,
                "Content-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n",
            ),
            &[400],
        ),
        (
            "a chunked body",
            request("GET", &good, port, "Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n"),
            &[400],
        ),
    ];
    for (name, raw, statuses) in cases {
        let reply = exchange(port, &raw).await;
        assert!(statuses.contains(&reply.status), "{name}: {}", reply.status);
        assert!(upstream.sent().is_empty(), "{name} went upstream");
    }
    // The same request without the fault goes through.
    let reply = exchange(port, &request("GET", &good, port, "")).await;
    assert_eq!((reply.status, reply.body.as_slice()), (200, &b"media"[..]));
    assert_eq!(upstream.sent().len(), 1);
}

#[tokio::test]
async fn only_the_listed_headers_cross_and_api_key_never_goes_upstream() {
    let upstream = Fake::new(|_| {
        Ok(answer(
            206,
            &[
                ("content-type", "video/mp4"),
                ("content-length", "4"),
                ("content-range", "bytes 2-5/10"),
                ("accept-ranges", "bytes"),
                ("etag", "\"e\""),
                ("last-modified", "Mon, 26 Oct 2026 10:00:00 GMT"),
                ("cache-control", "no-cache"),
                ("set-cookie", "session=rotated; Path=/"),
                ("www-authenticate", "Basic realm=\"x\""),
                ("location", "https://192.168.1.10/"),
                ("x-server", "anything"),
            ],
            "2345",
        ))
    });
    let setup = setup(upstream.clone());
    let reply = get(
        &setup,
        "/Videos/abc/stream?static=true&ApiKey=secret&api_key=secret&MediaSourceId=m",
        "Range: bytes=2-5\r\nIf-Range: \"e\"\r\nIf-None-Match: \"f\"\r\nIf-Modified-Since: Mon, 26 Oct 2026 10:00:00 GMT\r\n\
         Accept: */*\r\nCookie: session=player\r\nAuthorization: Basic eDp5\r\nX-Emby-Token: secret\r\n\
         User-Agent: a player\r\nReferer: http://192.168.1.10/\r\n",
    )
    .await;
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, b"2345");
    let mut names = reply.names();
    names.retain(|name| name != "date" && name != "connection");
    assert_eq!(
        names,
        [
            "accept-ranges",
            "cache-control",
            "content-length",
            "content-range",
            "content-type",
            "etag",
            "last-modified"
        ]
    );
    assert_eq!(reply.header("content-range"), Some("bytes 2-5/10"));
    let sent = upstream.sent();
    assert_eq!(sent.len(), 1);
    let sent = &sent[0];
    assert_eq!(sent.method, StreamMethod::Get);
    assert_eq!(sent.path, "/Videos/abc/stream");
    assert_eq!(
        sent.query,
        [
            ("static".to_owned(), "true".to_owned()),
            ("MediaSourceId".to_owned(), "m".to_owned())
        ]
    );
    let mut sent_names: Vec<&str> = sent.headers.keys().map(HeaderName::as_str).collect();
    sent_names.sort();
    assert_eq!(
        sent_names,
        [
            "accept",
            "if-modified-since",
            "if-none-match",
            "if-range",
            "range"
        ]
    );
    assert_eq!(sent.headers["range"], "bytes=2-5");
    // Debug shows names only and redacts the query.
    let debug = format!("{sent:?}");
    assert!(
        !debug.contains("bytes=2-5") && !debug.contains("\"m\""),
        "{debug}"
    );
}

#[tokio::test]
async fn a_head_gets_the_headers_without_a_body_and_304_passes() {
    let upstream = Fake::new(|request| {
        if request.headers.contains_key("if-none-match") {
            Ok(answer(304, &[("etag", "\"e\"")], ""))
        } else {
            Ok(answer(
                200,
                &[("content-type", "video/mp4"), ("content-length", "1000")],
                vec![0u8; 1000],
            ))
        }
    });
    let setup = setup(upstream.clone());
    let port = setup.route.port();
    let reply = exchange(
        port,
        &request(
            "HEAD",
            &path_on(&setup.route, "/Videos/abc/stream"),
            port,
            "",
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-length"), Some("1000"));
    assert!(reply.body.is_empty());
    assert_eq!(upstream.sent()[0].method, StreamMethod::Head);
    let reply = get(&setup, "/Videos/abc/stream", "If-None-Match: \"e\"\r\n").await;
    assert_eq!(reply.status, 304);
    assert_eq!(reply.header("etag"), Some("\"e\""));
}

#[tokio::test]
async fn refusals_and_failures_upstream_are_reported_once_per_route() {
    let upstream = Fake::new(|request| {
        let kind = request.path.rsplit('/').next().unwrap_or("");
        match kind {
            "basic" => Err(StreamError::Refused {
                status: 401,
                shape: GatewayShape::Password {
                    realm: Some("a gateway".into()),
                },
            }),
            "forbidden" => Err(StreamError::Refused {
                status: 403,
                shape: GatewayShape::Refused,
            }),
            "redirect" => Err(StreamError::Refused {
                status: 302,
                shape: GatewayShape::SignIn,
            }),
            "ok-redirect" => Ok(answer(302, &[("location", "https://192.168.1.10/")], "")),
            "proxy" => Err(StreamError::Status { status: 407 }),
            "unsatisfiable" => Ok(answer(
                416,
                &[("content-range", "bytes */10"), ("content-length", "0")],
                "",
            )),
            "page" => Ok(answer(
                200,
                &[("content-type", "text/html; charset=utf-8")],
                "<html>",
            )),
            "missing" => Err(StreamError::Status { status: 404 }),
            "past-the-end" => Err(StreamError::Status { status: 416 }),
            "broken" => Err(StreamError::Status { status: 500 }),
            "offline" => Err(StreamError::Unreachable),
            "unbuildable" => Err(StreamError::InvalidRequest),
            _ => Err(StreamError::Timeout),
        }
    });
    let mut setup = setup(upstream.clone());
    let expect = [
        (
            "basic",
            502,
            Some(RelayEvent::Refused {
                shape: GatewayShape::Password {
                    realm: Some("a gateway".into()),
                },
            }),
        ),
        ("basic", 502, None),
        ("forbidden", 502, None),
        ("redirect", 502, None),
        ("page", 502, None),
        ("missing", 404, Some(RelayEvent::NotFound)),
        ("missing", 404, None),
        ("past-the-end", 416, None),
        ("unsatisfiable", 416, None),
        ("broken", 500, Some(RelayEvent::Server { status: 500 })),
        // An upstream that answers a redirect as a response breaks its contract: nothing of it goes on.
        ("ok-redirect", 502, None),
        // Only 404, 416 and 5xx reach the player as they are.
        ("proxy", 502, None),
        ("offline", 502, Some(RelayEvent::Network)),
        ("slow", 504, None),
        ("unbuildable", 502, Some(RelayEvent::InvalidRequest)),
    ];
    for (kind, status, event) in expect {
        let reply = get(&setup, &format!("/Videos/abc/{kind}"), "").await;
        assert_eq!(reply.status, status, "{kind}");
        assert!(reply.header("location").is_none(), "{kind}");
        assert_eq!(drain(&mut setup.events), Vec::from_iter(event), "{kind}");
    }
    let reply = get(&setup, "/Videos/abc/unsatisfiable", "Range: bytes=20-\r\n").await;
    assert_eq!(reply.header("content-range"), Some("bytes */10"));
    // Another route has its own once.
    let (other, mut events) = route_on(&setup.relay, upstream);
    let port = other.port();
    let reply = exchange(
        port,
        &request("GET", &path_on(&other, "/Videos/abc/page"), port, ""),
    )
    .await;
    assert_eq!(reply.status, 502);
    assert_eq!(
        drain(&mut events),
        [RelayEvent::Refused {
            shape: GatewayShape::SignIn
        }]
    );
}

const MASTER: &str = "#EXTM3U\n\
    #EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"https://media.example.com/jf/videos/abc/session.key?ApiKey=secret\"\n\
    #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A\",URI=\"audio.m3u8?ApiKey=secret&PlaySessionId=p\"\n\
    #EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"a\"\n\
    main.m3u8?DeviceId=d&api_key=secret\n\
    #EXT-X-FUTURE-TAG:X=1,THING-URI=\"/jf/videos/abc/thing.json\"\n";

const MEDIA: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n\
    #EXT-X-KEY:METHOD=AES-128,URI=\"keys/k.key?ApiKey=secret\",IV=0x1\n\
    #EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n\
    #EXTINF:6.0,\n#EXT-X-BYTERANGE:1000@720\nhttps://media.example.com/jf/videos/abc/0.mp4?ApiKey=secret\n\
    #EXTINF:6.0,\nhls1/1.mp4\n#EXT-X-ENDLIST\n";

#[tokio::test]
async fn playlists_are_rewritten_onto_the_route_without_api_key() {
    let upstream = Fake::new(|request| {
        let body = match request.path.as_str() {
            "/videos/abc/master.m3u8" => MASTER,
            "/videos/abc/main.m3u8" => MEDIA,
            "/videos/abc/elsewhere.m3u8" => {
                "#EXTM3U\n#EXTINF:6,\nhttps://192.168.1.10/jf/videos/abc/0.mp4\n"
            }
            "/videos/abc/elsewhere-key.m3u8" => {
                "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://media.example.com/jf/videos/abc/k\"\n"
            }
            "/videos/abc/other-item.m3u8" => "#EXTM3U\n#EXTINF:6,\n../xyz/0.mp4\n",
            "/videos/abc/not-a-playlist.m3u8" => "<html></html>",
            // By type, without a .m3u8 path.
            "/videos/abc/typed" => "#EXTM3U\n0.ts?ApiKey=secret\n",
            _ => "segment",
        };
        let kind = if request.path.ends_with("not-a-playlist.m3u8") {
            "text/plain"
        } else if request.path.contains("m3u8") || request.path.ends_with("typed") {
            "application/vnd.apple.mpegurl"
        } else {
            "video/mp4"
        };
        Ok(answer(
            200,
            &[
                ("content-type", kind),
                ("content-length", "999"),
                ("etag", "\"e\""),
            ],
            body,
        ))
    });
    let mut setup = setup(upstream.clone());
    let token = setup.route.token().to_owned();
    let master = get(&setup, "/videos/abc/master.m3u8?DeviceId=d", "").await;
    assert_eq!(master.status, 200);
    let text = String::from_utf8(master.body.clone()).unwrap();
    assert_eq!(
        text,
        format!(
            "#EXTM3U\n\
            #EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"/{token}/videos/abc/session.key\"\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A\",URI=\"/{token}/videos/abc/audio.m3u8?PlaySessionId=p\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"a\"\n\
            /{token}/videos/abc/main.m3u8?DeviceId=d\n\
            #EXT-X-FUTURE-TAG:X=1,THING-URI=\"/{token}/videos/abc/thing.json\"\n"
        )
    );
    assert_eq!(
        master.header("content-length"),
        Some(text.len().to_string().as_str())
    );
    assert!(master.header("etag").is_none());
    assert_eq!(
        master.header("content-type"),
        Some("application/vnd.apple.mpegurl")
    );

    // The player follows the rewritten URI back through the relay.
    let media = get(&setup, "/videos/abc/main.m3u8?DeviceId=d", "").await;
    let text = String::from_utf8(media.body).unwrap();
    assert_eq!(
        text,
        format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"/{token}/videos/abc/keys/k.key\",IV=0x1\n\
            #EXT-X-MAP:URI=\"/{token}/videos/abc/init.mp4\",BYTERANGE=\"720@0\"\n\
            #EXTINF:6.0,\n#EXT-X-BYTERANGE:1000@720\n/{token}/videos/abc/0.mp4\n\
            #EXTINF:6.0,\n/{token}/videos/abc/hls1/1.mp4\n#EXT-X-ENDLIST\n"
        )
    );
    assert!(!text.to_ascii_lowercase().contains("api"));
    let segment = get(&setup, "/videos/abc/0.mp4", "Range: bytes=720-1719\r\n").await;
    assert_eq!(segment.body, b"segment");
    let typed = get(&setup, "/videos/abc/typed", "").await;
    assert_eq!(
        String::from_utf8(typed.body).unwrap(),
        format!("#EXTM3U\n/{token}/videos/abc/0.ts\n")
    );
    assert!(drain(&mut setup.events).is_empty());
    // A HEAD of a playlist has no length: the rewritten one is not known without the body.
    let port = setup.route.port();
    let head = exchange(
        port,
        &request(
            "HEAD",
            &path_on(&setup.route, "/videos/abc/master.m3u8"),
            port,
            "",
        ),
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-length"), None);
    assert_eq!(head.header("etag"), None);

    for path in [
        "/videos/abc/elsewhere.m3u8",
        "/videos/abc/elsewhere-key.m3u8",
        "/videos/abc/other-item.m3u8",
    ] {
        let reply = get(&setup, path, "").await;
        assert_eq!(reply.status, 502, "{path}");
        assert!(reply.body.is_empty(), "{path}");
    }
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistElsewhere]);
    let reply = get(&setup, "/videos/abc/not-a-playlist.m3u8", "").await;
    assert_eq!(reply.status, 502);
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistUnreadable]);
}

#[tokio::test]
async fn a_playlist_larger_than_the_limit_fails() {
    let upstream = Fake::new(|_| {
        let mut text = String::from("#EXTM3U\n");
        while text.len() <= current_relay::MAX_PLAYLIST_BYTES {
            text.push_str("#EXTINF:6.0,\n0.ts\n");
        }
        Ok(answer(
            200,
            &[("content-type", "application/x-mpegURL")],
            text,
        ))
    });
    let mut setup = setup(upstream);
    let reply = get(&setup, "/Videos/abc/main.m3u8", "").await;
    assert_eq!(reply.status, 502);
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistUnreadable]);
}

/// A file of `len` bytes where every byte depends on its offset.
fn file(len: usize) -> Bytes {
    Bytes::from(
        (0..len)
            .map(|i| (i % 251) as u8 ^ (i / 251) as u8)
            .collect::<Vec<u8>>(),
    )
}

#[tokio::test]
async fn sixteen_concurrent_range_requests_are_byte_exact() {
    const LEN: usize = 4 * 1024 * 1024;
    let data = file(LEN);
    let served = data.clone();
    let upstream = Fake::new(move |request| {
        let range = request.headers["range"].to_str().unwrap();
        let (start, end) = range
            .strip_prefix("bytes=")
            .and_then(|range| range.split_once('-'))
            .unwrap();
        let (start, end): (usize, usize) = (start.parse().unwrap(), end.parse().unwrap());
        let slice = served.slice(start..=end);
        let length = slice.len().to_string();
        let content_range = format!("bytes {start}-{end}/{LEN}");
        Ok(answer(
            206,
            &[
                ("content-type", "video/mp4"),
                ("content-length", &length),
                ("content-range", &content_range),
            ],
            slice,
        ))
    });
    let setup = setup(upstream.clone());
    let port = setup.route.port();
    let path = path_on(&setup.route, "/Videos/abc/stream?static=true");
    let mut tasks = Vec::new();
    for i in 0..16usize {
        let start = i * 250_000 + i * 7;
        let end = start + 200_000 + i * 1_013;
        let raw = request(
            "GET",
            &path,
            port,
            &format!("Range: bytes={start}-{end}\r\n"),
        );
        tasks.push(tokio::spawn(async move {
            (start, end, exchange(port, &raw).await)
        }));
    }
    for task in tasks {
        let (start, end, reply) = task.await.unwrap();
        assert_eq!(reply.status, 206);
        assert_eq!(
            reply.header("content-range"),
            Some(format!("bytes {start}-{end}/{LEN}").as_str())
        );
        assert!(
            reply.body == data[start..=end],
            "range {start}-{end} differs"
        );
    }
    assert_eq!(upstream.sent().len(), 16);
}

/// Waits until `pulled` stops growing and returns it.
async fn settled(pulled: &AtomicUsize) -> usize {
    let mut last = usize::MAX;
    for _ in 0..50 {
        sleep(Duration::from_millis(200)).await;
        let now = pulled.load(Ordering::SeqCst);
        if now == last {
            return now;
        }
        last = now;
    }
    panic!("the relay kept reading upstream for a client that does not read");
}

#[tokio::test]
async fn a_slow_reader_stops_the_reads_upstream() {
    let probe = Probe::default();
    let body = probe.clone();
    let upstream = Fake::new(move |_| {
        Ok(UpstreamResponse {
            status: 200,
            headers: headers(&[("content-type", "video/mp4")]),
            base: Url::parse(BASE).unwrap(),
            body: body.body(),
        })
    });
    let setup = setup(upstream);
    let port = setup.route.port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(&request(
            "GET",
            &path_on(&setup.route, "/Videos/abc/stream"),
            port,
            "",
        ))
        .await
        .unwrap();
    let mut buffer = vec![0u8; 16 * 1024];
    timeout(WAIT, stream.read_exact(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    // The client stops reading: what the relay took is what the sockets' buffers hold, and then it stops.
    let pulled = settled(&probe.pulled).await;
    let bytes = pulled * ENDLESS_CHUNK;
    assert!(
        bytes < 48 * 1024 * 1024,
        "{bytes} bytes read for a client that stopped"
    );
    // Reading again resumes it.
    let mut more = vec![0u8; 8 * 1024 * 1024];
    timeout(WAIT, stream.read_exact(&mut more))
        .await
        .unwrap()
        .unwrap();
    assert!(probe.pulled.load(Ordering::SeqCst) > pulled);
    assert!(!probe.dropped.load(Ordering::SeqCst));
    drop(stream);
    timeout(WAIT, async {
        while !probe.dropped.load(Ordering::SeqCst) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a closed client drops the upstream body");
}

fn endless(probe: &Probe) -> Arc<Fake> {
    let probe = probe.clone();
    Fake::new(move |_| {
        Ok(UpstreamResponse {
            status: 200,
            headers: headers(&[("content-type", "video/mp4")]),
            base: Url::parse(BASE).unwrap(),
            body: probe.body(),
        })
    })
}

/// Opens a stream on `route` and reads its first bytes.
async fn open_stream(route: &Route) -> TcpStream {
    let port = route.port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(&request(
            "GET",
            &path_on(route, "/Videos/abc/stream"),
            port,
            "",
        ))
        .await
        .unwrap();
    let mut buffer = vec![0u8; 1024];
    timeout(WAIT, stream.read_exact(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    stream
}

/// Reads until the relay closes the connection.
async fn closed(stream: &mut TcpStream) {
    let mut sink = vec![0u8; 1024 * 1024];
    timeout(WAIT, async {
        loop {
            match stream.read(&mut sink).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await
    .expect("the relay closes the connection");
}

#[tokio::test]
async fn removing_a_route_aborts_its_connections_only() {
    let relay = Relay::new(&tokio::runtime::Handle::current());
    let (first_probe, second_probe) = (Probe::default(), Probe::default());
    let (first, _first_events) = route_on(&relay, endless(&first_probe));
    let (second, _second_events) = route_on(&relay, endless(&second_probe));
    assert_eq!(first.port(), second.port());
    let mut first_stream = open_stream(&first).await;
    let mut second_stream = open_stream(&second).await;
    first.remove();
    closed(&mut first_stream).await;
    timeout(WAIT, async {
        while !first_probe.dropped.load(Ordering::SeqCst) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the removed route's upstream body is dropped");
    // The other route still streams.
    let mut buffer = vec![0u8; 1024 * 1024];
    timeout(WAIT, second_stream.read_exact(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert!(!second_probe.dropped.load(Ordering::SeqCst));
    // A connection serves one route: on a kept-alive connection, another route's token is refused.
    let (third, _third_events) = route_on(&relay, ok_media());
    let port = second.port();
    let mut raw = format!(
        "HEAD {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n",
        path_on(&second, "/Videos/abc/stream")
    )
    .into_bytes();
    raw.extend(request("HEAD", &path_on(&third, "/Videos/abc/x"), port, ""));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(&raw).await.unwrap();
    let mut bytes = Vec::new();
    timeout(WAIT, stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    let statuses: Vec<&str> = text
        .split("\r\n")
        .filter(|line| line.starts_with("HTTP/1.1 "))
        .collect();
    // The second request is refused and the connection closed; whether the 404 gets out first is a race the test does not need.
    assert!(
        statuses == ["HTTP/1.1 200 OK"]
            || statuses == ["HTTP/1.1 200 OK", "HTTP/1.1 404 Not Found"],
        "{statuses:?}"
    );
    // A fresh connection serves the third route.
    let reply = exchange(
        port,
        &request("GET", &path_on(&third, "/Videos/abc/x"), port, ""),
    )
    .await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn the_listener_stops_with_the_last_route_and_starts_again_on_a_new_port() {
    let relay = Relay::new(&tokio::runtime::Handle::current());
    assert_eq!(relay.port(), None);
    let (first, _events) = route_on(&relay, ok_media());
    let (second, _more) = route_on(&relay, ok_media());
    let port = first.port();
    assert_eq!(relay.port(), Some(port));
    let mut idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    drop(first);
    let reply = exchange(
        port,
        &request("GET", &path_on(&second, "/Videos/abc/x"), port, ""),
    )
    .await;
    assert_eq!(reply.status, 200);
    drop(second);
    assert_eq!(relay.port(), None);
    // Connections without a request yet are closed too, and the port takes no more.
    closed(&mut idle).await;
    timeout(WAIT, async {
        while TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("nothing listens once the last route is gone");
    let (third, _events) = route_on(&relay, ok_media());
    let port = third.port();
    assert_eq!(relay.port(), Some(port));
    let reply = exchange(
        port,
        &request("GET", &path_on(&third, "/Videos/abc/x"), port, ""),
    )
    .await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn a_seventeenth_connection_waits_for_a_free_one() {
    let probe = Probe::default();
    let setup = setup(endless(&probe));
    let mut streams = Vec::new();
    for _ in 0..current_relay::MAX_CONNECTIONS {
        streams.push(open_stream(&setup.route).await);
    }
    let port = setup.route.port();
    let raw = request("GET", &path_on(&setup.route, "/Audio/abc/x"), port, "");
    let waiting = tokio::spawn(async move {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(&raw).await.unwrap();
        let mut first = [0u8; 12];
        stream.read_exact(&mut first).await.unwrap();
        first
    });
    sleep(Duration::from_millis(500)).await;
    assert!(
        !waiting.is_finished(),
        "a seventeenth connection was served"
    );
    drop(streams.pop());
    let first = timeout(WAIT, waiting).await.unwrap().unwrap();
    assert_eq!(&first, b"HTTP/1.1 200");
}

#[tokio::test]
async fn debug_output_shows_no_token() {
    let setup = setup(ok_media());
    let token = setup.route.token().to_owned();
    let url = setup.route.url("/Videos/abc/stream?static=true");
    assert_eq!(
        url,
        format!(
            "http://127.0.0.1:{}/{token}/Videos/abc/stream?static=true",
            setup.route.port()
        )
    );
    for debug in [format!("{:?}", setup.route), format!("{:?}", setup.relay)] {
        assert!(!debug.contains(&token), "{debug}");
    }
}

#[tokio::test]
async fn bad_prefixes_are_refused() {
    let relay = Relay::new(&tokio::runtime::Handle::current());
    let (sink, _events) = mpsc::unbounded_channel();
    let error = relay
        .add_route(RouteConfig {
            upstream: ok_media(),
            prefixes: vec!["/".into()],
            events: Arc::new(sink),
        })
        .unwrap_err();
    assert_eq!(error, current_relay::RelayError::InvalidPrefix);
    assert_eq!(relay.port(), None);
}

#[tokio::test]
async fn a_tag_the_player_reads_differently_fails_the_playlist() {
    // Each was read by FFmpeg 6.1.1 as naming the off-origin key (manager's review, 2026-10-26).
    for line in [
        "#EXT-X-KEY:METHOD=AES-128,IV=0x1 URI=\"http://192.168.1.10/k\"",
        "#EXT-X-KEY:METHOD=AES-128,X=a\"b,URI=\"http://192.168.1.10/k\"",
        "#EXT-X-KEY:METHOD=AES-128,X=\"a\\\",Y=\",URI=\"http://192.168.1.10/k\"",
        "#EXT-X-MAP:URI=\"init.mp4\" ,BYTERANGE=\"720@0\"",
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A\",AUTOSELECT=YES URI=\"http://192.168.1.10/a.m3u8\"",
        "#EXTINF:6,\rhttp://192.168.1.10/0.ts",
    ] {
        let text = format!("#EXTM3U\n{line}\n#EXTINF:6,\n0.ts\n");
        let upstream = Fake::new(move |_| {
            Ok(answer(
                200,
                &[("content-type", "application/vnd.apple.mpegurl")],
                text.clone(),
            ))
        });
        let mut setup = setup(upstream);
        let reply = get(&setup, "/videos/abc/main.m3u8", "").await;
        assert_eq!(reply.status, 502, "{line:?}");
        assert_eq!(
            drain(&mut setup.events),
            [RelayEvent::PlaylistUnreadable],
            "{line:?}"
        );
    }
}

#[tokio::test]
async fn a_body_that_starts_like_a_playlist_is_rewritten_whatever_its_type() {
    let upstream = Fake::new(|request| {
        let (kind, body) = match request.path.as_str() {
            "/videos/abc/stream" => ("video/mp4", "#EXTM3U\n0.ts?ApiKey=secret\n"),
            "/videos/abc/untyped" => ("", "  \r\n#EXTM3U\n0.ts\n"),
            "/videos/abc/x-type" => ("audio/x-mpegurl", "#EXTM3U\n0.ts\n"),
            "/videos/abc/list.m3u" => ("text/plain", "#EXTM3U\n0.ts\n"),
            "/videos/abc/hostile" => (
                "video/mp2t",
                "#EXTM3U\n#EXTINF:6,\nhttp://192.168.1.10/0.ts\n",
            ),
            "/videos/abc/bom" => ("video/mp4", "\u{feff}#EXTM3U\nhttp://192.168.1.10/0.ts\n"),
            _ => ("video/mp4", "media, not a playlist"),
        };
        let mut response = answer(200, &[], body);
        if !kind.is_empty() {
            response.headers = headers(&[("content-type", kind)]);
        }
        // Three bytes at a time, so the magic arrives over several chunks.
        response.body = Box::new(Chunks {
            data: Bytes::from(body),
            size: 3,
        });
        Ok(response)
    });
    let mut setup = setup(upstream);
    let token = setup.route.token().to_owned();
    for (path, expected) in [
        (
            "/videos/abc/stream",
            format!("#EXTM3U\n/{token}/videos/abc/0.ts\n"),
        ),
        (
            "/videos/abc/untyped",
            format!("  \r\n#EXTM3U\n/{token}/videos/abc/0.ts\n"),
        ),
        (
            "/videos/abc/x-type",
            format!("#EXTM3U\n/{token}/videos/abc/0.ts\n"),
        ),
        (
            "/videos/abc/list.m3u",
            format!("#EXTM3U\n/{token}/videos/abc/0.ts\n"),
        ),
        ("/videos/abc/media.mp4", "media, not a playlist".to_owned()),
    ] {
        let reply = get(&setup, path, "").await;
        if path == "/videos/abc/untyped" {
            // Blank lines before the magic: not a playlist this parser reads, so refused rather than passed on.
            assert_eq!(reply.status, 502, "{path}");
            continue;
        }
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(String::from_utf8(reply.body).unwrap(), expected, "{path}");
    }
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistUnreadable]);
    let reply = get(&setup, "/videos/abc/hostile", "").await;
    assert_eq!(reply.status, 502);
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistElsewhere]);
    let reply = get(&setup, "/videos/abc/bom", "").await;
    assert_eq!(reply.status, 502);
    assert!(reply.body.is_empty());
}

#[tokio::test]
async fn a_playlist_in_a_206_is_passed_on_whole_or_not_at_all() {
    const TEXT: &str = "#EXTM3U\n0.ts\n";
    let upstream = Fake::new(|request| {
        let range = request.headers["range"].to_str().unwrap().to_owned();
        let total = TEXT.len();
        let (content_range, body) = if range == "bytes=0-" {
            (format!("bytes 0-{}/{total}", total - 1), TEXT)
        } else {
            (format!("bytes 3-{}/{total}", total - 1), &TEXT[3..])
        };
        Ok(answer(
            206,
            &[
                ("content-type", "application/vnd.apple.mpegurl"),
                ("content-range", &content_range),
                ("accept-ranges", "bytes"),
            ],
            body,
        ))
    });
    let mut setup = setup(upstream);
    let token = setup.route.token().to_owned();
    let reply = get(&setup, "/videos/abc/main.m3u8", "Range: bytes=0-\r\n").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-range"), None);
    assert_eq!(reply.header("accept-ranges"), None);
    let text = format!("#EXTM3U\n/{token}/videos/abc/0.ts\n");
    assert_eq!(
        reply.header("content-length"),
        Some(text.len().to_string().as_str())
    );
    assert_eq!(String::from_utf8(reply.body).unwrap(), text);
    let reply = get(&setup, "/videos/abc/main.m3u8", "Range: bytes=3-\r\n").await;
    assert_eq!(reply.status, 502);
    assert_eq!(drain(&mut setup.events), [RelayEvent::PlaylistUnreadable]);
}

#[tokio::test]
async fn a_refusal_closes_the_connection() {
    let setup = setup(ok_media());
    let port = setup.route.port();
    let other_token = "B".repeat(setup.route.token().len());
    // No `Connection: close` from the client: the relay closes.
    let raw =
        format!("GET /{other_token}/Videos/abc/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let reply = exchange(port, raw.as_bytes()).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.header("connection"), Some("close"));
}

#[tokio::test]
async fn idle_connections_give_their_slots_back() {
    let setup = setup(ok_media());
    let port = setup.route.port();
    let mut idle = Vec::new();
    for _ in 0..current_relay::MAX_CONNECTIONS {
        idle.push(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    }
    // Every slot is held by a connection that never asks; after the first-request timeout they are closed and a player gets in.
    let started = tokio::time::Instant::now();
    let reply = timeout(
        current_relay::FIRST_REQUEST_TIMEOUT + WAIT,
        exchange(
            port,
            &request("GET", &path_on(&setup.route, "/Videos/abc/x"), port, ""),
        ),
    )
    .await
    .expect("a player is served once idle connections time out");
    assert_eq!(reply.status, 200);
    assert!(started.elapsed() >= current_relay::FIRST_REQUEST_TIMEOUT - Duration::from_millis(500));
    for mut stream in idle {
        closed(&mut stream).await;
    }
}

/// An upstream that never answers, noting when the request it was given is dropped.
struct Never {
    sent: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Upstream for Never {
    fn send(&self, _request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        let flag = DropFlag(self.dropped.clone());
        Box::pin(async move {
            let _flag = flag;
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn removing_a_route_while_its_request_waits_upstream_ends_both() {
    let relay = Relay::new(&tokio::runtime::Handle::current());
    let (sent, dropped) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    let (sink, _events) = mpsc::unbounded_channel();
    let route = relay
        .add_route(RouteConfig {
            upstream: Arc::new(Never {
                sent: sent.clone(),
                dropped: dropped.clone(),
            }),
            prefixes: vec!["/Videos/abc/".into()],
            events: Arc::new(sink),
        })
        .unwrap();
    let port = route.port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(&request(
            "GET",
            &path_on(&route, "/Videos/abc/stream"),
            port,
            "",
        ))
        .await
        .unwrap();
    timeout(WAIT, async {
        while sent.load(Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(route);
    closed(&mut stream).await;
    timeout(WAIT, async {
        while !dropped.load(Ordering::SeqCst) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the pending upstream request is dropped");
}

#[tokio::test]
async fn a_range_from_inside_a_media_file_is_not_sniffed() {
    // Whitespace, or even the magic, in the middle of a media file is media: only a body from its first byte is sniffed.
    let upstream = Fake::new(|_| {
        Ok(answer(
            206,
            &[
                ("content-type", "video/mp4"),
                ("content-range", "bytes 100-114/1000"),
                ("content-length", "15"),
            ],
            "#EXTM3U\nhttp://",
        ))
    });
    let mut setup = setup(upstream);
    let reply = get(&setup, "/Videos/abc/stream", "Range: bytes=100-114\r\n").await;
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, b"#EXTM3U\nhttp://");
    assert!(drain(&mut setup.events).is_empty());
}
