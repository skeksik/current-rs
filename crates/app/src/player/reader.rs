//! The stream reader for a platform engine (ADR 0026, R2): the file of a direct-play stream read by byte ranges through the core,
//! instead of a URL the platform's player fetches itself.
//!
//! [`PlatformReader`] is made with the stream's relay route ([`super::server`]) and handed to the platform engine with the load
//! ([`super::platform`]); the driver feeds it the engine's position, duration and buffering, and the backend its seeks. It opens on
//! first use (the size is known only from the server's first answer), and ends with the route: a stop, another item, a re-plan or
//! the player closing cancels it, and its pending reads end with [`ReadError::Cancelled`].
//!
//! Whether a load gets a reader is [`ReaderSlot::set_enabled`]'s, off by default: a temporary switch until R3 decides the routing
//! rule (ADR 0026, "Which routes use it"). No app turns it on yet.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use current_playback::EngineEvent;
use current_relay::{
    Bytes, FileFacts, ReadError, ReaderFacts, ReaderLimits, ReaderRoute, StreamReader,
};
use tokio::sync::OnceCell;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section leaves the state consistent, so a panic elsewhere does not make it unusable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the engine said before the reader opened, applied when it does, and kept up to date after.
#[derive(Debug, Default)]
struct Feed {
    position: Duration,
    duration: Option<Duration>,
    starving: bool,
}

/// One file read for the platform engine. Cheap to share; every call may come from any thread.
pub struct PlatformReader {
    route: ReaderRoute,
    path: String,
    query: Vec<(String, String)>,
    feed: Mutex<Feed>,
    reader: OnceCell<StreamReader>,
}

impl std::fmt::Debug for PlatformReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformReader")
            .field("opened", &self.reader.initialized())
            .finish_non_exhaustive()
    }
}

impl PlatformReader {
    /// A reader of `path` with `query` (relative to the server's address, without `ApiKey`) on `route`.
    pub(crate) fn new(route: ReaderRoute, path: String, query: Vec<(String, String)>) -> Self {
        Self {
            route,
            path,
            query,
            feed: Mutex::default(),
            reader: OnceCell::new(),
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
        self.reader
            .get_or_try_init(|| async {
                let facts = {
                    let feed = lock(&self.feed);
                    FileFacts {
                        start: feed.position,
                        duration: feed.duration,
                        bitrate: None,
                        content_type: None,
                    }
                };
                let reader = self
                    .route
                    .open(
                        &self.path,
                        self.query.clone(),
                        facts,
                        ReaderLimits::default(),
                    )
                    .await?;
                // What the engine said while the first request ran.
                let feed = lock(&self.feed);
                reader.set_position(feed.position);
                if let Some(duration) = feed.duration {
                    reader.set_duration(duration);
                }
                reader.set_starving(feed.starving);
                Ok(reader)
            })
            .await
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
        self.route.cancel();
    }

    /// Updates the feed and, once open, the reader.
    fn update(&self, change: impl FnOnce(&mut Feed), apply: impl FnOnce(&StreamReader, &Feed)) {
        let mut feed = lock(&self.feed);
        change(&mut feed);
        if let Some(reader) = self.reader.get() {
            apply(reader, &feed);
        }
    }

    /// Where the load starts.
    pub(crate) fn start_at(&self, start: Duration) {
        self.update(
            |feed| feed.position = start,
            |reader, feed| reader.seek_to(feed.position),
        );
    }

    /// A seek the core sends the engine, before the engine reads there.
    pub(crate) fn seek_to(&self, position: Duration) {
        self.update(
            |feed| feed.position = position,
            |reader, feed| reader.seek_to(feed.position),
        );
    }

    /// An event the engine reported about this reader's load.
    pub(crate) fn engine_event(&self, event: &EngineEvent) {
        match *event {
            EngineEvent::Position(position) => self.update(
                |feed| feed.position = position,
                |reader, feed| reader.set_position(feed.position),
            ),
            EngineEvent::Duration(duration) => self.update(
                |feed| feed.duration = Some(duration),
                |reader, _| reader.set_duration(duration),
            ),
            EngineEvent::Buffering(starving) => self.starving(starving),
            // Playing, or paused: not waiting for bytes. "Its position stops while playing" is R3's (ADR 0026, "As built (R2)").
            EngineEvent::Loaded | EngineEvent::Restarted | EngineEvent::Paused(_) => {
                self.starving(false);
            }
            _ => {}
        }
    }

    fn starving(&self, starving: bool) {
        self.update(
            |feed| feed.starving = starving,
            |reader, feed| reader.set_starving(feed.starving),
        );
    }
}

/// A player's readers: whether loads get one, the reader of the stream given last, and the one its platform engine reads now.
#[derive(Debug, Default)]
pub(crate) struct ReaderSlot {
    enabled: AtomicBool,
    /// Made by the media server with the stream's relay route; ended with it.
    current: Mutex<Option<Arc<PlatformReader>>>,
    /// The reader of the platform engine's load playing, by the load's generation.
    fed: Mutex<Option<(u64, Arc<PlatformReader>)>>,
}

impl ReaderSlot {
    /// Turns readers on or off for the loads that follow (ADR 0026: until R3 decides the routing rule).
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

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

    const SIZE: u64 = 1_000_000;

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
        let route = ReaderRoute::new(
            &tokio::runtime::Handle::current(),
            ReaderRouteConfig {
                upstream: Arc::new(File),
                prefixes: vec!["/Videos/e1/".into()],
                events: Arc::new(NoEvents),
            },
        )
        .unwrap();
        PlatformReader::new(route, "/Videos/e1/stream".into(), Vec::new())
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
        assert_eq!(facts.point, 100_000);
        // After: positions and seeks move the point.
        reader.engine_event(&EngineEvent::Position(Duration::from_secs(20)));
        assert_eq!(reader.facts().unwrap().point, 200_000);
        reader.seek_to(Duration::from_secs(50));
        assert_eq!(reader.facts().unwrap().point, 500_000);
        // Starving while buffering; no longer once playing again or paused.
        let starving = |reader: &PlatformReader| lock(&reader.feed).starving;
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
}
