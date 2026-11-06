//! The stream reader (ADR 0026): a file of the server read by byte ranges for a player that takes a stream from us instead of a
//! URL, with libmpv's read-ahead limits.
//!
//! - A [`ReaderRoute`] is a relay route without a port: one server ([`Upstream`]), the path prefixes of the item playing and a
//!   [`RelayEvents`] sink, told once per kind. Dropping it cancels every reader on it.
//! - A [`StreamReader`] reads one file through windows: up to [`ReaderLimits::max_windows`] stretches of the file, each fetched by
//!   one ranged request (`Range: bytes=<offset>-`), so a player that reads in several places at once (the playback position, the
//!   cues at the end, a subtitle scan) does not thrash.
//! - **Ahead:** nothing at or past the playback point's byte offset plus [`ReaderLimits::ahead`] (150 MiB, mpv's
//!   `demuxer-max-bytes`) is fetched or handed out, except in index windows (the last [`ReaderLimits::index_window`] bytes of the
//!   file) up to that size; the playback window prefetches only while the bytes held ahead, summed over the windows, stay within
//!   the same budget.
//! - **Behind:** bytes behind the read positions are kept within [`ReaderLimits::behind`] (50 MiB, mpv's
//!   `demuxer-max-back-bytes`) plus the unused part of the read-ahead (mpv's donation).
//! - **The playback point:** size × position / duration (or the server's bitrate without a duration), pinned at each start and
//!   seek to the first read that can be the playback position; without either, the read that pinned it. **The starvation guard:**
//!   while the player says it is starving, a held read is served and pins the point to itself.
//! - **Failures:** a body that breaks off is asked again from the next byte with `If-Range`, [`ReaderLimits::tries`] times; refusals
//!   and failures report the relay's events, as for a relayed request.
//!
//! No Debug, Display or error here shows a URL, a path or a credential.

mod pump;
mod state;

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use current_net::RuntimeHandle;
use current_net::header::{
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, HeaderValue, LAST_MODIFIED,
};
use hyper::body::Bytes;
use tokio::sync::Notify;

use crate::RelayError;
use crate::events::{RelayEvent, RelayEvents, RouteEvents};
use crate::path::Prefixes;
use crate::upstream::{UpstreamBody, UpstreamResponse};
use state::{State, Step};

/// The reader's limits. [`Default`] is libmpv's (mpv v0.41.0 `demux/demux.c`, ADR 0026); tests use smaller ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLimits {
    /// How far past the playback point the reader fetches and serves (mpv's `demuxer-max-bytes`, 150 MiB).
    pub ahead: u64,
    /// How much is kept behind the read positions, before the donation of unused read-ahead (mpv's `demuxer-max-back-bytes`,
    /// 50 MiB).
    pub behind: u64,
    /// The end of the file where reads open index windows, which may read this far without the limit (8 MiB).
    pub index_window: u64,
    /// Windows per file (4).
    pub max_windows: usize,
    /// How far past a window's end a read still waits for that window instead of opening another (4 MiB); also how far past the
    /// estimate a read may pin the point.
    pub gap: u64,
    /// How many times a window asks again after its body broke off, before its reads fail (3).
    pub tries: u32,
    /// The pause before the first retry, doubled for each next one (500 ms).
    pub backoff: Duration,
}

const MIB: u64 = 1024 * 1024;

impl Default for ReaderLimits {
    fn default() -> Self {
        Self {
            ahead: 150 * MIB,
            behind: 50 * MIB,
            index_window: 8 * MIB,
            max_windows: 4,
            gap: 4 * MIB,
            tries: 3,
            backoff: Duration::from_millis(500),
        }
    }
}

/// What the core knows of the file before it is opened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFacts {
    /// Where playback starts.
    pub start: Duration,
    /// The file's duration, when the server knows it.
    pub duration: Option<Duration>,
    /// The file's bitrate in bits per second, used only without a duration.
    pub bitrate: Option<u64>,
    /// The content type to report when the server sends none.
    pub content_type: Option<String>,
}

/// Why a read or an open failed. Carries nothing that could name the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The route was removed (stop, item change, re-plan, player closed).
    #[error("the stream was stopped")]
    Cancelled,
    /// The reader was closed.
    #[error("the stream was closed")]
    Closed,
    /// The server refused or failed, or its answer could not be used; the route's events say why.
    #[error("the stream could not be read")]
    Failed,
    /// The path is outside the route's prefixes: a bug in the caller.
    #[error("the stream is not one this route serves")]
    NotAllowed,
}

/// What the info panel can show of a reader (ADR 0026, "The info panel").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderFacts {
    pub size: u64,
    /// The playback point's byte offset.
    pub point: u64,
    /// Whether the point follows the playback position (a duration or a bitrate is known); without, it moves only at seeks and
    /// starvation releases.
    pub rate_known: bool,
    /// Bytes fetched and not yet handed out, summed over the windows.
    pub held_ahead: u64,
    /// How far past the point the player has been given bytes.
    pub served_ahead: u64,
    /// The same in playback time, when the rate is known.
    pub served_ahead_time: Option<Duration>,
    /// Bytes handed out per second over the last few seconds.
    pub read_rate: u64,
    pub requests: u64,
    pub reconnects: u64,
    /// Held reads the starvation guard let through.
    pub starvation_releases: u64,
    pub windows: usize,
}

/// What a new reader route serves.
pub struct ReaderRouteConfig {
    pub upstream: Arc<dyn crate::Upstream>,
    /// As [`crate::RouteConfig::prefixes`].
    pub prefixes: Vec<String>,
    pub events: Arc<dyn RelayEvents>,
}

/// A route for the stream reader: the same server, prefixes and events as a relay route, but no port, no token and no URL (ADR
/// 0026, R1). Dropping it, or [`ReaderRoute::remove`], cancels every reader opened on it.
pub struct ReaderRoute {
    core: Arc<RouteCore>,
}

struct RouteCore {
    runtime: RuntimeHandle,
    upstream: Arc<dyn crate::Upstream>,
    prefixes: Prefixes,
    events: Arc<RouteEvents>,
    closed: AtomicBool,
    readers: Mutex<Vec<Weak<Inner>>>,
}

/// One file read for a player. Reads may run at once, from any task. Dropping it, or [`StreamReader::close`], ends its reads with
/// [`ReadError::Closed`] and its requests upstream.
pub struct StreamReader {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    core: Arc<RouteCore>,
    path: String,
    query: Vec<(String, String)>,
    /// `If-Range` for every request after the first: the first answer's `ETag`, else its `Last-Modified`.
    validator: Option<HeaderValue>,
    content_type: String,
    size: u64,
    state: Mutex<State>,
    changed: Notify,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section leaves the state consistent, so a panic elsewhere does not make it unusable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ReaderRoute {
    /// A route whose readers run their requests on `runtime`.
    ///
    /// # Errors
    /// [`RelayError::InvalidPrefix`] for a prefix that is not a clean absolute path ending in `/`.
    pub fn new(runtime: &RuntimeHandle, config: ReaderRouteConfig) -> Result<Self, RelayError> {
        Ok(Self {
            core: Arc::new(RouteCore {
                runtime: runtime.clone(),
                upstream: config.upstream,
                prefixes: Prefixes::new(config.prefixes)?,
                events: Arc::new(RouteEvents::new(config.events)),
                closed: AtomicBool::new(false),
                readers: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Opens `path` (relative to the server's address, inside the route's prefixes) with `query` (without `ApiKey`), asking for
    /// the whole file from its first byte; the answer gives the size and the type.
    ///
    /// # Errors
    /// [`ReadError::NotAllowed`] for a path outside the prefixes, [`ReadError::Cancelled`] once the route is removed,
    /// [`ReadError::Failed`] when the server refuses or fails (reported to the route's events) or gives no size.
    pub async fn open(
        &self,
        path: &str,
        query: Vec<(String, String)>,
        facts: FileFacts,
        limits: ReaderLimits,
    ) -> Result<StreamReader, ReadError> {
        let core = &self.core;
        if core.closed.load(Ordering::Acquire) {
            return Err(ReadError::Cancelled);
        }
        if !core.prefixes.allow(path) {
            return Err(ReadError::NotAllowed);
        }
        let response = pump::request(core, path, &query, 0, None)
            .await
            .map_err(|failure| {
                if let Some(event) = failure.event() {
                    core.events.report(event);
                }
                ReadError::Failed
            })?;
        let Some(size) = size_of(&response) else {
            core.events.report(RelayEvent::BadRange);
            return Err(ReadError::Failed);
        };
        let header = |name| {
            response
                .headers
                .get(name)
                .filter(|value: &&HeaderValue| !value.is_empty())
                .cloned()
        };
        let validator = header(ETAG).or_else(|| header(LAST_MODIFIED));
        let content_type = header(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok().map(str::to_owned))
            .or(facts.content_type.clone())
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let rate = match facts.duration.filter(|duration| !duration.is_zero()) {
            Some(duration) => Some(size as f64 / duration.as_secs_f64()),
            None => facts.bitrate.map(|bits| bits as f64 / 8.0),
        };
        let mut state = State::new(size, limits, rate, facts.start);
        state.requests = 1;
        let inner = Arc::new(Inner {
            core: core.clone(),
            path: path.to_owned(),
            query,
            validator,
            content_type,
            size,
            state: Mutex::new(state),
            changed: Notify::new(),
        });
        {
            let mut state = lock(&inner.state);
            let id = state.open(0);
            inner.start_pump(&mut state, id, Some(response.body));
        }
        let mut readers = lock(&core.readers);
        readers.retain(|reader| reader.strong_count() > 0);
        readers.push(Arc::downgrade(&inner));
        drop(readers);
        // A removal that ran meanwhile did not see this reader.
        if core.closed.load(Ordering::Acquire) {
            inner.close(ReadError::Cancelled);
            return Err(ReadError::Cancelled);
        }
        Ok(StreamReader { inner })
    }

    /// Removes the route; the same as dropping it.
    pub fn remove(self) {}
}

impl Drop for ReaderRoute {
    fn drop(&mut self) {
        self.core.closed.store(true, Ordering::Release);
        let readers = std::mem::take(&mut *lock(&self.core.readers));
        for reader in readers.iter().filter_map(Weak::upgrade) {
            reader.close(ReadError::Cancelled);
        }
    }
}

impl std::fmt::Debug for ReaderRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderRoute").finish_non_exhaustive()
    }
}

/// The file's size: a 206's `Content-Range` total, or a 200's `Content-Length`.
fn size_of(response: &UpstreamResponse) -> Option<u64> {
    let text = |name| {
        response
            .headers
            .get(name)
            .and_then(|value: &HeaderValue| value.to_str().ok())
    };
    match response.status {
        206 => text(CONTENT_RANGE)
            .and_then(|range| range.rsplit_once('/'))
            .and_then(|(_, total)| total.trim().parse().ok()),
        _ => text(CONTENT_LENGTH).and_then(|length| length.trim().parse().ok()),
    }
}

impl Inner {
    fn start_pump(
        self: &Arc<Self>,
        state: &mut State,
        id: u64,
        body: Option<Box<dyn UpstreamBody>>,
    ) {
        let task = self.core.runtime.spawn(pump::run(self.clone(), id, body));
        match state.window_mut(id) {
            Some(window) => window.pump = Some(state::Pump(task)),
            None => task.abort(),
        }
    }

    fn close(&self, why: ReadError) {
        let windows = {
            let mut state = lock(&self.state);
            if state.closed.is_none() {
                state.closed = Some(why);
            }
            std::mem::take(&mut state.windows)
        };
        // Dropping the windows aborts their pumps, outside the lock.
        drop(windows);
        self.changed.notify_waiters();
    }

    fn report(&self, event: RelayEvent) {
        self.core.events.report(event);
    }

    /// Runs `check` under the lock until it gives an answer, waiting for a change between tries. A change cannot be missed: the
    /// wait is registered before each check.
    async fn wait_until<T>(&self, mut check: impl FnMut(&mut State) -> Option<T>) -> T {
        loop {
            let mut notified = pin!(self.changed.notified());
            notified.as_mut().enable();
            if let Some(answer) = check(&mut lock(&self.state)) {
                return answer;
            }
            notified.await;
        }
    }
}

/// Counts a read waiting on a window's next bytes, for as long as it waits; dropped with a cancelled read too.
struct Waiting<'a> {
    inner: &'a Inner,
    window: u64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if let Some(window) = lock(&self.inner.state).window_mut(self.window) {
            window.waiting = window.waiting.saturating_sub(1);
        }
    }
}

impl StreamReader {
    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        self.inner.size
    }

    /// The file's content type, as the server named it (else [`FileFacts::content_type`]).
    pub fn content_type(&self) -> &str {
        &self.inner.content_type
    }

    /// Up to `max_len` bytes at `offset`: fewer at a window's or a chunk's end, empty at or past the end of the file. Waits while
    /// the bytes are not fetched yet or are past the limit. Dropping the future cancels the read and its wait.
    ///
    /// # Errors
    /// [`ReadError::Failed`] when the window's requests failed for good; [`ReadError::Cancelled`] or [`ReadError::Closed`] once
    /// the route is removed or the reader closed.
    pub fn read(
        &self,
        offset: u64,
        max_len: u32,
    ) -> impl Future<Output = Result<Bytes, ReadError>> + Send + '_ {
        self.read_inner(offset, u64::from(max_len))
    }

    async fn read_inner(&self, offset: u64, max_len: u64) -> Result<Bytes, ReadError> {
        let inner = &self.inner;
        // Registered once while this read waits for a window's next bytes, and dropped with the read.
        let mut waiting: Option<Waiting<'_>> = None;
        loop {
            let mut notified = pin!(inner.changed.notified());
            notified.as_mut().enable();
            {
                let mut state = lock(&inner.state);
                if let Some(why) = state.closed {
                    return Err(why);
                }
                match state.read(offset, max_len, Instant::now()) {
                    Step::Data(bytes) => {
                        drop(state);
                        // The playback window may prefetch again.
                        inner.changed.notify_waiters();
                        return Ok(bytes);
                    }
                    Step::Eof => return Ok(Bytes::new()),
                    Step::Failed => return Err(ReadError::Failed),
                    Step::Opened(id) => {
                        inner.start_pump(&mut state, id, None);
                        continue;
                    }
                    Step::Wait {
                        window,
                        missing: true,
                    } if waiting
                        .as_ref()
                        .is_none_or(|waiting| waiting.window != window) =>
                    {
                        if let Some(entry) = state.window_mut(window) {
                            entry.waiting += 1;
                        }
                        drop(state);
                        // Replaced outside the lock: dropping the old one takes it.
                        waiting = Some(Waiting { inner, window });
                        // Tell the window's pump, then check again with a fresh registration so this read does not wake itself.
                        inner.changed.notify_waiters();
                        continue;
                    }
                    Step::Wait { .. } => {}
                }
            }
            notified.await;
        }
    }

    /// The playback position the engine reported.
    pub fn set_position(&self, position: Duration) {
        lock(&self.inner.state).set_position(position);
        self.inner.changed.notify_waiters();
    }

    /// A seek the core sends the player, before the player reads there: the point moves to `position` and is pinned again by the
    /// next read that can be the playback position.
    pub fn seek_to(&self, position: Duration) {
        lock(&self.inner.state).seek_to(position);
        self.inner.changed.notify_waiters();
    }

    /// Whether the engine says it is starving (buffering, or its position stopped while playing).
    pub fn set_starving(&self, starving: bool) {
        lock(&self.inner.state).set_starving(starving);
        self.inner.changed.notify_waiters();
    }

    /// What the info panel shows.
    pub fn facts(&self) -> ReaderFacts {
        lock(&self.inner.state).facts(Instant::now())
    }

    /// Closes the reader; the same as dropping it.
    pub fn close(self) {}
}

impl Drop for StreamReader {
    fn drop(&mut self) {
        self.inner.close(ReadError::Closed);
    }
}

impl std::fmt::Debug for StreamReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamReader")
            .field("size", &self.inner.size)
            .finish_non_exhaustive()
    }
}
