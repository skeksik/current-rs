//! Observable list: the core owns the `Vec`, the UI mirrors it by applying diffs in order.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::TaskHandle;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Item {
    pub id: u64,
    pub title: String,
}

/// One change to the list. Indices refer to the list as it is when the diff is applied, after the diffs before it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ItemDiff {
    Append {
        values: Vec<Item>,
    },
    Insert {
        index: u32,
        value: Item,
    },
    Set {
        index: u32,
        value: Item,
    },
    Remove {
        index: u32,
    },
    /// Replace everything: the first message of every subscription, and the recovery after the UI fell behind.
    Reset {
        values: Vec<Item>,
    },
}

/// Implemented by the UI. Called on a core worker thread with diffs to apply in order, never an empty batch.
#[uniffi::export(callback_interface)]
pub trait ItemsListener: Send + Sync {
    fn on_diffs(&self, diffs: Vec<ItemDiff>);
}

/// How many unread diffs a subscriber may fall behind before it gets a `Reset` instead.
const BACKLOG: usize = 256;

pub(crate) struct ItemsState {
    shared: Arc<Shared>,
}

struct Shared {
    list: Mutex<List>,
    tx: broadcast::Sender<ItemDiff>,
}

struct List {
    items: Vec<Item>,
    next_id: u64,
    step: u64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, List> {
        // A poisoned lock holds a list some diff may not have reached; acceptable in a spike, the real code resets.
        self.list.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The snapshot and the receiver are taken under the same lock as every change, so no diff is lost or doubled.
    fn snapshot_and_subscribe(&self) -> (Vec<Item>, broadcast::Receiver<ItemDiff>) {
        let list = self.lock();
        (list.items.clone(), self.tx.subscribe())
    }

    fn step(&self) {
        let mut list = self.lock();
        let diff = list.next_diff();
        list.apply(&diff);
        let _ = self.tx.send(diff); // No subscriber is fine.
    }
}

impl List {
    fn item(&mut self) -> Item {
        self.next_id += 1;
        Item {
            id: self.next_id,
            title: format!("Item {}", self.next_id),
        }
    }

    /// A fixed script that exercises every kind of diff and keeps the list short.
    fn next_diff(&mut self) -> ItemDiff {
        let len = self.items.len() as u32;
        let step = self.step;
        self.step += 1;
        match step % 5 {
            0 => ItemDiff::Append {
                values: vec![self.item(), self.item()],
            },
            1 => ItemDiff::Insert {
                index: 0,
                value: self.item(),
            },
            2 if len > 0 => {
                let mut value = self.items[(len / 2) as usize].clone();
                value.title = format!("{} (changed)", value.title);
                ItemDiff::Set {
                    index: len / 2,
                    value,
                }
            }
            3 if len > 0 => ItemDiff::Remove { index: len - 1 },
            4 if len >= 10 => ItemDiff::Reset {
                values: vec![self.item(), self.item(), self.item()],
            },
            _ => ItemDiff::Append {
                values: vec![self.item()],
            },
        }
    }

    fn apply(&mut self, diff: &ItemDiff) {
        apply(&mut self.items, diff.clone());
    }
}

/// What every UI does with a diff, written once here so the tests can check a mirror against the core.
pub(crate) fn apply(items: &mut Vec<Item>, diff: ItemDiff) {
    match diff {
        ItemDiff::Append { values } => items.extend(values),
        ItemDiff::Insert { index, value } => items.insert(index as usize, value),
        ItemDiff::Set { index, value } => items[index as usize] = value,
        ItemDiff::Remove { index } => {
            items.remove(index as usize);
        }
        ItemDiff::Reset { values } => *items = values,
    }
}

impl ItemsState {
    pub(crate) fn new() -> Self {
        Self::with_backlog(BACKLOG)
    }

    fn with_backlog(backlog: usize) -> Self {
        let (tx, _) = broadcast::channel(backlog);
        let list = Mutex::new(List {
            items: Vec::new(),
            next_id: 0,
            step: 0,
        });
        Self {
            shared: Arc::new(Shared { list, tx }),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<Item> {
        self.shared.lock().items.clone()
    }

    pub(crate) fn step(&self) {
        self.shared.step();
    }

    /// The task owns the listener and sends whatever diffs are waiting as one batch.
    pub(crate) fn subscribe(
        &self,
        rt: &tokio::runtime::Handle,
        listener: Box<dyn ItemsListener>,
    ) -> Arc<TaskHandle> {
        let shared = self.shared.clone();
        let task = rt.spawn(async move {
            let (values, mut rx) = shared.snapshot_and_subscribe();
            listener.on_diffs(vec![ItemDiff::Reset { values }]);
            loop {
                let mut batch = match rx.recv().await {
                    Ok(diff) => vec![diff],
                    Err(RecvError::Lagged(_)) => {
                        let (values, fresh) = shared.snapshot_and_subscribe();
                        rx = fresh;
                        vec![ItemDiff::Reset { values }]
                    }
                    Err(RecvError::Closed) => break,
                };
                loop {
                    match rx.try_recv() {
                        Ok(diff) => batch.push(diff),
                        Err(TryRecvError::Lagged(_)) => {
                            let (values, fresh) = shared.snapshot_and_subscribe();
                            rx = fresh;
                            batch = vec![ItemDiff::Reset { values }];
                        }
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
                listener.on_diffs(batch);
            }
        });
        Arc::new(TaskHandle::new(task))
    }

    pub(crate) fn start_feed(
        &self,
        rt: &tokio::runtime::Handle,
        interval_ms: u64,
    ) -> Arc<TaskHandle> {
        let shared = self.shared.clone();
        let period = Duration::from_millis(interval_ms.max(1));
        let task = rt.spawn(async move {
            let mut tick = tokio::time::interval(period);
            loop {
                tick.tick().await;
                shared.step();
            }
        });
        Arc::new(TaskHandle::new(task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Alive, AliveToken, Recorder, wait_until};

    /// Mirrors the list the way a UI does.
    struct Mirror {
        items: Arc<Mutex<Vec<Item>>>,
        batches: Recorder<Vec<ItemDiff>>,
        gate: Option<Arc<std::sync::Barrier>>,
        entered: Arc<std::sync::atomic::AtomicBool>,
        _alive: AliveToken,
    }

    impl ItemsListener for Mirror {
        fn on_diffs(&self, diffs: Vec<ItemDiff>) {
            assert!(!diffs.is_empty());
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.wait();
            }
            let mut items = self.items.lock().unwrap();
            for diff in diffs.iter().cloned() {
                apply(&mut items, diff);
            }
            self.batches.push(diffs);
        }
    }

    type Parts = (Mirror, Arc<Mutex<Vec<Item>>>, Recorder<Vec<ItemDiff>>);

    fn mirror(alive: &Alive, gate: Option<Arc<std::sync::Barrier>>) -> Parts {
        let items = Arc::new(Mutex::new(Vec::new()));
        let batches = Recorder::new();
        let m = Mirror {
            items: items.clone(),
            batches: Recorder(batches.0.clone()),
            gate,
            entered: Default::default(),
            _alive: alive.token(),
        };
        (m, items, batches)
    }

    fn rt() -> tokio::runtime::Handle {
        crate::runtime::handle().unwrap()
    }

    #[test]
    fn script_uses_every_kind_of_diff() {
        let state = ItemsState::new();
        let mut kinds = std::collections::HashSet::new();
        let mut list = state.shared.lock();
        for _ in 0..40 {
            let diff = list.next_diff();
            kinds.insert(std::mem::discriminant(&diff));
            list.apply(&diff);
        }
        assert_eq!(kinds.len(), 5);
    }

    #[test]
    fn starts_with_reset_of_the_current_list() {
        let state = ItemsState::new();
        state.step();
        let alive = Alive::default();
        let (m, _items, batches) = mirror(&alive, None);
        let handle = state.subscribe(&rt(), Box::new(m));
        wait_until("first batch", || !batches.take().is_empty());
        assert_eq!(
            batches.take()[0],
            vec![ItemDiff::Reset {
                values: state.snapshot()
            }]
        );
        handle.cancel();
    }

    #[test]
    fn mirror_matches_the_core_after_many_changes() {
        let state = ItemsState::new();
        let alive = Alive::default();
        let (m, items, _batches) = mirror(&alive, None);
        let handle = state.subscribe(&rt(), Box::new(m));
        for _ in 0..500 {
            state.step();
        }
        wait_until("mirror in sync", || {
            *items.lock().unwrap() == state.snapshot()
        });
        handle.cancel();
        wait_until("listener dropped", || alive.count() == 0);
    }

    #[test]
    fn a_listener_that_falls_behind_gets_a_reset() {
        let state = ItemsState::with_backlog(4);
        let alive = Alive::default();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let (m, items, batches) = mirror(&alive, Some(gate.clone()));
        let entered = m.entered.clone();
        let handle = state.subscribe(&rt(), Box::new(m));
        wait_until("first callback", || {
            entered.load(std::sync::atomic::Ordering::SeqCst)
        });
        // The listener is stuck in its first callback while the core moves on by more than the backlog.
        for _ in 0..50 {
            state.step();
        }
        gate.wait(); // releases the first batch
        gate.wait(); // releases the recovery batch
        wait_until("mirror in sync", || {
            *items.lock().unwrap() == state.snapshot()
        });
        let second = &batches.take()[1];
        assert!(
            matches!(second[0], ItemDiff::Reset { .. }),
            "recovery starts with a reset: {second:?}"
        );
        handle.cancel();
    }

    #[test]
    fn feed_drives_changes_from_a_core_task() {
        let core = crate::Core::new().unwrap();
        let alive = Alive::default();
        let (m, items, _batches) = mirror(&alive, None);
        let handle = core.subscribe_items(Box::new(m));
        core.start_feed(1);
        wait_until("some changes", || items.lock().unwrap().len() >= 3);
        core.stop_feed();
        std::thread::sleep(Duration::from_millis(20));
        wait_until("mirror in sync", || *items.lock().unwrap() == core.items());
        drop(handle);
        wait_until("listener dropped", || alive.count() == 0);
    }
}
