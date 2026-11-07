//! The stream reader against a fake upstream holding one file in memory (ADR 0026, R1). Limits are scaled down (64 KiB ahead,
//! 16 KiB behind) so the file stays small; the rules are the same as with libmpv's numbers.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_net::header::{HeaderMap, HeaderName, HeaderValue};
use current_net::{GatewayShape, StreamError, Url};
use current_relay::{
    Bytes, FileFacts, ReadError, ReaderLimits, ReaderRoute, ReaderRouteConfig, RelayEvent,
    StreamReader, Upstream, UpstreamBody, UpstreamFuture, UpstreamRequest, UpstreamResponse,
};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::{sleep, timeout};

const BASE: &str = "https://media.example.com/jf/";
const PATH: &str = "/Videos/e1/stream";
const SIZE: u64 = 1024 * 1024;
const KIB: u64 = 1024;
const CHUNK: usize = 4 * 1024;
const ETAG: &str = "\"v1\"";
/// The file plays for 100 s: 10485.76 bytes per second.
const DURATION: Duration = Duration::from_secs(100);
/// How long a read that should wait is given before it counts as waiting.
const HELD: Duration = Duration::from_millis(150);
const WAIT: Duration = Duration::from_secs(10);

fn data() -> Bytes {
    (0..SIZE)
        .map(|at| (at % 251) as u8)
        .collect::<Vec<_>>()
        .into()
}

fn limits() -> ReaderLimits {
    ReaderLimits {
        ahead: 64 * KIB,
        behind: 16 * KIB,
        index_window: 16 * KIB,
        max_windows: 4,
        gap: 8 * KIB,
        tries: 3,
        backoff: Duration::ZERO,
    }
}

fn facts() -> FileFacts {
    FileFacts {
        start: Duration::ZERO,
        duration: Some(DURATION),
        bitrate: None,
        content_type: None,
    }
}

fn rate_bytes(seconds: f64) -> u64 {
    (seconds * SIZE as f64 / DURATION.as_secs_f64()) as u64
}

#[derive(Default)]
struct Behaviour {
    /// Per request, in order: break the body after this many bytes.
    breaks: VecDeque<u64>,
    /// Every request after the first fails with this.
    later_fail: Option<StreamError>,
    /// A request with `If-Range` gets a 200 with the whole file: the file changed.
    changed: bool,
    /// The same, only for a request from the first byte.
    changed_at_start: bool,
    /// Every request fails with this.
    refuse: Option<StreamError>,
    /// Every request is answered with an HTML page.
    html: bool,
    /// Bodies of requests from this offset on wait for a permit per chunk.
    gate: Option<(u64, Arc<Semaphore>)>,
    /// The file's length; [`SIZE`] when `None`.
    size: Option<u64>,
    /// The validator headers sent instead of a strong `ETag` (`None`: [`ETAG`]).
    validators: Option<Vec<(&'static str, String)>>,
    /// Every request after the first answers with this total in its `Content-Range`.
    later_total: Option<u64>,
    /// Every request after the first gets a 416.
    later_416: bool,
    /// Every answer waits for a permit before its headers come.
    answer_gate: Option<Arc<Semaphore>>,
}

/// A server holding one file: answers `Range: bytes=<from>-` with a 206 in chunks of [`CHUNK`].
struct FakeFile {
    data: Bytes,
    sent: Mutex<Vec<(Option<String>, Option<String>)>>,
    pulled: Arc<AtomicU64>,
    /// Bytes pulled from gated bodies only.
    gated: Arc<AtomicU64>,
    live: Arc<AtomicUsize>,
    behaviour: Mutex<Behaviour>,
}

impl FakeFile {
    fn new() -> Arc<Self> {
        Self::with(Behaviour::default())
    }

    fn with(behaviour: Behaviour) -> Arc<Self> {
        let data = match behaviour.size {
            Some(size) => data().slice(..size as usize),
            None => data(),
        };
        Arc::new(Self {
            data,
            sent: Mutex::new(Vec::new()),
            pulled: Arc::new(AtomicU64::new(0)),
            gated: Arc::new(AtomicU64::new(0)),
            live: Arc::new(AtomicUsize::new(0)),
            behaviour: Mutex::new(behaviour),
        })
    }

    fn sent(&self) -> Vec<(Option<String>, Option<String>)> {
        self.sent.lock().unwrap().clone()
    }

    fn pulled(&self) -> u64 {
        self.pulled.load(Ordering::SeqCst)
    }

    fn answer(&self, request: &UpstreamRequest) -> Result<UpstreamResponse, StreamError> {
        let text = |name: &str| {
            request
                .headers
                .get(name)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        let range = text("range");
        let if_range = text("if-range");
        let first = {
            let mut sent = self.sent.lock().unwrap();
            sent.push((range.clone(), if_range.clone()));
            sent.len() == 1
        };
        assert_eq!(request.path, PATH);
        let mut behaviour = self.behaviour.lock().unwrap();
        if let Some(error) = behaviour.refuse.clone() {
            return Err(error);
        }
        if !first && let Some(error) = behaviour.later_fail.clone() {
            return Err(error);
        }
        let from: u64 = range
            .as_deref()
            .and_then(|range| range.strip_prefix("bytes="))
            .and_then(|range| range.strip_suffix('-'))
            .map(|from| from.parse().unwrap())
            .unwrap_or(0);
        let size = self.data.len() as u64;
        let whole =
            if_range.is_some() && (behaviour.changed || behaviour.changed_at_start && from == 0);
        let from = if whole { 0 } else { from };
        let mut pairs = vec![("content-type", "video/x-matroska".to_owned())];
        match &behaviour.validators {
            Some(validators) => pairs.extend(validators.iter().cloned()),
            None => pairs.push(("etag", ETAG.to_owned())),
        }
        let refused_range = !first && behaviour.later_416;
        let status = if refused_range {
            pairs.push(("content-range", format!("bytes */{size}")));
            416
        } else if whole {
            pairs.push(("content-length", size.to_string()));
            200
        } else {
            let total = behaviour.later_total.filter(|_| !first).unwrap_or(size);
            pairs.push((
                "content-range",
                format!("bytes {from}-{}/{total}", size - 1),
            ));
            206
        };
        let from = if refused_range { size } else { from };
        if behaviour.html {
            pairs[0].1 = "text/html".to_owned();
        }
        let gate = behaviour
            .gate
            .as_ref()
            .filter(|(at, _)| from >= *at)
            .map(|(_, gate)| gate.clone());
        self.live.fetch_add(1, Ordering::SeqCst);
        Ok(UpstreamResponse {
            status,
            headers: headers(&pairs),
            base: Url::parse(BASE).unwrap(),
            body: Box::new(FileBody {
                data: self.data.slice(from as usize..),
                break_after: behaviour.breaks.pop_front(),
                sent: 0,
                pulled: self.pulled.clone(),
                gated: self.gated.clone(),
                live: self.live.clone(),
                gate,
            }),
        })
    }
}

impl Upstream for FakeFile {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
        let gate = self.behaviour.lock().unwrap().answer_gate.clone();
        let answer = self.answer(&request);
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            answer
        })
    }
}

struct FileBody {
    data: Bytes,
    break_after: Option<u64>,
    sent: u64,
    pulled: Arc<AtomicU64>,
    gated: Arc<AtomicU64>,
    live: Arc<AtomicUsize>,
    gate: Option<Arc<Semaphore>>,
}

impl UpstreamBody for FileBody {
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            if let Some(gate) = &self.gate {
                gate.acquire().await.unwrap().forget();
            }
            let mut take = CHUNK.min(self.data.len());
            if let Some(limit) = self.break_after {
                if self.sent >= limit {
                    return Err(StreamError::Unreachable);
                }
                take = take.min((limit - self.sent) as usize);
            }
            if take == 0 {
                return Ok(None);
            }
            let chunk = self.data.split_to(take);
            self.sent += take as u64;
            self.pulled.fetch_add(take as u64, Ordering::SeqCst);
            if self.gate.is_some() {
                self.gated.fetch_add(take as u64, Ordering::SeqCst);
            }
            Ok(Some(chunk))
        })
    }
}

impl Drop for FileBody {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    map
}

fn route(upstream: Arc<FakeFile>) -> (ReaderRoute, mpsc::UnboundedReceiver<RelayEvent>) {
    let (events, received) = mpsc::unbounded_channel();
    let route = ReaderRoute::new(
        &tokio::runtime::Handle::current(),
        ReaderRouteConfig {
            upstream,
            prefixes: vec!["/Videos/e1/".to_owned()],
            events: Arc::new(events),
        },
    )
    .unwrap();
    (route, received)
}

async fn open(route: &ReaderRoute, limits: ReaderLimits, facts: FileFacts) -> StreamReader {
    route
        .open(PATH, vec![("static".into(), "true".into())], facts, limits)
        .await
        .unwrap()
}

async fn setup(
    upstream: Arc<FakeFile>,
) -> (
    ReaderRoute,
    StreamReader,
    mpsc::UnboundedReceiver<RelayEvent>,
) {
    let (route, events) = route(upstream);
    let reader = open(&route, limits(), facts()).await;
    (route, reader, events)
}

fn drain(events: &mut mpsc::UnboundedReceiver<RelayEvent>) -> Vec<RelayEvent> {
    let mut all = Vec::new();
    while let Ok(event) = events.try_recv() {
        all.push(event);
    }
    all
}

/// A read that completes within [`HELD`], else `None`.
async fn read_now(
    reader: &StreamReader,
    offset: u64,
    len: u32,
) -> Option<Result<Bytes, ReadError>> {
    timeout(HELD, reader.read(offset, len)).await.ok()
}

/// Reads from `from` on until a read is held; checks every byte, and returns where it stopped.
async fn read_until_held(reader: &StreamReader, from: u64) -> u64 {
    let expected = data();
    let mut at = from;
    while let Some(read) = read_now(reader, at, 8 * 1024).await {
        let bytes = read.unwrap();
        if bytes.is_empty() {
            break;
        }
        assert_eq!(
            bytes,
            expected.slice(at as usize..at as usize + bytes.len())
        );
        at += bytes.len() as u64;
    }
    at
}

/// Waits until `check` holds.
async fn until(mut check: impl FnMut() -> bool) {
    timeout(WAIT, async {
        while !check() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition not reached");
}

/// Lets the pumps run until nothing changes.
async fn settle() {
    sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn opening_reads_size_type_and_stops_at_the_read_ahead() {
    let upstream = FakeFile::new();
    let (_route, reader, mut events) = setup(upstream.clone()).await;
    assert_eq!(reader.size(), SIZE);
    assert_eq!(reader.content_type(), "video/x-matroska");
    settle().await;
    // A reader nobody reads holds exactly the read-ahead and pulls at most one chunk more.
    let facts = reader.facts();
    assert_eq!(facts.held_ahead, 64 * KIB);
    assert!(upstream.pulled() <= 64 * KIB + CHUNK as u64);
    assert_eq!(upstream.sent(), vec![(Some("bytes=0-".into()), None)]);
    assert!(facts.rate_known);
    assert_eq!(facts.requests, 1);
    assert!(drain(&mut events).is_empty());
}

#[tokio::test]
async fn the_forward_limit_is_byte_exact_and_moves_with_the_position() {
    let upstream = FakeFile::new();
    let (_route, reader, _events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    // One second later the point is 10485 bytes on.
    reader.set_position(Duration::from_secs(1));
    assert_eq!(reader.facts().point, rate_bytes(1.0));
    assert_eq!(
        read_until_held(&reader, 64 * KIB).await,
        rate_bytes(1.0) + 64 * KIB
    );
    settle().await;
    assert!(upstream.pulled() <= rate_bytes(1.0) + 64 * KIB + CHUNK as u64);
    assert_eq!(upstream.sent().len(), 1);
}

#[tokio::test]
async fn the_read_ahead_budget_is_summed_over_the_windows() {
    let upstream = FakeFile::new();
    let (_route, reader, _events) = setup(upstream.clone()).await;
    // Pin the point at the start, then move it so the limit is far enough for a second window.
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    reader.set_position(Duration::from_secs(5));
    settle().await;
    let playback = reader.facts().held_ahead;
    assert_eq!(playback, 64 * KIB);
    // A read past the playback window opens a second one, which fetches for its read beyond the budget...
    let far = 100_000;
    read_now(&reader, far, 1).await.unwrap().unwrap();
    settle().await;
    let facts = reader.facts();
    assert_eq!(facts.windows, 2);
    assert!(facts.held_ahead <= 64 * KIB + CHUNK as u64);
    // ...and the playback window refills only to what the budget leaves.
    read_until_held_from(&reader, 1, 32 * KIB).await;
    settle().await;
    assert_eq!(reader.facts().held_ahead, 64 * KIB);
}

/// Reads `count` bytes from `from` on.
async fn read_until_held_from(reader: &StreamReader, from: u64, count: u64) {
    let mut at = from;
    while at < from + count {
        let len = (from + count - at).min(8 * KIB) as u32;
        at += read_now(reader, at, len).await.unwrap().unwrap().len() as u64;
    }
}

#[tokio::test]
async fn behind_keeps_the_back_budget_plus_the_unused_read_ahead() {
    let upstream = FakeFile::new();
    let (_route, reader, _events) = setup(upstream.clone()).await;
    // Held at 64 KiB with the point at 0: 48 KiB read leaves 16 KiB ahead, so 16 + 48 KiB may stay behind (donation).
    read_until_held_from(&reader, 0, 48 * KIB).await;
    settle().await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    assert_eq!(upstream.sent().len(), 1, "the start was kept");
    // With the point just past where the reads end and the read-ahead full, only 16 KiB stay behind.
    reader.set_position(Duration::from_secs(34));
    read_until_held_from(&reader, 48 * KIB, 300 * KIB).await;
    settle().await;
    assert_eq!(reader.facts().held_ahead, 64 * KIB);
    read_now(&reader, 348 * KIB - 8 * KIB, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(upstream.sent().len(), 1, "8 KiB back is kept");
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    assert_eq!(upstream.sent().len(), 2, "the start was dropped");
}

#[tokio::test]
async fn a_seek_moves_the_point_and_the_next_read_pins_it() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    reader.set_position(Duration::from_secs(2));
    assert_eq!(reader.facts().point, rate_bytes(2.0));
    reader.seek_to(Duration::from_secs(50));
    let estimate = rate_bytes(50.0);
    assert_eq!(reader.facts().point, estimate);
    // The player's first read there is where the file holds that position.
    let anchor = estimate + 5_000;
    read_now(&reader, anchor, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, anchor);
    reader.set_position(Duration::from_secs(51));
    assert_eq!(reader.facts().point, anchor + rate_bytes(1.0));
}

#[tokio::test]
async fn only_a_read_at_the_playback_position_pins_the_point() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    reader.seek_to(Duration::from_secs(50));
    let estimate = rate_bytes(50.0);
    // A scan further than the read-ahead past the estimate is held; it, the file's head and an index read pin nothing.
    assert!(
        read_now(&reader, estimate + 64 * KIB + 8 * KIB, 1)
            .await
            .is_none()
    );
    read_now(&reader, 100, 1).await.unwrap().unwrap();
    read_now(&reader, SIZE - 1000, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, estimate);
    // A read a little before the estimate does.
    read_now(&reader, estimate - 1_000, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reader.facts().point, estimate - 1_000);
}

#[tokio::test]
async fn a_held_read_is_served_while_the_player_starves() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    let reader = Arc::new(reader);
    let held = {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(64 * KIB, 100).await })
    };
    sleep(HELD).await;
    assert!(!held.is_finished());
    reader.set_starving(true);
    let bytes = timeout(WAIT, held).await.unwrap().unwrap().unwrap();
    assert_eq!(bytes, data().slice(64 * 1024..64 * 1024 + 100));
    let facts = reader.facts();
    assert_eq!(facts.starvation_releases, 1);
    assert_eq!(facts.point, 64 * KIB);
}

#[tokio::test]
async fn index_windows_read_without_the_limit_up_to_their_size() {
    let upstream = FakeFile::new();
    let (_route, reader, _events) = setup(upstream.clone()).await;
    // The last 16 KiB of the file, far past the 64 KiB limit, read to the end.
    assert_eq!(read_until_held(&reader, SIZE - 16 * KIB).await, SIZE);
    assert_eq!(read_now(&reader, SIZE, 10).await.unwrap().unwrap().len(), 0);
    // Just before them, past the limit: held, until the player starves.
    assert!(read_now(&reader, SIZE - 32 * KIB, 10).await.is_none());
    reader.set_starving(true);
    assert!(read_now(&reader, SIZE - 32 * KIB, 10).await.is_some());
}

#[tokio::test]
async fn a_broken_body_is_asked_again_with_if_range() {
    let upstream = FakeFile::with(Behaviour {
        breaks: VecDeque::from([20_000]),
        ..Behaviour::default()
    });
    let (_route, reader, mut events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    assert_eq!(
        upstream.sent(),
        vec![
            (Some("bytes=0-".into()), None),
            (Some("bytes=20000-".into()), Some(ETAG.into())),
        ]
    );
    let facts = reader.facts();
    assert_eq!(facts.reconnects, 1);
    assert_eq!(facts.requests, 2);
    assert!(drain(&mut events).is_empty());
}

#[tokio::test]
async fn a_file_changed_on_the_server_fails_as_a_bad_range() {
    let upstream = FakeFile::with(Behaviour {
        breaks: VecDeque::from([20_000]),
        changed: true,
        ..Behaviour::default()
    });
    let (_route, reader, mut events) = setup(upstream).await;
    let at = read_until_held_upto(&reader, 20_000).await;
    assert_eq!(at, 20_000);
    assert_eq!(
        timeout(WAIT, reader.read(20_000, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(drain(&mut events), vec![RelayEvent::BadRange]);
}

/// Reads from 0 until a read fails or `end` is reached.
async fn read_until_held_upto(reader: &StreamReader, end: u64) -> u64 {
    let mut at = 0;
    while at < end {
        let len = (end - at).min(8 * KIB) as u32;
        match timeout(WAIT, reader.read(at, len)).await.unwrap() {
            Ok(bytes) => at += bytes.len() as u64,
            Err(_) => break,
        }
    }
    at
}

#[tokio::test]
async fn a_window_fails_after_its_tries_and_reports_network_once() {
    let upstream = FakeFile::with(Behaviour {
        breaks: VecDeque::from([20_000]),
        later_fail: Some(StreamError::Unreachable),
        ..Behaviour::default()
    });
    let (_route, reader, mut events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held_upto(&reader, 20_000).await, 20_000);
    assert_eq!(
        timeout(WAIT, reader.read(20_000, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(
        timeout(WAIT, reader.read(20_000, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(reader.facts().reconnects, 3);
    assert_eq!(upstream.sent().len(), 4);
    assert_eq!(drain(&mut events), vec![RelayEvent::Network]);
}

#[tokio::test]
async fn refusals_and_failures_on_open_are_reported_once_per_route() {
    let cases = [
        (
            Behaviour {
                refuse: Some(StreamError::Refused {
                    status: 403,
                    shape: GatewayShape::Refused,
                }),
                ..Behaviour::default()
            },
            RelayEvent::Refused {
                status: 403,
                shape: GatewayShape::Refused,
            },
        ),
        (
            Behaviour {
                refuse: Some(StreamError::Status { status: 404 }),
                ..Behaviour::default()
            },
            RelayEvent::NotFound,
        ),
        (
            Behaviour {
                refuse: Some(StreamError::Status { status: 500 }),
                ..Behaviour::default()
            },
            RelayEvent::Server { status: 500 },
        ),
        (
            Behaviour {
                html: true,
                ..Behaviour::default()
            },
            RelayEvent::Refused {
                status: 206,
                shape: GatewayShape::SignIn,
            },
        ),
    ];
    for (behaviour, event) in cases {
        let (route, mut events) = route(FakeFile::with(behaviour));
        for _ in 0..2 {
            let opened = route.open(PATH, Vec::new(), facts(), limits()).await;
            assert_eq!(opened.unwrap_err(), ReadError::Failed);
        }
        assert_eq!(drain(&mut events), vec![event]);
    }
}

#[tokio::test]
async fn a_path_outside_the_prefixes_is_not_opened() {
    let upstream = FakeFile::new();
    let (route, _events) = route(upstream.clone());
    let opened = route
        .open("/Videos/other/stream", Vec::new(), facts(), limits())
        .await;
    assert_eq!(opened.unwrap_err(), ReadError::NotAllowed);
    assert!(upstream.sent().is_empty());
}

#[tokio::test]
async fn removing_the_route_cancels_reads_and_stops_the_requests() {
    let upstream = FakeFile::new();
    let (route, reader, _events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    let reader = Arc::new(reader);
    let held = {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(64 * KIB, 10).await })
    };
    sleep(HELD).await;
    route.remove();
    assert_eq!(
        timeout(WAIT, held).await.unwrap().unwrap(),
        Err(ReadError::Cancelled)
    );
    assert_eq!(reader.read(0, 10).await, Err(ReadError::Cancelled));
    until(|| upstream.live.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn closing_the_reader_ends_its_requests() {
    let upstream = FakeFile::new();
    let (_route, reader, _events) = setup(upstream.clone()).await;
    settle().await;
    reader.close();
    until(|| upstream.live.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn the_least_recently_read_window_goes_first() {
    let upstream = FakeFile::new();
    let (route, _events) = route(upstream.clone());
    let wide = ReaderLimits {
        ahead: SIZE,
        max_windows: 2,
        ..limits()
    };
    let reader = open(&route, wide, facts()).await;
    for offset in [300_000, 600_000] {
        read_now(&reader, offset, 1).await.unwrap().unwrap();
    }
    assert_eq!(reader.facts().windows, 2);
    // The window at 0 was dropped for the one at 600 000: reading there asks again.
    read_now(&reader, 10, 1).await.unwrap().unwrap();
    let sent = upstream.sent();
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[3].0.as_deref(), Some("bytes=10-"));
}

#[tokio::test]
async fn the_end_of_the_file_reads_empty() {
    let (route, _events) = route(FakeFile::new());
    let wide = ReaderLimits {
        ahead: SIZE,
        ..limits()
    };
    let reader = open(&route, wide, facts()).await;
    let last = read_now(&reader, SIZE - 10, 100).await.unwrap().unwrap();
    assert_eq!(last, data().slice(SIZE as usize - 10..));
    assert!(
        read_now(&reader, SIZE, 100)
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        read_now(&reader, SIZE + 5, 100)
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn two_reads_on_different_windows_run_at_once() {
    let (route, _events) = route(FakeFile::new());
    let wide = ReaderLimits {
        ahead: SIZE,
        ..limits()
    };
    let reader = open(&route, wide, facts()).await;
    let (first, second) = tokio::join!(reader.read(200_000, 100), reader.read(700_000, 100));
    let expected = data();
    assert_eq!(first.unwrap(), expected.slice(200_000..200_100));
    assert_eq!(second.unwrap(), expected.slice(700_000..700_100));
    assert_eq!(reader.facts().windows, 3);
}

#[tokio::test]
async fn a_cancelled_read_stops_what_its_window_fetched_for_it() {
    let gate = Arc::new(Semaphore::new(0));
    let far = 500_000;
    let upstream = FakeFile::with(Behaviour {
        gate: Some((far, gate.clone())),
        ..Behaviour::default()
    });
    let (route, _events) = route(upstream.clone());
    let wide = ReaderLimits {
        ahead: SIZE,
        ..limits()
    };
    let reader = Arc::new(open(&route, wide, facts()).await);
    // The player's first read, at the start, pins the point there.
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    let gated = || upstream.gated.load(Ordering::SeqCst);
    // A read three chunks into a window that is not the playback window: it fetches only for the read.
    let waiting = {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(far + 3 * CHUNK as u64, 10).await })
    };
    gate.add_permits(1);
    until(|| gated() == CHUNK as u64).await;
    waiting.abort();
    let _ = waiting.await;
    gate.add_permits(10);
    settle().await;
    // At most the chunk already asked for when the read went: nothing more is fetched for nobody.
    assert!(gated() <= 2 * CHUNK as u64);
    // The control: a read there now gets its bytes, so the window could have fetched them.
    let at = far as usize + 3 * CHUNK;
    let bytes = timeout(WAIT, reader.read(at as u64, 10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, data().slice(at..at + 10));
}

#[tokio::test]
async fn without_a_duration_the_bitrate_paces_the_point() {
    let (route, _events) = route(FakeFile::new());
    let reader = open(
        &route,
        limits(),
        FileFacts {
            duration: None,
            bitrate: Some(80_000),
            ..facts()
        },
    )
    .await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    reader.set_position(Duration::from_secs(2));
    // 80 000 bit/s is 10 000 bytes a second.
    let facts = reader.facts();
    assert!(facts.rate_known);
    assert_eq!(facts.point, 20_000);
}

#[tokio::test]
async fn without_any_rate_the_point_moves_only_at_seeks_and_starvation() {
    let (route, _events) = route(FakeFile::new());
    let reader = open(
        &route,
        limits(),
        FileFacts {
            duration: None,
            bitrate: None,
            ..facts()
        },
    )
    .await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    reader.set_position(Duration::from_secs(30));
    let facts = reader.facts();
    assert!(!facts.rate_known);
    assert_eq!(facts.point, 0);
    assert_eq!(facts.served_ahead_time, None);
    assert!(read_now(&reader, 64 * KIB, 10).await.is_none());
    // The control: the same read goes through once the player starves.
    reader.set_starving(true);
    assert!(read_now(&reader, 64 * KIB, 10).await.is_some());
    reader.set_starving(false);
    // A seek pins the point to the first read after it, wherever it is.
    reader.seek_to(Duration::from_secs(60));
    read_now(&reader, 600_000, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, 600_000);
    assert_eq!(read_until_held(&reader, 600_001).await, 600_000 + 64 * KIB);
    // Starving lets the held read through and pins the point there.
    reader.set_starving(true);
    read_now(&reader, 600_000 + 64 * KIB, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reader.facts().point, 600_000 + 64 * KIB);
}

#[tokio::test]
async fn facts_show_what_was_handed_out() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    read_until_held_from(&reader, 0, 40 * KIB).await;
    let facts = reader.facts();
    assert_eq!(facts.point, 0);
    assert_eq!(facts.served_ahead, 40 * KIB);
    assert_eq!(
        facts.served_ahead_time,
        Some(Duration::from_secs_f64(
            40.0 * 1024.0 / (SIZE as f64 / 100.0)
        ))
    );
    assert_eq!(facts.read_rate, 40 * KIB / 5);
    assert_eq!(facts.size, SIZE);
}

/// Starts a read that the limit holds, and checks it is held.
async fn held_read(
    reader: &Arc<StreamReader>,
    offset: u64,
) -> tokio::task::JoinHandle<Result<Bytes, ReadError>> {
    let read = {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(offset, 10).await })
    };
    sleep(HELD).await;
    assert!(!read.is_finished());
    read
}

#[tokio::test]
async fn a_read_held_before_a_seek_does_not_pin_the_point() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    let reader = Arc::new(reader);
    let stale = held_read(&reader, 64 * KIB).await;
    // A seek 10 s on: the read held at the old limit is woken, but it began before the seek.
    reader.seek_to(Duration::from_secs(10));
    settle().await;
    let estimate = rate_bytes(10.0);
    assert_eq!(reader.facts().point, estimate);
    // The player's read at the target pins it, and the read-ahead runs from there.
    assert_eq!(
        read_until_held(&reader, estimate).await,
        estimate + 64 * KIB
    );
    assert_eq!(reader.facts().point, estimate);
    stale.abort();
}

#[tokio::test]
async fn without_any_rate_a_read_held_before_a_seek_does_not_pin_the_point() {
    let (route, _events) = route(FakeFile::new());
    let no_rate = FileFacts {
        duration: None,
        bitrate: None,
        ..facts()
    };
    let reader = Arc::new(open(&route, limits(), no_rate).await);
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    let stale = held_read(&reader, 64 * KIB).await;
    reader.seek_to(Duration::from_secs(60));
    settle().await;
    // The player's first read at the target is served at once and pins the point there.
    read_now(&reader, 600_000, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, 600_000);
    stale.abort();
}

#[tokio::test]
async fn a_seek_target_off_the_estimate_pins_the_point_and_prefetches_at_once() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    reader.seek_to(Duration::from_secs(50));
    let target = rate_bytes(50.0) + 24 * KIB;
    read_now(&reader, target, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, target);
    settle().await;
    // Its window is the playback window: it prefetched to the limit without being asked.
    assert_eq!(reader.facts().held_ahead, 64 * KIB - 1);
}

#[tokio::test]
async fn a_small_file_keeps_its_limit() {
    // 24 KiB, less than twice the 16 KiB index window: no part of it is an index window.
    let upstream = FakeFile::with(Behaviour {
        size: Some(24 * KIB),
        ..Behaviour::default()
    });
    let (route, _events) = route(upstream);
    let small = ReaderLimits {
        ahead: 4 * KIB,
        gap: KIB,
        ..limits()
    };
    let reader = open(&route, small, facts()).await;
    assert_eq!(read_until_held(&reader, 0).await, 4 * KIB);
    assert!(read_now(&reader, 12 * KIB, 10).await.is_none());
    reader.set_starving(true);
    assert!(read_now(&reader, 12 * KIB, 10).await.is_some());
}

#[tokio::test]
async fn a_200_to_if_range_fails_even_from_the_first_byte() {
    let upstream = FakeFile::with(Behaviour {
        changed_at_start: true,
        ..Behaviour::default()
    });
    let (route, mut events) = route(upstream);
    let one = ReaderLimits {
        ahead: SIZE,
        max_windows: 1,
        ..limits()
    };
    let reader = open(&route, one, facts()).await;
    // A read far on drops the window at 0; reading at 0 again asks with If-Range, and the file changed.
    read_now(&reader, 600_000, 1).await.unwrap().unwrap();
    assert_eq!(
        timeout(WAIT, reader.read(0, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(drain(&mut events), vec![RelayEvent::BadRange]);
}

#[tokio::test]
async fn another_total_length_fails_as_a_bad_range() {
    let upstream = FakeFile::with(Behaviour {
        breaks: VecDeque::from([20_000]),
        validators: Some(Vec::new()),
        later_total: Some(SIZE + 1),
        ..Behaviour::default()
    });
    let (_route, reader, mut events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held_upto(&reader, 20_000).await, 20_000);
    assert_eq!(
        timeout(WAIT, reader.read(20_000, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(drain(&mut events), vec![RelayEvent::BadRange]);
    // Without a validator nothing goes as If-Range.
    assert_eq!(upstream.sent()[1], (Some("bytes=20000-".into()), None));
}

#[tokio::test]
async fn a_weak_entity_tag_or_a_date_is_not_sent_as_if_range() {
    for validators in [
        vec![("etag", "W/\"v1\"".to_owned())],
        vec![("last-modified", "Tue, 03 Nov 2026 10:00:00 GMT".to_owned())],
    ] {
        let upstream = FakeFile::with(Behaviour {
            breaks: VecDeque::from([20_000]),
            validators: Some(validators),
            ..Behaviour::default()
        });
        let (_route, reader, _events) = setup(upstream.clone()).await;
        assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
        assert_eq!(upstream.sent()[1], (Some("bytes=20000-".into()), None));
    }
}

#[tokio::test]
async fn a_416_on_a_reconnect_reports_a_bad_range() {
    let upstream = FakeFile::with(Behaviour {
        breaks: VecDeque::from([20_000]),
        later_416: true,
        ..Behaviour::default()
    });
    let (_route, reader, mut events) = setup(upstream).await;
    assert_eq!(read_until_held_upto(&reader, 20_000).await, 20_000);
    assert_eq!(
        timeout(WAIT, reader.read(20_000, 10)).await.unwrap(),
        Err(ReadError::Failed)
    );
    assert_eq!(drain(&mut events), vec![RelayEvent::BadRange]);
}

#[tokio::test]
async fn a_read_held_before_a_seek_is_served_by_starvation_but_pins_nothing() {
    let (_route, reader, _events) = setup(FakeFile::new()).await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    // Playing at 50 s: the limit is 64 KiB past 50 s, and a read waits there.
    reader.set_position(Duration::from_secs(50));
    let old_limit = rate_bytes(50.0) + 64 * KIB;
    assert_eq!(read_until_held(&reader, rate_bytes(50.0)).await, old_limit);
    let reader = Arc::new(reader);
    let stale = held_read(&reader, old_limit).await;
    // A seek back to 20 s, and the player buffers, as it does after a seek.
    reader.seek_to(Duration::from_secs(20));
    reader.set_starving(true);
    let bytes = timeout(WAIT, stale).await.unwrap().unwrap().unwrap();
    assert_eq!(
        bytes,
        data().slice(old_limit as usize..old_limit as usize + 10)
    );
    assert_eq!(reader.facts().point, rate_bytes(20.0));
    reader.set_starving(false);
    // The read at the target still pins the point, and the limit holds from there.
    let target = rate_bytes(20.0);
    assert_eq!(read_until_held(&reader, target).await, target + 64 * KIB);
    assert_eq!(reader.facts().point, target);
}

#[tokio::test]
async fn a_duration_known_after_opening_paces_the_point() {
    let (route, _events) = route(FakeFile::new());
    let unknown = FileFacts {
        duration: None,
        bitrate: None,
        ..facts()
    };
    let reader = open(&route, limits(), unknown).await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    reader.set_position(Duration::from_secs(2));
    assert_eq!(reader.facts().point, 0);
    reader.set_duration(DURATION);
    let facts = reader.facts();
    assert!(facts.rate_known);
    assert_eq!(facts.point, rate_bytes(2.0));
}

#[tokio::test]
async fn cancelling_a_shared_route_ends_its_reads_and_refuses_new_ones() {
    let upstream = FakeFile::new();
    let (route, reader, _events) = setup(upstream.clone()).await;
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    route.cancel();
    assert_eq!(reader.read(0, 10).await, Err(ReadError::Cancelled));
    let opened = route.open(PATH, Vec::new(), facts(), limits()).await;
    assert_eq!(opened.unwrap_err(), ReadError::Cancelled);
    until(|| upstream.live.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn cancelling_the_route_ends_an_open_waiting_for_the_server() {
    let gate = Arc::new(Semaphore::new(0));
    let upstream = FakeFile::with(Behaviour {
        answer_gate: Some(gate.clone()),
        ..Behaviour::default()
    });
    let (route, _events) = route(upstream);
    let route = Arc::new(route);
    let opening = {
        let route = route.clone();
        tokio::spawn(async move {
            route
                .open(PATH, Vec::new(), facts(), limits())
                .await
                .map(|_| ())
        })
    };
    // The control: the open waits for the server's answer.
    sleep(HELD).await;
    assert!(!opening.is_finished());
    route.cancel();
    assert_eq!(
        timeout(WAIT, opening).await.unwrap().unwrap(),
        Err(ReadError::Cancelled)
    );
    gate.add_permits(10);
}

#[tokio::test]
async fn the_head_read_before_the_duration_does_not_pin_the_point_once_it_is_known() {
    // A resume at 50 s by a player that reads the file's head before it can report the duration.
    let (route, _events) = route(FakeFile::new());
    let unknown = FileFacts {
        start: Duration::from_secs(50),
        duration: None,
        bitrate: None,
        ..facts()
    };
    let reader = open(&route, limits(), unknown).await;
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    assert_eq!(
        reader.facts().point,
        0,
        "without a rate the head read pins it"
    );
    reader.set_duration(DURATION);
    reader.set_position(Duration::from_secs(50));
    let estimate = rate_bytes(50.0);
    assert_eq!(reader.facts().point, estimate);
    // The player's read at the target is served and pins the point there.
    let target = estimate + 1_000;
    read_now(&reader, target, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, target);
    assert_eq!(
        read_until_held(&reader, target + 1).await,
        target + 64 * KIB
    );
}

#[tokio::test]
async fn a_right_anchor_pinned_without_a_rate_is_kept_when_the_duration_arrives() {
    let (route, _events) = route(FakeFile::new());
    let unknown = FileFacts {
        duration: None,
        bitrate: None,
        ..facts()
    };
    let reader = Arc::new(open(&route, limits(), unknown).await);
    read_now(&reader, 0, 1).await.unwrap().unwrap();
    // A seek to 50 s; the player's read at the target pins the point, a little before where the duration will put 50 s.
    reader.seek_to(Duration::from_secs(50));
    let target = rate_bytes(50.0) - 1_000;
    read_now(&reader, target, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, target);
    // The read-ahead runs to its front and a read waits there.
    assert_eq!(
        read_until_held(&reader, target + 1).await,
        target + 64 * KIB
    );
    let front = held_read(&reader, target + 64 * KIB).await;
    // The duration arrives: the anchor is right by the estimate, so it stays, and the held read does not pin the point.
    reader.set_duration(DURATION);
    settle().await;
    let facts = reader.facts();
    assert!(facts.rate_known);
    assert_eq!(facts.point, target);
    assert!(!front.is_finished(), "still held at the read-ahead's front");
    front.abort();
}

#[tokio::test]
async fn a_release_far_from_the_estimate_while_opening_serves_without_pinning_the_point() {
    // Started at 10 s, the player reads an MP4's index near the file's end (past the index window and the read-ahead) before its
    // first frame; the opening stall lets it through.
    let (route, _events) = route(FakeFile::new());
    let start = FileFacts {
        start: Duration::from_secs(10),
        ..facts()
    };
    let reader = Arc::new(open(&route, limits(), start).await);
    let estimate = rate_bytes(10.0);
    assert_eq!(reader.facts().point, estimate);
    let index = SIZE - 40 * KIB;
    let held = {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(index, 100).await })
    };
    sleep(HELD).await;
    assert!(!held.is_finished());
    reader.set_starving(true);
    let bytes = timeout(WAIT, held).await.unwrap().unwrap().unwrap();
    assert_eq!(bytes, data().slice(index as usize..index as usize + 100));
    reader.set_starving(false);
    // Served, not pinned: the point stays at the target and playback from there still stops at the read-ahead.
    assert_eq!(reader.facts().starvation_releases, 1);
    assert_eq!(reader.facts().point, estimate);
    reader.restarted();
    assert_eq!(
        read_until_held(&reader, estimate).await,
        estimate + 64 * KIB
    );
    assert_eq!(reader.facts().point, estimate);
}

#[tokio::test]
async fn the_first_frame_pins_again_a_point_a_release_pinned_and_keeps_one_a_read_pinned() {
    // Without a rate the first read pins the point wherever it is, the index at the file's end too: the first frame drops it, and
    // playback's read pins it.
    let (files, _events) = route(FakeFile::new());
    let no_rate = FileFacts {
        duration: None,
        ..facts()
    };
    let reader = open(&files, limits(), no_rate).await;
    let index = SIZE - 40 * KIB;
    read_now(&reader, index, 100).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, index);
    reader.restarted();
    assert_eq!(read_until_held(&reader, 0).await, 64 * KIB);
    assert_eq!(reader.facts().point, 0);

    // A normal start: the player's read at the target pins the point, and the first frame keeps it.
    let (files, _events) = route(FakeFile::new());
    let start = FileFacts {
        start: Duration::from_secs(10),
        ..facts()
    };
    let reader = open(&files, limits(), start).await;
    let target = rate_bytes(10.0) + 1_000;
    read_now(&reader, target, 1).await.unwrap().unwrap();
    assert_eq!(reader.facts().point, target);
    reader.restarted();
    assert_eq!(reader.facts().point, target);
}
