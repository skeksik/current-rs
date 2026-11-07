//! The reader's bookkeeping, without I/O: the windows of the file and their bytes, the playback point and the limits (ADR 0026,
//! "The reader"). Every method runs under the reader's lock and never waits.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use hyper::body::Bytes;
use tokio::task::JoinHandle;

use super::{ReadError, ReaderFacts, ReaderLimits};

/// How far back the read rate looks.
const RATE_SPAN: Duration = Duration::from_secs(5);

/// A pump task, aborted when its window goes.
pub(crate) struct Pump(pub(crate) JoinHandle<()>);

impl Drop for Pump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowStatus {
    Fetching,
    /// The file's last byte is in the window.
    Done,
    /// The window's upstream failed for good; its reads past what it holds fail.
    Failed,
}

/// One stretch of the file, read from the server by one ranged request at a time. Holds the bytes `start..end`, contiguous.
pub(crate) struct Window {
    pub(crate) id: u64,
    pub(crate) start: u64,
    chunks: VecDeque<Bytes>,
    pub(crate) end: u64,
    /// The furthest byte handed out from this window (exclusive), or where it was opened. Bytes before it are "behind".
    pub(crate) cursor: u64,
    last_used: u64,
    /// Opened in the last [`ReaderLimits::index_window`] bytes of the file (cues, tags).
    pub(crate) index: bool,
    pub(crate) status: WindowStatus,
    /// Reads waiting for bytes this window has not fetched yet: they let it fetch past the read-ahead budget.
    pub(crate) waiting: usize,
    /// How far a starvation release of a read begun before the last seek lets this window fetch and serve past the limit: that
    /// read is served, but it does not move the point.
    released_to: u64,
    pub(crate) pump: Option<Pump>,
}

impl Window {
    fn held_ahead(&self) -> u64 {
        self.end.saturating_sub(self.cursor.max(self.start))
    }

    fn held_behind(&self) -> u64 {
        self.cursor.min(self.end).saturating_sub(self.start)
    }

    /// Up to `len` bytes from `offset`, which must be in `start..end`, from one chunk (no copy).
    fn slice(&self, offset: u64, len: u64) -> Bytes {
        let mut at = self.start;
        for chunk in &self.chunks {
            let chunk_len = chunk.len() as u64;
            if offset < at + chunk_len {
                // Both bounds are inside this chunk, whose length is a usize.
                let from = (offset - at) as usize;
                let to = from + (len.min(chunk_len - (offset - at))) as usize;
                return chunk.slice(from..to);
            }
            at += chunk_len;
        }
        Bytes::new()
    }

    /// Drops `count` bytes from the front.
    fn drop_front(&mut self, mut count: u64) {
        while count > 0 {
            let Some(front) = self.chunks.front_mut() else {
                return;
            };
            let len = front.len() as u64;
            if len <= count {
                self.chunks.pop_front();
                self.start += len;
                count -= len;
            } else {
                // `count` is less than this chunk's length, a usize.
                let _ = front.split_to(count as usize);
                self.start += count;
                count = 0;
            }
        }
    }
}

/// Where the playback point was last pinned to a byte: `offset` held the frame at `position`.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    offset: u64,
    position: Duration,
    /// Pinned while no rate was known: without one, any first read pins the point (the file's head included), so such an anchor
    /// is dropped once the rate becomes known ([`State::set_duration`]).
    without_rate: bool,
    /// Pinned by a starvation release, not by a read [`State::maybe_anchor`] accepted: the player's first frame pins it again
    /// ([`State::restarted`]), as it does one the file's head pinned without a rate.
    released: bool,
}

/// What a read does next.
pub(crate) enum Step {
    Data(Bytes),
    Eof,
    Failed,
    /// A window was opened for the read: start its pump, then try again.
    Opened(u64),
    /// Wait for a change. `missing`: the window has not fetched the bytes yet (else the read is held by the limit).
    Wait {
        window: u64,
        missing: bool,
    },
}

/// What a window's pump does next.
pub(crate) enum Fetch {
    /// Append up to this many bytes, starting at the window's end.
    Room {
        from: u64,
        room: u64,
    },
    Wait,
    Stop,
}

pub(crate) struct State {
    pub(crate) size: u64,
    pub(crate) limits: ReaderLimits,
    /// Bytes per second of playback: size / duration, else the server's bitrate; `None` when neither is known.
    rate: Option<f64>,
    pub(crate) windows: Vec<Window>,
    next_id: u64,
    clock: u64,
    position: Duration,
    anchor: Option<Anchor>,
    /// After a start or a seek, until a read pins the point.
    awaiting_anchor: bool,
    /// After a start or a seek, until the player's first frame ([`State::restarted`]).
    awaiting_frame: bool,
    /// Counts seeks. A read stamps it when it begins, and only a read that began after the last seek may pin the point: one held
    /// before it (at the old limit) is woken by the seek but is not the player's read at the new position.
    generation: u64,
    starving: bool,
    pub(crate) closed: Option<ReadError>,
    pub(crate) requests: u64,
    pub(crate) reconnects: u64,
    releases: u64,
    served: VecDeque<(Instant, u64)>,
}

impl State {
    pub(crate) fn new(size: u64, limits: ReaderLimits, rate: Option<f64>, start: Duration) -> Self {
        Self {
            size,
            limits,
            rate: rate.filter(|rate| rate.is_finite() && *rate > 0.0),
            windows: Vec::new(),
            next_id: 0,
            clock: 0,
            position: start,
            anchor: None,
            awaiting_anchor: true,
            awaiting_frame: true,
            generation: 0,
            starving: false,
            closed: None,
            requests: 0,
            reconnects: 0,
            releases: 0,
            served: VecDeque::new(),
        }
    }

    fn bytes_for(&self, time: Duration) -> Option<u64> {
        // A float to an integer saturates; a byte offset past the file is clamped by the callers.
        self.rate.map(|rate| (time.as_secs_f64() * rate) as u64)
    }

    /// The playback point's estimate before any anchor: size × position / duration (or the bitrate's), else the file's start.
    fn estimate(&self) -> u64 {
        self.bytes_for(self.position).unwrap_or(0).min(self.size)
    }

    /// The playback point's byte offset (ADR 0026, "The playback point").
    pub(crate) fn point(&self) -> u64 {
        let point = match self.anchor {
            Some(anchor) => {
                let since = self.position.saturating_sub(anchor.position);
                anchor
                    .offset
                    .saturating_add(self.bytes_for(since).unwrap_or(0))
            }
            None => self.estimate(),
        };
        point.min(self.size)
    }

    /// The first byte no window may fetch or serve (index windows excepted): 150 MiB past the point by default.
    fn limit(&self) -> u64 {
        self.point().saturating_add(self.limits.ahead)
    }

    fn allowed_end(&self, window: &Window) -> u64 {
        let limit = self.limit().max(window.released_to);
        if window.index {
            limit.max(window.start.saturating_add(self.limits.index_window))
        } else {
            limit
        }
    }

    /// Whether `offset` is in the file's index area: its last [`ReaderLimits::index_window`] bytes, in a file more than twice
    /// that long (a smaller file has no part read apart from its playback).
    fn is_index(&self, offset: u64) -> bool {
        let window = self.limits.index_window;
        self.size > window.saturating_mul(2) && offset >= self.size - window
    }

    /// The seek count a read stamps when it begins.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn held_ahead(&self) -> u64 {
        self.windows.iter().map(Window::held_ahead).sum()
    }

    fn held_behind(&self) -> u64 {
        self.windows.iter().map(Window::held_behind).sum()
    }

    /// The window that prefetches: the latest-starting one, not an index window, that starts at most the gap past the point and
    /// ends at most the read-ahead before it. A window left further behind (the file's head read at a resume) does not fetch its way
    /// up to the point.
    fn playback_window(&self) -> Option<u64> {
        let point = self.point();
        self.windows
            .iter()
            .filter(|window| {
                !window.index
                    && window.start <= point.saturating_add(self.limits.gap)
                    && window.end.saturating_add(self.limits.ahead) >= point
            })
            .max_by_key(|window| window.start)
            .map(|window| window.id)
    }

    pub(crate) fn window_mut(&mut self, id: u64) -> Option<&mut Window> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    fn window(&self, id: u64) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    /// The window that serves `offset`: one that holds it or reaches it within the gap, the latest-starting if several.
    fn find(&self, offset: u64) -> Option<usize> {
        self.windows
            .iter()
            .enumerate()
            .filter(|(_, window)| {
                window.start <= offset && offset <= window.end.saturating_add(self.limits.gap)
            })
            .max_by_key(|(_, window)| window.start)
            .map(|(at, _)| at)
    }

    /// Opens a window at `offset`, dropping the least recently read one when there are already as many as allowed.
    pub(crate) fn open(&mut self, offset: u64) -> u64 {
        if self.windows.len() >= self.limits.max_windows.max(1)
            && let Some(at) = self
                .windows
                .iter()
                .enumerate()
                .min_by_key(|(_, window)| window.last_used)
                .map(|(at, _)| at)
        {
            self.windows.remove(at);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.clock += 1;
        let index = self.is_index(offset);
        self.windows.push(Window {
            id,
            start: offset,
            chunks: VecDeque::new(),
            end: offset,
            cursor: offset,
            last_used: self.clock,
            index,
            status: WindowStatus::Fetching,
            waiting: 0,
            released_to: 0,
            pump: None,
        });
        id
    }

    /// Pins the point to a read after a start or seek, if the read can be the playback position (ADR 0026, "As built (R1)"): it
    /// began after the last seek (`generation`), is not an index read, is not a read of the file's head while the estimate is past
    /// it, and lies within the read-ahead either side of the estimate. Without any rate the first such read pins it wherever it is.
    /// A scan opens later, and at real sizes further than the read-ahead past the estimate.
    fn maybe_anchor(&mut self, offset: u64, generation: u64) {
        if !self.awaiting_anchor || generation != self.generation || self.is_index(offset) {
            return;
        }
        if self.rate.is_some() {
            let estimate = self.estimate();
            let ahead = self.limits.ahead;
            let head = offset < self.limits.gap && estimate >= self.limits.gap;
            let far = offset.abs_diff(estimate) > ahead;
            if head || far {
                return;
            }
        }
        self.anchor = Some(Anchor {
            offset,
            position: self.position,
            without_rate: self.rate.is_none(),
            released: false,
        });
        self.awaiting_anchor = false;
    }

    /// One step of a read of up to `max_len` bytes at `offset`.
    /// `generation` is [`State::generation`] when the read began.
    pub(crate) fn read(
        &mut self,
        offset: u64,
        max_len: u64,
        generation: u64,
        now: Instant,
    ) -> Step {
        if offset >= self.size || max_len == 0 {
            return Step::Eof;
        }
        self.maybe_anchor(offset, generation);
        let Some(at) = self.find(offset) else {
            return Step::Opened(self.open(offset));
        };
        self.clock += 1;
        let clock = self.clock;
        let window = &mut self.windows[at];
        window.last_used = clock;
        let id = window.id;
        let mut allowed = self.allowed_end(&self.windows[at]);
        if offset >= allowed {
            if !self.starving {
                return Step::Wait {
                    window: id,
                    missing: false,
                };
            }
            // The starvation guard: the player waits for this byte. A read begun since the last seek, within the read-ahead of the
            // estimate (or anywhere without a rate), says the point is at least here. Any other is only served, so it cannot pin
            // the point away from the target: one begun before the seek (held at the old limit), or one far from the estimate, such
            // as an MP4's index past the index window that the player reads while it opens.
            let near = self.rate.is_none() || offset.abs_diff(self.estimate()) <= self.limits.ahead;
            if generation == self.generation && near {
                self.anchor = Some(Anchor {
                    offset,
                    position: self.position,
                    without_rate: self.rate.is_none(),
                    released: true,
                });
                self.awaiting_anchor = false;
            } else {
                let window = &mut self.windows[at];
                window.released_to = window.released_to.max(offset.saturating_add(max_len));
            }
            self.releases += 1;
            allowed = self.allowed_end(&self.windows[at]);
        }
        let window = &mut self.windows[at];
        if offset < window.end {
            let len = max_len.min(window.end - offset).min(allowed - offset);
            let bytes = window.slice(offset, len);
            let served_end = offset + bytes.len() as u64;
            window.cursor = window.cursor.max(served_end);
            self.note_served(bytes.len() as u64, now);
            self.trim_behind();
            return Step::Data(bytes);
        }
        match window.status {
            WindowStatus::Failed => Step::Failed,
            WindowStatus::Done => Step::Eof,
            WindowStatus::Fetching => Step::Wait {
                window: id,
                missing: true,
            },
        }
    }

    /// What a window's pump may do: fetch on demand (a read waits for its bytes) up to the limit, or prefetch as the playback
    /// window up to the limit and within the read-ahead budget summed over the windows.
    pub(crate) fn fetch(&self, id: u64) -> Fetch {
        if self.closed.is_some() {
            return Fetch::Stop;
        }
        let Some(window) = self.window(id) else {
            return Fetch::Stop;
        };
        if window.status != WindowStatus::Fetching || window.end >= self.size {
            return Fetch::Stop;
        }
        let positional = self.allowed_end(window).saturating_sub(window.end);
        let room = if window.waiting > 0 {
            positional
        } else if self.playback_window() == Some(id) {
            positional.min(self.limits.ahead.saturating_sub(self.held_ahead()))
        } else {
            0
        };
        if room == 0 {
            Fetch::Wait
        } else {
            Fetch::Room {
                from: window.end,
                room,
            }
        }
    }

    /// Appends bytes a pump fetched at the window's end; false when the window is gone.
    pub(crate) fn append(&mut self, id: u64, bytes: Bytes) -> bool {
        let size = self.size;
        let Some(window) = self.window_mut(id) else {
            return false;
        };
        window.end += bytes.len() as u64;
        if !bytes.is_empty() {
            window.chunks.push_back(bytes);
        }
        if window.end >= size {
            window.status = WindowStatus::Done;
        }
        self.trim_behind();
        true
    }

    /// Keeps the bytes behind the read positions within the back budget: [`ReaderLimits::behind`] plus the unused part of
    /// [`ReaderLimits::ahead`] (mpv's donation), dropping from the least recently read window first.
    fn trim_behind(&mut self) {
        let budget = self
            .limits
            .behind
            .saturating_add(self.limits.ahead.saturating_sub(self.held_ahead()));
        let mut excess = self.held_behind().saturating_sub(budget);
        while excess > 0 {
            let Some(window) = self
                .windows
                .iter_mut()
                .filter(|window| window.held_behind() > 0)
                .min_by_key(|window| window.last_used)
            else {
                return;
            };
            let take = excess.min(window.held_behind());
            window.drop_front(take);
            excess -= take;
        }
    }

    fn note_served(&mut self, count: u64, now: Instant) {
        self.served.push_back((now, count));
        while self
            .served
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > RATE_SPAN)
        {
            self.served.pop_front();
        }
    }

    /// The file's duration, once known: the point then follows the position at size / duration (it replaces a bitrate's rate).
    /// An anchor pinned while no rate was known is checked against the new estimate as a read would be: kept if it passes, else
    /// dropped and pinned again by the next read that can be the playback position (a platform player reads the file's head before
    /// it can report the duration).
    pub(crate) fn set_duration(&mut self, duration: Duration) {
        if duration.is_zero() {
            return;
        }
        let rate = self.size as f64 / duration.as_secs_f64();
        if !rate.is_finite() || rate <= 0.0 {
            return;
        }
        let was_known = self.rate.is_some();
        self.rate = Some(rate);
        let Some(anchor) = self
            .anchor
            .filter(|anchor| anchor.without_rate && !was_known)
        else {
            return;
        };
        // Kept when the estimate accepts it (the player's read at the position, pinned after a seek); dropped, and pinned again by
        // the next read that can be the playback position, when it rejects it as it would a read: the file's head while the
        // estimate is past it, or further than the read-ahead from it.
        let estimate = self.bytes_for(anchor.position).unwrap_or(0).min(self.size);
        let head = anchor.offset < self.limits.gap && estimate >= self.limits.gap;
        let far = anchor.offset.abs_diff(estimate) > self.limits.ahead;
        if head || far {
            self.anchor = None;
            self.awaiting_anchor = true;
        } else {
            self.anchor = Some(Anchor {
                without_rate: false,
                ..anchor
            });
        }
    }

    pub(crate) fn set_position(&mut self, position: Duration) {
        self.position = position;
    }

    pub(crate) fn seek_to(&mut self, position: Duration) {
        self.position = position;
        self.anchor = None;
        self.awaiting_anchor = true;
        self.awaiting_frame = true;
        self.generation += 1;
    }

    /// The player's first frame after the start or a seek. A point a starvation release pinned while the player opened, or one the
    /// file's head pinned while no rate was known (the player reads the head first, wherever playback starts), need not be where
    /// playback is: it is pinned again, as after a seek, by the next read that can be the playback position. Kept: a point a read
    /// pinned with a rate known (within the read-ahead of the estimate), and one a read pinned past the head without a rate (the
    /// player's read at the seek target; without a rate nothing tells it from any other read, so the first one stands).
    ///
    /// The engine's first frame does not say which seek it ends: a late one of an earlier seek can come after the next
    /// [`State::seek_to`]. While nothing has pinned the point since the seek there is nothing to judge, so the check waits for the
    /// next first frame instead of being used up by the late one. Counting seeks against first frames was not chosen: an engine
    /// that merges seeks into one first frame (whether Windows' player does is not verified) would leave the count behind for good.
    pub(crate) fn restarted(&mut self) {
        if !self.awaiting_frame {
            return;
        }
        let Some(anchor) = self.anchor else {
            // Nothing pinned since the start or the seek.
            return;
        };
        self.awaiting_frame = false;
        if anchor.released || (anchor.without_rate && anchor.offset < self.limits.gap) {
            self.anchor = None;
            self.awaiting_anchor = true;
            self.generation += 1;
        }
    }

    pub(crate) fn set_starving(&mut self, starving: bool) {
        self.starving = starving;
    }

    pub(crate) fn facts(&self, now: Instant) -> ReaderFacts {
        let point = self.point();
        let served_ahead = self
            .windows
            .iter()
            .filter(|window| !window.index && window.start <= point)
            .map(|window| window.cursor.saturating_sub(point))
            .max()
            .unwrap_or(0);
        let recent: u64 = self
            .served
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= RATE_SPAN)
            .map(|(_, count)| count)
            .sum();
        ReaderFacts {
            size: self.size,
            point,
            rate_known: self.rate.is_some(),
            held_ahead: self.held_ahead(),
            served_ahead,
            served_ahead_time: self
                .rate
                .and_then(|rate| Duration::try_from_secs_f64(served_ahead as f64 / rate).ok()),
            read_rate: recent / RATE_SPAN.as_secs(),
            requests: self.requests,
            reconnects: self.reconnects,
            starvation_releases: self.releases,
            windows: self.windows.len(),
        }
    }
}
