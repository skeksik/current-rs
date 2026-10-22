//! Observable value: a `tokio::sync::watch` channel in the core, a listener on the UI side.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::TaskHandle;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Snapshot {
    pub count: u64,
    /// Which thread changed the value last (a core worker for the ticker, the UI thread for a button).
    pub changed_on: String,
}

/// Implemented by the UI. Called on a core worker thread: the UI must hop to its own thread before touching views.
#[uniffi::export(callback_interface)]
pub trait ValueListener: Send + Sync {
    fn on_value(&self, value: Snapshot);
}

pub(crate) struct ValueState {
    tx: watch::Sender<Snapshot>,
}

impl ValueState {
    pub(crate) fn new() -> Self {
        let (tx, _) = watch::channel(Snapshot {
            count: 0,
            changed_on: String::new(),
        });
        Self { tx }
    }

    pub(crate) fn get(&self) -> Snapshot {
        self.tx.borrow().clone()
    }

    pub(crate) fn increment(&self, by_thread: Option<String>) {
        let changed_on = by_thread.unwrap_or_else(crate::runtime::thread_name);
        self.tx.send_modify(|s| {
            s.count += 1;
            s.changed_on = changed_on;
        });
    }

    /// The task owns the listener. Only the latest value is delivered: a slow UI skips values, never queues them.
    pub(crate) fn subscribe(
        &self,
        rt: &tokio::runtime::Handle,
        listener: Box<dyn ValueListener>,
    ) -> Arc<TaskHandle> {
        let mut rx = self.tx.subscribe();
        let task = rt.spawn(async move {
            loop {
                let value = rx.borrow_and_update().clone();
                listener.on_value(value);
                if rx.changed().await.is_err() {
                    break; // The state was dropped.
                }
            }
        });
        Arc::new(TaskHandle::new(task))
    }

    pub(crate) fn start_ticker(
        &self,
        rt: &tokio::runtime::Handle,
        interval_ms: u64,
    ) -> Arc<TaskHandle> {
        let tx = self.tx.clone();
        let period = Duration::from_millis(interval_ms.max(1));
        let task = rt.spawn(async move {
            let mut tick = tokio::time::interval(period);
            tick.tick().await; // The first tick is immediate.
            loop {
                tick.tick().await;
                let changed_on = crate::runtime::thread_name();
                tx.send_modify(|s| {
                    s.count += 1;
                    s.changed_on = changed_on;
                });
            }
        });
        Arc::new(TaskHandle::new(task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Alive, AliveToken, Recorder, wait_until};

    struct TestListener {
        seen: Recorder<(u64, String)>,
        _alive: AliveToken,
    }

    impl ValueListener for TestListener {
        fn on_value(&self, value: Snapshot) {
            self.seen.push((value.count, crate::runtime::thread_name()));
        }
    }

    fn listener(alive: &Alive) -> (Box<dyn ValueListener>, Recorder<(u64, String)>) {
        let seen = Recorder::new();
        let l = TestListener {
            seen: Recorder(seen.0.clone()),
            _alive: alive.token(),
        };
        (Box::new(l), seen)
    }

    #[test]
    fn sends_current_value_then_changes_on_a_worker_thread() {
        let core = crate::Core::new().unwrap();
        core.increment();
        let alive = Alive::default();
        let (l, seen) = listener(&alive);
        let handle = core.subscribe_value(l);
        wait_until("initial value", || seen.take().len() == 1);
        assert_eq!(seen.take()[0].0, 1);
        core.increment();
        wait_until("change", || seen.take().last().map(|v| v.0) == Some(2));
        assert!(
            seen.take()
                .iter()
                .all(|(_, thread)| thread == "core-worker")
        );
        handle.cancel();
    }

    #[test]
    fn cancel_releases_the_listener() {
        let core = crate::Core::new().unwrap();
        let alive = Alive::default();
        let (l, seen) = listener(&alive);
        let handle = core.subscribe_value(l);
        wait_until("initial value", || !seen.take().is_empty());
        assert_eq!(alive.count(), 1);
        assert_eq!(core.live_listeners(), 1);
        handle.cancel();
        wait_until("listener dropped", || alive.count() == 0);
        assert_eq!(core.live_listeners(), 0);
        assert!(handle.is_finished());
        core.increment();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(seen.take().len(), 1, "no callback after cancel");
    }

    #[test]
    fn dropping_the_handle_releases_the_listener() {
        let core = crate::Core::new().unwrap();
        let alive = Alive::default();
        let (l, _seen) = listener(&alive);
        drop(core.subscribe_value(l));
        wait_until("listener dropped", || alive.count() == 0);
    }

    #[test]
    fn ticker_changes_the_value_from_the_core_until_stopped() {
        let core = crate::Core::new().unwrap();
        core.start_ticker(5);
        wait_until("ticks", || core.value().count >= 3);
        assert_eq!(core.value().changed_on, "core-worker");
        core.stop_ticker();
        std::thread::sleep(Duration::from_millis(30));
        let stopped = core.value().count;
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(core.value().count, stopped);
    }
}
