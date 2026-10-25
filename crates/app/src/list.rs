//! Observable lists: a list the core owns, sent to subscribers as batches of diffs (ADR 0004).
//!
//! Every change is applied and broadcast under one lock, as one message however many diffs it holds, and a subscription takes its
//! snapshot and its receiver under the same lock, so no diff is lost or doubled. A subscription starts with `Reset(snapshot)`; one that falls behind the broadcast backlog gets a
//! fresh `Reset` instead of an error.

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

/// How many changes a slow subscriber may fall behind before it is reset. A change of many diffs ([`ObservableList::change_many`])
/// counts once, so no single change resets a subscriber that keeps up.
pub const DEFAULT_BACKLOG: usize = 256;

/// One change to a list. Indices refer to the list after the diffs before it in the same batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorDiff<T> {
    Append { values: Vec<T> },
    Insert { index: usize, value: T },
    Set { index: usize, value: T },
    Remove { index: usize },
    Reset { values: Vec<T> },
}

impl<T> VectorDiff<T> {
    /// Applies the diff to a mirror of the list.
    ///
    /// # Errors
    /// [`OutOfRange`] if the index does not exist in `list` (for `Insert`, if it is past the end); `list` is then unchanged.
    pub fn apply(self, list: &mut Vec<T>) -> Result<(), OutOfRange> {
        let len = list.len();
        let check = |index: usize, limit: usize| {
            if index < limit {
                Ok(())
            } else {
                Err(OutOfRange { index, len })
            }
        };
        match self {
            Self::Append { values } => list.extend(values),
            Self::Insert { index, value } => {
                check(index, len + 1)?;
                list.insert(index, value);
            }
            Self::Set { index, value } => {
                check(index, len)?;
                list[index] = value;
            }
            Self::Remove { index } => {
                check(index, len)?;
                list.remove(index);
            }
            Self::Reset { values } => *list = values,
        }
        Ok(())
    }
}

/// An index that does not exist in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("index {index} is out of range for a list of {len}")]
pub struct OutOfRange {
    pub index: usize,
    pub len: usize,
}

/// A list the core owns and UIs observe.
#[derive(Debug)]
pub struct ObservableList<T> {
    inner: Mutex<Inner<T>>,
}

#[derive(Debug)]
struct Inner<T> {
    values: Vec<T>,
    tx: broadcast::Sender<Change<T>>,
}

/// One change as broadcast: its diffs, in order.
type Change<T> = Vec<VectorDiff<T>>;

impl<T: Clone + Send + 'static> ObservableList<T> {
    pub fn new() -> Arc<Self> {
        Self::with_backlog(DEFAULT_BACKLOG)
    }

    /// A list whose subscribers are reset after falling `backlog` changes behind (at least one).
    pub fn with_backlog(backlog: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                values: Vec::new(),
                tx: broadcast::Sender::new(backlog.max(1)),
            }),
        })
    }

    /// The current values.
    pub fn snapshot(&self) -> Vec<T> {
        self.lock().values.clone()
    }

    pub fn len(&self) -> usize {
        self.lock().values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().values.is_empty()
    }

    /// The value at `index`, if it exists.
    pub fn get(&self, index: usize) -> Option<T> {
        self.lock().values.get(index).cloned()
    }

    pub fn append(&self, values: Vec<T>) {
        if !values.is_empty() {
            // Cannot fail: an append has no index.
            let _ = self.change(VectorDiff::Append { values });
        }
    }

    /// # Errors
    /// [`OutOfRange`] if `index` is past the end.
    pub fn insert(&self, index: usize, value: T) -> Result<(), OutOfRange> {
        self.change(VectorDiff::Insert { index, value })
    }

    /// # Errors
    /// [`OutOfRange`] if `index` does not exist.
    pub fn set(&self, index: usize, value: T) -> Result<(), OutOfRange> {
        self.change(VectorDiff::Set { index, value })
    }

    /// # Errors
    /// [`OutOfRange`] if `index` does not exist.
    pub fn remove(&self, index: usize) -> Result<(), OutOfRange> {
        self.change(VectorDiff::Remove { index })
    }

    pub fn reset(&self, values: Vec<T>) {
        // Cannot fail: a reset has no index.
        let _ = self.change(VectorDiff::Reset { values });
    }

    /// A subscription whose first batch is `[Reset(current values)]`. It holds the list weakly: it ends when the list is dropped.
    pub fn subscribe(self: &Arc<Self>) -> ListSubscription<T> {
        let inner = self.lock();
        ListSubscription {
            list: Arc::downgrade(self),
            rx: inner.tx.subscribe(),
            first: Some(inner.values.clone()),
            reset_requested: false,
        }
    }

    /// Applies `diffs` in order as one change: a subscriber receives them in one batch. Stops at the first diff whose index does
    /// not exist; the diffs before it stay applied and sent.
    ///
    /// # Errors
    /// [`OutOfRange`] for the first diff whose index does not exist.
    pub fn change_many(&self, diffs: Vec<VectorDiff<T>>) -> Result<(), OutOfRange> {
        let mut inner = self.lock();
        let mut applied = Vec::with_capacity(diffs.len());
        let mut result = Ok(());
        for diff in diffs {
            if let Err(error) = diff.clone().apply(&mut inner.values) {
                result = Err(error);
                break;
            }
            applied.push(diff);
        }
        if !applied.is_empty() {
            // An error means no one is subscribed, which is fine.
            let _ = inner.tx.send(applied);
        }
        result
    }

    fn change(&self, diff: VectorDiff<T>) -> Result<(), OutOfRange> {
        self.change_many(vec![diff])
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        // Each change is applied before it is sent and a failed apply leaves the list unchanged, so a panic while holding the lock
        // cannot leave the list and its subscribers out of step: a poisoned lock is safe.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One subscriber's view of an [`ObservableList`].
#[derive(Debug)]
pub struct ListSubscription<T> {
    list: Weak<ObservableList<T>>,
    rx: broadcast::Receiver<Change<T>>,
    first: Option<Vec<T>>,
    reset_requested: bool,
}

impl<T: Clone + Send + 'static> ListSubscription<T> {
    /// Makes the next batch a fresh `[Reset]`: for a subscriber that lost a batch (its listener failed) and so cannot apply diffs.
    pub fn request_reset(&mut self) {
        self.reset_requested = true;
    }

    /// The next batch: `[Reset]` first, then every diff waiting, at least one. A subscriber that fell behind gets `[Reset]` again.
    /// `None` once the list is dropped.
    pub async fn next_batch(&mut self) -> Option<Vec<VectorDiff<T>>> {
        if let Some(values) = self.first.take() {
            return Some(vec![VectorDiff::Reset { values }]);
        }
        if std::mem::take(&mut self.reset_requested) {
            return self.resync();
        }
        let first = match self.rx.recv().await {
            Ok(change) => change,
            Err(RecvError::Lagged(_)) => return self.resync(),
            Err(RecvError::Closed) => return None,
        };
        let mut batch = first;
        loop {
            match self.rx.try_recv() {
                Ok(change) => batch.extend(change),
                Err(TryRecvError::Lagged(_)) => return self.resync(),
                Err(TryRecvError::Empty | TryRecvError::Closed) => return Some(batch),
            }
        }
    }

    fn resync(&mut self) -> Option<Vec<VectorDiff<T>>> {
        let list = self.list.upgrade()?;
        let inner = list.lock();
        self.rx = inner.tx.subscribe();
        Some(vec![VectorDiff::Reset {
            values: inner.values.clone(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all<T>(mirror: &mut Vec<T>, batch: Vec<VectorDiff<T>>) {
        for diff in batch {
            diff.apply(mirror).unwrap();
        }
    }

    #[tokio::test]
    async fn starts_with_a_reset_then_sends_changes() {
        let list = ObservableList::new();
        list.append(vec![1, 2]);
        let mut sub = list.subscribe();
        assert_eq!(
            sub.next_batch().await,
            Some(vec![VectorDiff::Reset { values: vec![1, 2] }])
        );
        list.insert(0, 0).unwrap();
        list.remove(2).unwrap();
        assert_eq!(
            sub.next_batch().await,
            Some(vec![
                VectorDiff::Insert { index: 0, value: 0 },
                VectorDiff::Remove { index: 2 },
            ])
        );
    }

    #[test]
    fn a_bad_index_changes_nothing() {
        let list = ObservableList::new();
        list.append(vec!['a']);
        assert_eq!(list.insert(2, 'b'), Err(OutOfRange { index: 2, len: 1 }));
        assert_eq!(list.set(1, 'b'), Err(OutOfRange { index: 1, len: 1 }));
        assert_eq!(list.remove(1), Err(OutOfRange { index: 1, len: 1 }));
        assert_eq!(list.snapshot(), vec!['a']);
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_gets_a_reset() {
        let list = ObservableList::with_backlog(4);
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        for value in 0..10 {
            list.append(vec![value]);
        }
        assert_eq!(
            sub.next_batch().await,
            Some(vec![VectorDiff::Reset {
                values: (0..10).collect()
            }])
        );
        list.append(vec![10]);
        assert_eq!(
            sub.next_batch().await,
            Some(vec![VectorDiff::Append { values: vec![10] }])
        );
    }

    #[tokio::test]
    async fn a_requested_reset_comes_next() {
        let list = ObservableList::new();
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        list.append(vec![1]);
        sub.request_reset();
        assert_eq!(
            sub.next_batch().await,
            Some(vec![VectorDiff::Reset { values: vec![1] }])
        );
        // The diff sent before the reset is not sent again.
        list.append(vec![2]);
        assert_eq!(
            sub.next_batch().await,
            Some(vec![VectorDiff::Append { values: vec![2] }])
        );
    }

    #[tokio::test]
    async fn a_change_of_many_diffs_arrives_in_one_batch() {
        let list = ObservableList::new();
        list.append(vec![0, 0, 0]);
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        list.change_many(vec![
            VectorDiff::Set { index: 0, value: 1 },
            VectorDiff::Set { index: 2, value: 3 },
        ])
        .unwrap();
        assert_eq!(
            sub.next_batch().await,
            Some(vec![
                VectorDiff::Set { index: 0, value: 1 },
                VectorDiff::Set { index: 2, value: 3 },
            ])
        );
        assert_eq!(list.get(2), Some(3));
        assert_eq!(list.get(3), None);
        assert_eq!(
            list.change_many(vec![VectorDiff::Remove { index: 5 }]),
            Err(OutOfRange { index: 5, len: 3 })
        );
    }

    #[tokio::test]
    async fn a_change_larger_than_the_backlog_is_not_a_reset() {
        let list = ObservableList::with_backlog(4);
        list.append((0..100).collect());
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        list.change_many(
            (10..100)
                .rev()
                .map(|index| VectorDiff::Remove { index })
                .collect(),
        )
        .unwrap();
        let batch = sub.next_batch().await.unwrap();
        assert_eq!(batch.len(), 90);
        assert!(
            batch
                .iter()
                .all(|diff| matches!(diff, VectorDiff::Remove { .. }))
        );
    }

    #[tokio::test]
    async fn the_subscription_ends_with_the_list() {
        let list = ObservableList::<u8>::new();
        let mut sub = list.subscribe();
        sub.next_batch().await.unwrap();
        drop(list);
        assert_eq!(sub.next_batch().await, None);
    }

    #[tokio::test]
    async fn mirror_matches_the_list_after_many_changes() {
        let list = ObservableList::new();
        let mut sub = list.subscribe();
        let mut mirror = Vec::new();
        apply_all(&mut mirror, sub.next_batch().await.unwrap());
        // A fixed pseudo-random sequence of changes.
        let mut seed = 7_u32;
        for step in 0..500_u32 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let len = list.len();
            let at = (seed >> 8) as usize % (len + 1);
            match seed % 5 {
                0 => list.append(vec![step, step + 1]),
                1 => list.insert(at, step).unwrap(),
                2 if len > 0 => list.set(at % len, step).unwrap(),
                3 if len > 0 => list.remove(at % len).unwrap(),
                4 if step % 50 == 0 => list.reset(vec![step]),
                _ => list.append(vec![step]),
            }
            if step % 7 == 0 {
                apply_all(&mut mirror, sub.next_batch().await.unwrap());
            }
        }
        // Every change is sent: drop the list and drain until the subscription ends.
        let expected = list.snapshot();
        drop(list);
        while let Some(batch) = sub.next_batch().await {
            apply_all(&mut mirror, batch);
        }
        assert_eq!(mirror, expected);
    }
}
