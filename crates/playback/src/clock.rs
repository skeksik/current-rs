//! The controller's time: a monotonic clock, so tests drive it by hand and no test waits for real time.

use std::time::{Duration, Instant};

/// A monotonic clock: the time since an arbitrary fixed start.
pub trait Clock {
    fn now(&self) -> Duration;
}

/// The system's monotonic clock, counted from its creation.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}
