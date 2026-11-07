//! The stream reader for a platform engine (ADR 0026, R2): the file of a direct-play stream read by byte ranges through the core,
//! instead of a URL the platform's player fetches itself.
//!
//! [`PlatformReader`] is made with the stream's relay route ([`super::server`]) and handed to the platform engine with the load
//! ([`super::platform`]); the driver feeds it the engine's position, duration and buffering, and the backend its seeks. It opens on
//! first use (the size is known only from the server's first answer), and ends with the route: a stop, another item, a re-plan or
//! the player closing cancels it, and its pending reads end with [`ReadError::Cancelled`].
//!
//! Which loads get a reader is the routing rule of ADR 0026 ("Which routes use it", R3): direct play on the platform engine's native
//! route. Server streams, HLS and libmpv keep their URLs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use current_playback::{DirectFile, EngineEvent};
use current_relay::{
    Bytes, FileFacts, ReadError, ReaderFacts, ReaderLimits, ReaderRoute, StreamReader,
};
use tokio::sync::OnceCell;
use tokio::time::Instant;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section leaves the state consistent, so a panic elsewhere does not make it unusable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the plan and the engine said before the reader opened, applied when it does, and kept up to date after.
#[derive(Debug, Default)]
struct Feed {
    position: Duration,
    /// From the plan's source at first, then the engine's.
    duration: Option<Duration>,
    /// From the plan's source, for a file without a duration.
    bitrate: Option<u32>,
    /// The engine says it buffers.
    buffering: bool,
    /// The engine plays (not paused, not ended).
    playing: bool,
    /// When the position last moved, or the engine started, sought or resumed: a stall counts from here.
    moved_at: Option<Instant>,
    /// The position stood still for [`STALL_AFTER`] while playing.
    stalled: bool,
    /// A seek the reader has not been told of: it was not open yet.
    seek: Option<Duration>,
    /// The feed changed while the reader was not open (its first request was running): applied once it is.
    pending: bool,
}

/// How long the position may stand still while the engine plays, after a start, seek or resume too, before the reader counts the
/// engine as starving (ADR 0026, "The playback point"; R3). A guess: R4 measures it on the owner's PC.
pub(crate) const STALL_AFTER: Duration = Duration::from_secs(2);

/// How often the reader looks for a stall. A guess like [`STALL_AFTER`], measured in R4.
pub(crate) const STALL_TICK: Duration = Duration::from_millis(500);

impl Feed {
    /// Whether the engine waits for bytes: it says it buffers, or its position stood still while it played.
    fn starving(&self) -> bool {
        self.buffering || self.stalled
    }

    /// The position may stand still from now for [`STALL_AFTER`] before it is a stall.
    fn grace(&mut self) {
        self.moved_at = Some(Instant::now());
        self.stalled = false;
    }
}

/// One file read for the platform engine. Cheap to share; every call may come from any thread.
pub struct PlatformReader {
    route: ReaderRoute,
    path: String,
    query: Vec<(String, String)>,
    feed: Mutex<Feed>,
    reader: OnceCell<StreamReader>,
    cancelled: AtomicBool,
}

impl std::fmt::Debug for PlatformReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformReader")
            .field("opened", &self.reader.initialized())
            .finish_non_exhaustive()
    }
}

impl PlatformReader {
    /// A reader of `path` with `query` (relative to the server's address, without `ApiKey`) on `route`, for a file with the plan's
    /// `direct` facts: its duration paces the reader from the first read, before the engine can report one.
    pub(crate) fn new(
        route: ReaderRoute,
        path: String,
        query: Vec<(String, String)>,
        direct: DirectFile,
    ) -> Self {
        Self {
            route,
            path,
            query,
            feed: Mutex::new(Feed {
                duration: direct.duration,
                bitrate: direct.bitrate,
                ..Feed::default()
            }),
            reader: OnceCell::new(),
            cancelled: AtomicBool::new(false),
        }
    }

    /// Looks for a stall every [`STALL_TICK`] until the reader ends (a task on the current runtime, holding no strong reference).
    pub(crate) fn watch_for_stalls(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(STALL_TICK).await;
                let Some(reader) = weak.upgrade() else {
                    return;
                };
                if reader.cancelled.load(Ordering::Acquire) {
                    return;
                }
                reader.check_stall();
            }
        });
    }

    /// Counts the engine as starving when its position stood still for [`STALL_AFTER`] while it played, not buffering already.
    fn check_stall(&self) {
        let stalled = {
            let feed = lock(&self.feed);
            feed.playing
                && !feed.buffering
                && !feed.stalled
                && feed
                    .moved_at
                    .is_some_and(|moved| moved.elapsed() >= STALL_AFTER)
        };
        if stalled {
            self.update(|feed| feed.stalled = true, apply_starving);
        }
    }

    /// Opens the file on first use; later calls give the same answer at once: its size and content type.
    ///
    /// # Errors
    /// [`ReadError::Failed`] when the server refused or failed (the route's events say why, and the player shows it),
    /// [`ReadError::Cancelled`] once the route ended.
    pub async fn open(&self) -> Result<(u64, String), ReadError> {
        let reader = self.opened().await?;
        Ok((reader.size(), reader.content_type().to_owned()))
    }

    async fn opened(&self) -> Result<&StreamReader, ReadError> {
        let reader = self
            .reader
            .get_or_try_init(|| async {
                let facts = {
                    let mut feed = lock(&self.feed);
                    // The open takes all of the feed as it is now: what changes while it runs is applied after.
                    feed.seek = None;
                    feed.pending = false;
                    FileFacts {
                        start: feed.position,
                        duration: feed.duration,
                        bitrate: feed.bitrate.map(u64::from),
                        content_type: None,
                    }
                };
                self.route
                    .open(
                        &self.path,
                        self.query.clone(),
                        facts,
                        ReaderLimits::default(),
                    )
                    .await
            })
            .await?;
        self.catch_up(reader);
        Ok(reader)
    }

    /// Applies what the feed got while the reader was not open yet: a change between the open's start and the reader being
    /// stored reached only the feed.
    fn catch_up(&self, reader: &StreamReader) {
        let mut feed = lock(&self.feed);
        if !feed.pending {
            return;
        }
        feed.pending = false;
        if let Some(position) = feed.seek.take() {
            reader.seek_to(position);
        }
        reader.set_position(feed.position);
        if let Some(duration) = feed.duration {
            reader.set_duration(duration);
        }
        reader.set_starving(feed.starving());
    }

    /// Up to `max_len` bytes at `offset`, opening the file first if needed; empty at its end.
    ///
    /// # Errors
    /// As [`StreamReader::read`], and [`PlatformReader::open`]'s.
    pub async fn read(&self, offset: u64, max_len: u32) -> Result<Bytes, ReadError> {
        self.opened().await?.read(offset, max_len).await
    }

    /// What the info panel can show, once open.
    pub fn facts(&self) -> Option<ReaderFacts> {
        self.reader.get().map(StreamReader::facts)
    }

    /// Ends the reader: its reads end with [`ReadError::Cancelled`].
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.route.cancel();
    }

    /// Updates the feed and, once open, the reader; before, the change waits for [`PlatformReader::catch_up`].
    fn update(&self, change: impl FnOnce(&mut Feed), apply: impl FnOnce(&StreamReader, &Feed)) {
        let mut feed = lock(&self.feed);
        change(&mut feed);
        match self.reader.get() {
            Some(reader) => apply(reader, &feed),
            None => feed.pending = true,
        }
    }

    /// Where the load starts.
    pub(crate) fn start_at(&self, start: Duration) {
        self.seek_to(start);
    }

    /// A seek the core sends the engine, before the engine reads there.
    pub(crate) fn seek_to(&self, position: Duration) {
        self.update(
            |feed| {
                feed.position = position;
                feed.seek = Some(position);
                feed.grace();
            },
            |reader, feed| {
                reader.seek_to(feed.position);
                reader.set_starving(feed.starving());
            },
        );
    }

    /// An event the engine reported about this reader's load.
    pub(crate) fn engine_event(&self, event: &EngineEvent) {
        match *event {
            EngineEvent::Position(position) => self.update(
                |feed| {
                    if position != feed.position {
                        feed.position = position;
                        feed.grace();
                    }
                },
                |reader, feed| {
                    reader.set_position(feed.position);
                    reader.set_starving(feed.starving());
                },
            ),
            EngineEvent::Duration(duration) => self.update(
                |feed| feed.duration = Some(duration),
                |reader, _| reader.set_duration(duration),
            ),
            EngineEvent::Buffering(buffering) => self.update(
                |feed| {
                    feed.buffering = buffering;
                    if !buffering {
                        feed.grace();
                    }
                },
                apply_starving,
            ),
            // Opened: not playing yet; playing (again) after the first frame or a resume, with the grace a start needs.
            EngineEvent::Loaded => self.update(
                |feed| {
                    feed.playing = false;
                    feed.buffering = false;
                    feed.stalled = false;
                },
                apply_starving,
            ),
            EngineEvent::Restarted | EngineEvent::Paused(false) => self.update(
                |feed| {
                    feed.playing = true;
                    feed.buffering = false;
                    feed.grace();
                },
                apply_starving,
            ),
            EngineEvent::Paused(true) | EngineEvent::Ended(_) => self.update(
                |feed| {
                    feed.playing = false;
                    feed.stalled = false;
                },
                apply_starving,
            ),
            _ => {}
        }
    }
}

fn apply_starving(reader: &StreamReader, feed: &Feed) {
    reader.set_starving(feed.starving());
}

/// A player's readers: the reader of the stream given last, and the one its platform engine reads now.
#[derive(Debug, Default)]
pub(crate) struct ReaderSlot {
    /// Made by the media server with the stream's relay route; ended with it.
    current: Mutex<Option<Arc<PlatformReader>>>,
    /// The reader of the platform engine's load playing, by the load's generation.
    fed: Mutex<Option<(u64, Arc<PlatformReader>)>>,
}

impl ReaderSlot {
    /// The reader for the stream just given; ends the one before.
    pub(crate) fn set_current(&self, reader: Option<Arc<PlatformReader>>) {
        let old = std::mem::replace(&mut *lock(&self.current), reader);
        if let Some(old) = old {
            old.cancel();
        }
    }

    /// The reader of the stream given last, if it has one.
    pub(crate) fn current(&self) -> Option<Arc<PlatformReader>> {
        lock(&self.current).clone()
    }

    /// The platform engine's load `generation` reads through `reader` (or through none).
    pub(crate) fn feed(&self, generation: u64, reader: Option<Arc<PlatformReader>>) {
        *lock(&self.fed) = reader.map(|reader| (generation, reader));
    }

    /// The reader of the platform engine's load playing, if it reads through one.
    pub(crate) fn fed(&self) -> Option<Arc<PlatformReader>> {
        lock(&self.fed).as_ref().map(|(_, reader)| reader.clone())
    }

    /// An event of the platform engine's load `generation`, already admitted by the load gate.
    pub(crate) fn engine_event(&self, generation: u64, event: &EngineEvent) {
        let reader = lock(&self.fed)
            .as_ref()
            .filter(|(fed, _)| *fed == generation)
            .map(|(_, reader)| reader.clone());
        if let Some(reader) = reader {
            reader.engine_event(event);
        }
    }

    /// The platform engine stopped its load.
    pub(crate) fn stopped(&self) {
        *lock(&self.fed) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use current_net::header::{HeaderMap, HeaderName, HeaderValue};
    use current_net::{StreamError, Url};
    use current_relay::{
        ReaderRouteConfig, RelayEvent, RelayEvents, Upstream, UpstreamBody, UpstreamFuture,
        UpstreamRequest, UpstreamResponse,
    };

    /// 2 GB, generated as it is read: a resume far into it is past the read-ahead and the file's head, as in a real film.
    const SIZE: u64 = 2_000_000_000;
    /// Bytes per second over 100 s.
    const RATE: u64 = SIZE / 100;

    /// A server with one file of [`SIZE`] zero bytes, in 64 KiB chunks.
    struct File;

    struct Body(u64);

    impl UpstreamBody for Body {
        fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>> {
            let take = (SIZE - self.0).min(64 * 1024);
            self.0 += take;
            Box::pin(async move { Ok((take > 0).then(|| Bytes::from(vec![0; take as usize]))) })
        }
    }

    impl Upstream for File {
        fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
            let from: u64 = request
                .headers
                .get("range")
                .and_then(|value| value.to_str().ok())
                .and_then(|range| range.strip_prefix("bytes="))
                .and_then(|range| range.strip_suffix('-'))
                .and_then(|from| from.parse().ok())
                .unwrap_or(0);
            let mut headers = HeaderMap::new();
            let mut header = |name: &'static str, value: String| {
                headers.insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_str(&value).unwrap(),
                );
            };
            header("content-type", "video/mp4".into());
            header("content-range", format!("bytes {from}-{}/{SIZE}", SIZE - 1));
            Box::pin(async move {
                Ok::<_, StreamError>(UpstreamResponse {
                    status: 206,
                    headers,
                    base: Url::parse("https://media.example.com/").unwrap(),
                    body: Box::new(Body(from)),
                })
            })
        }
    }

    struct NoEvents;

    impl RelayEvents for NoEvents {
        fn report(&self, _: RelayEvent) {}
    }

    fn reader() -> PlatformReader {
        reader_with(DirectFile::default())
    }

    fn reader_with(direct: DirectFile) -> PlatformReader {
        let route = ReaderRoute::new(
            &tokio::runtime::Handle::current(),
            ReaderRouteConfig {
                upstream: Arc::new(File),
                prefixes: vec!["/Videos/e1/".into()],
                events: Arc::new(NoEvents),
            },
        )
        .unwrap();
        PlatformReader::new(route, "/Videos/e1/stream".into(), Vec::new(), direct)
    }

    #[tokio::test]
    async fn the_engine_feeds_the_reader_before_and_after_it_opens() {
        let reader = reader();
        // Before opening: the load's start and the duration the engine reported are kept.
        reader.start_at(Duration::from_secs(10));
        reader.engine_event(&EngineEvent::Duration(Duration::from_secs(100)));
        assert!(reader.facts().is_none());
        assert_eq!(reader.open().await.unwrap(), (SIZE, "video/mp4".to_owned()));
        let facts = reader.facts().unwrap();
        assert!(facts.rate_known);
        assert_eq!(facts.point, 10 * RATE);
        // After: positions and seeks move the point.
        reader.engine_event(&EngineEvent::Position(Duration::from_secs(20)));
        assert_eq!(reader.facts().unwrap().point, 20 * RATE);
        reader.seek_to(Duration::from_secs(50));
        assert_eq!(reader.facts().unwrap().point, 50 * RATE);
        // Starving while buffering; no longer once playing again or paused.
        let starving = |reader: &PlatformReader| lock(&reader.feed).starving();
        for (event, after) in [
            (EngineEvent::Buffering(true), true),
            (EngineEvent::Restarted, false),
            (EngineEvent::Buffering(true), true),
            (EngineEvent::Paused(false), false),
            (EngineEvent::Buffering(true), true),
            (EngineEvent::Buffering(false), false),
        ] {
            reader.engine_event(&event);
            assert_eq!(starving(&reader), after, "after {event:?}");
        }
        reader.cancel();
        assert_eq!(reader.read(0, 10).await, Err(ReadError::Cancelled));
    }

    #[tokio::test]
    async fn the_plans_duration_paces_the_reader_from_the_first_read() {
        let reader = reader_with(DirectFile {
            duration: Some(Duration::from_secs(100)),
            bitrate: None,
        });
        reader.start_at(Duration::from_secs(50));
        // The player reads the file's head first: it does not pin the point, which is where 50 s is.
        assert!(!reader.read(0, 10).await.unwrap().is_empty());
        let facts = reader.facts().unwrap();
        assert!(facts.rate_known);
        assert_eq!(facts.point, 50 * RATE);
    }

    #[tokio::test]
    async fn a_duration_the_engine_reports_after_the_head_read_still_finds_the_point() {
        // No duration in the plan: the engine can report one only after it read the head through the reader.
        let reader = reader();
        reader.start_at(Duration::from_secs(50));
        assert!(!reader.read(0, 10).await.unwrap().is_empty());
        assert_eq!(
            reader.facts().unwrap().point,
            0,
            "without a rate the head read pins it"
        );
        reader.engine_event(&EngineEvent::Duration(Duration::from_secs(100)));
        assert_eq!(reader.facts().unwrap().point, 50 * RATE);
        // The player's read at the target pins it there.
        let target = 50 * RATE + 4096;
        assert!(!reader.read(target, 10).await.unwrap().is_empty());
        assert_eq!(reader.facts().unwrap().point, target);
    }

    /// [`File`], whose answers wait for a permit.
    struct Held(Arc<tokio::sync::Semaphore>);

    impl Upstream for Held {
        fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
            let gate = self.0.clone();
            Box::pin(async move {
                gate.acquire().await.unwrap().forget();
                File.send(request).await
            })
        }
    }

    #[tokio::test]
    async fn a_seek_while_the_first_request_runs_reaches_the_reader() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let route = ReaderRoute::new(
            &tokio::runtime::Handle::current(),
            ReaderRouteConfig {
                upstream: Arc::new(Held(gate.clone())),
                prefixes: vec!["/Videos/e1/".into()],
                events: Arc::new(NoEvents),
            },
        )
        .unwrap();
        let reader = Arc::new(PlatformReader::new(
            route,
            "/Videos/e1/stream".into(),
            Vec::new(),
            DirectFile {
                duration: Some(Duration::from_secs(100)),
                bitrate: None,
            },
        ));
        let opening = {
            let reader = reader.clone();
            tokio::spawn(async move { reader.open().await })
        };
        // The open took the feed (start 0) and waits for the server; the seek reaches only the feed.
        tokio::task::yield_now().await;
        assert!(!opening.is_finished());
        reader.seek_to(Duration::from_secs(50));
        gate.add_permits(10);
        opening.await.unwrap().unwrap();
        assert_eq!(reader.facts().unwrap().point, 50 * RATE);
    }

    /// Starts reading at 10 s into the 2 GB file of 100 s, playing, and returns a read held far past the read-ahead.
    async fn playing_with_a_held_read(
        reader: &Arc<PlatformReader>,
    ) -> tokio::task::JoinHandle<Result<Bytes, ReadError>> {
        reader.start_at(Duration::from_secs(10));
        reader.open().await.unwrap();
        reader.engine_event(&EngineEvent::Restarted);
        reader.engine_event(&EngineEvent::Position(Duration::from_secs(10)));
        let held = {
            let reader = reader.clone();
            // 400 MB: past 10 s (200 MB) plus 150 MiB.
            tokio::spawn(async move { reader.read(400_000_000, 10).await })
        };
        tokio::task::yield_now().await;
        held
    }

    fn timed_reader() -> Arc<PlatformReader> {
        let reader = Arc::new(reader_with(DirectFile {
            duration: Some(Duration::from_secs(100)),
            bitrate: None,
        }));
        reader.watch_for_stalls();
        reader
    }

    /// Lets the watchdog and the woken read run.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_position_that_stands_still_while_playing_counts_as_starving() {
        let reader = timed_reader();
        let held = playing_with_a_held_read(&reader).await;
        // Within the grace after the start: still held.
        tokio::time::advance(STALL_AFTER - Duration::from_millis(600)).await;
        settle().await;
        assert!(!held.is_finished());
        // Past it, with no position change: a stall, so the held read goes through.
        tokio::time::advance(Duration::from_millis(1_200)).await;
        settle().await;
        assert!(held.is_finished());
        assert_eq!(held.await.unwrap().unwrap().len(), 10);
        assert_eq!(reader.facts().unwrap().starvation_releases, 1);
        assert!(lock(&reader.feed).starving());
        // The position moves: no longer starving.
        reader.engine_event(&EngineEvent::Position(Duration::from_secs(11)));
        assert!(!lock(&reader.feed).starving());
        reader.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn a_paused_or_buffering_engine_or_a_fresh_seek_is_no_stall() {
        // Paused: the position stands still for good reason.
        let reader = timed_reader();
        let held = playing_with_a_held_read(&reader).await;
        reader.engine_event(&EngineEvent::Paused(true));
        tokio::time::advance(STALL_AFTER * 3).await;
        settle().await;
        assert!(!held.is_finished());
        assert!(!lock(&reader.feed).stalled);
        // Resumed: the grace again, then a stall.
        reader.engine_event(&EngineEvent::Paused(false));
        tokio::time::advance(STALL_AFTER - Duration::from_millis(600)).await;
        settle().await;
        assert!(!held.is_finished());
        tokio::time::advance(Duration::from_millis(1_200)).await;
        settle().await;
        assert!(held.is_finished());
        reader.cancel();

        // Buffering: starving already, and the watchdog adds no stall of its own.
        let reader = timed_reader();
        let _held = playing_with_a_held_read(&reader).await;
        reader.engine_event(&EngineEvent::Buffering(true));
        tokio::time::advance(STALL_AFTER * 3).await;
        settle().await;
        assert!(!lock(&reader.feed).stalled);
        reader.cancel();

        // A seek restarts the grace.
        let reader = timed_reader();
        let held = playing_with_a_held_read(&reader).await;
        tokio::time::advance(STALL_AFTER - Duration::from_millis(600)).await;
        reader.seek_to(Duration::from_secs(10));
        tokio::time::advance(STALL_AFTER - Duration::from_millis(600)).await;
        settle().await;
        assert!(!held.is_finished());
        reader.cancel();
    }
}
