//! D1 spike: how the core's state and actions cross the UniFFI bridge (docs/notes/uniffi-bridge.md).
//!
//! Throwaway prototype with neutral names. The pattern, not the types, is what the real `current-ffi` keeps:
//! - one global tokio runtime owned by the core ([`runtime`]);
//! - observable state: `subscribe_*` takes a listener (a callback interface the UI implements) and returns a [`TaskHandle`];
//! - lists travel as batches of [`ItemDiff`], starting with a `Reset`;
//! - async calls take an explicit [`CancelToken`]; abandoning the foreign await does not stop the work.

mod error;
mod list;
mod ops;
mod runtime;
mod task_handle;
mod value;

pub use error::BridgeError;
pub use list::{Item, ItemDiff, ItemsListener};
pub use ops::CancelToken;
pub use task_handle::TaskHandle;
pub use value::{Snapshot, ValueListener};

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

uniffi::setup_scaffolding!();

/// The object a UI holds: the state of one "screen" in the prototype.
#[derive(uniffi::Object)]
pub struct Core {
    rt: tokio::runtime::Handle,
    value: value::ValueState,
    items: list::ItemsState,
    ticker: std::sync::Mutex<Option<Arc<TaskHandle>>>,
    feed: std::sync::Mutex<Option<Arc<TaskHandle>>>,
    in_flight: Arc<AtomicU32>,
    listeners: Arc<AtomicU32>,
}

#[uniffi::export]
impl Core {
    /// Starts the global runtime on first use. Fails only if the OS refuses to create its threads.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, BridgeError> {
        Ok(Arc::new(Self {
            rt: runtime::handle()?,
            value: value::ValueState::new(),
            items: list::ItemsState::new(),
            ticker: std::sync::Mutex::new(None),
            feed: std::sync::Mutex::new(None),
            in_flight: Arc::new(AtomicU32::new(0)),
            listeners: Arc::new(AtomicU32::new(0)),
        }))
    }

    /// Current value, for a UI that renders before its first callback.
    pub fn value(&self) -> Snapshot {
        self.value.get()
    }

    /// Calls `listener.on_value` with the current value, then on every change, on a runtime worker thread,
    /// until the returned handle is cancelled or dropped.
    pub fn subscribe_value(&self, listener: Box<dyn ValueListener>) -> Arc<TaskHandle> {
        let held = Counted::hold(&self.listeners);
        self.value.subscribe(
            &self.rt,
            Box::new(Held {
                listener,
                _held: held,
            }),
        )
    }

    pub fn increment(&self) {
        self.value.increment(None);
    }

    /// Increments the value every `interval_ms` from a core task. Replaces a running ticker.
    pub fn start_ticker(&self, interval_ms: u64) {
        let handle = self.value.start_ticker(&self.rt, interval_ms);
        replace_task(&self.ticker, Some(handle));
    }

    pub fn stop_ticker(&self) {
        replace_task(&self.ticker, None);
    }

    pub fn items(&self) -> Vec<Item> {
        self.items.snapshot()
    }

    /// Calls `listener.on_diffs` with `[Reset(current)]`, then with each batch of changes, on a runtime worker thread.
    pub fn subscribe_items(&self, listener: Box<dyn ItemsListener>) -> Arc<TaskHandle> {
        let held = Counted::hold(&self.listeners);
        self.items.subscribe(
            &self.rt,
            Box::new(Held {
                listener,
                _held: held,
            }),
        )
    }

    /// Applies the next scripted change (append, insert, set, remove, reset in turn).
    pub fn feed_step(&self) {
        self.items.step();
    }

    /// Applies a scripted change every `interval_ms` from a core task. Replaces a running feed.
    pub fn start_feed(&self, interval_ms: u64) {
        let handle = self.items.start_feed(&self.rt, interval_ms);
        replace_task(&self.feed, Some(handle));
    }

    pub fn stop_feed(&self) {
        replace_task(&self.feed, None);
    }

    /// Waits `delay_ms`, then returns `text` reversed. Fails with `InvalidInput` for empty text and with `Cancelled`
    /// once `token` is cancelled. The work runs on the core's runtime; see [`ops`] for what happens when the
    /// foreign side stops awaiting.
    pub async fn slow_reverse(
        &self,
        text: String,
        delay_ms: u64,
        token: Arc<CancelToken>,
    ) -> Result<String, BridgeError> {
        ops::slow_reverse(&self.rt, self.in_flight.clone(), text, delay_ms, token).await
    }

    /// Number of `slow_reverse` calls whose Rust work is still running: shows abandoned awaits in the UI.
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Number of foreign listeners the core still holds: drops when a handle is cancelled or released, so the apps
    /// can show that unsubscribing really lets go of their object.
    pub fn live_listeners(&self) -> u32 {
        self.listeners.load(Ordering::SeqCst)
    }
}

/// Name of the calling thread, so the apps can show which thread a callback arrived on. A free function on purpose:
/// a listener that captured `Core` to call a method would keep the core alive (docs/notes/uniffi-bridge.md).
#[uniffi::export]
pub fn current_thread_name() -> String {
    runtime::thread_name()
}

impl Drop for Core {
    fn drop(&mut self) {
        replace_task(&self.ticker, None);
        replace_task(&self.feed, None);
    }
}

/// Counts something while it is alive (a listener the core holds, a call still running).
pub(crate) struct Counted(Arc<AtomicU32>);

impl Counted {
    pub(crate) fn hold(counter: &Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A foreign listener plus the count of it; dropping the task drops both.
struct Held<L: ?Sized> {
    listener: Box<L>,
    _held: Counted,
}

impl ValueListener for Held<dyn ValueListener> {
    fn on_value(&self, value: Snapshot) {
        self.listener.on_value(value);
    }
}

impl ItemsListener for Held<dyn ItemsListener> {
    fn on_diffs(&self, diffs: Vec<ItemDiff>) {
        self.listener.on_diffs(diffs);
    }
}

fn replace_task(slot: &std::sync::Mutex<Option<Arc<TaskHandle>>>, new: Option<Arc<TaskHandle>>) {
    // A poisoned lock only means another caller panicked mid-replace; the slot is still a valid Option.
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = std::mem::replace(&mut *slot, new) {
        old.cancel();
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Counts live listeners: proves the core releases a listener when its handle is cancelled or dropped.
    #[derive(Clone, Default)]
    pub struct Alive(Arc<AtomicUsize>);

    impl Alive {
        pub fn token(&self) -> AliveToken {
            self.0.fetch_add(1, Ordering::SeqCst);
            AliveToken(self.0.clone())
        }
        pub fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    pub struct AliveToken(Arc<AtomicUsize>);

    impl Drop for AliveToken {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Records every call; `wait_until` polls because callbacks arrive on runtime threads.
    pub struct Recorder<T>(pub Arc<Mutex<Vec<T>>>);

    impl<T: Clone> Recorder<T> {
        pub fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }
        pub fn push(&self, value: T) {
            self.0.lock().unwrap().push(value);
        }
        pub fn take(&self) -> Vec<T> {
            self.0.lock().unwrap().clone()
        }
    }

    pub fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }
}
