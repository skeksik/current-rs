//! Observable lists: a list the core owns, sent to subscribers as batches of diffs (ADR 0004).
//!
//! Every change is applied and broadcast under one lock, and a subscription takes its snapshot and its receiver under the same lock,
//! so no diff is lost or doubled. A subscription starts with `Reset(snapshot)`; one that falls behind the broadcast backlog gets a
//! fresh `Reset` instead of an error.

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

/// How many diffs a slow subscriber may fall behind before it is reset.
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
    tx: broadcast::Sender<VectorDiff<T>>,
}

impl<T: Clone + Send + 'static> ObservableList<T> {
    pub fn new() -> Arc<Self> {
        Self::with_backlog(DEFAULT_BACKLOG)
    }

    /// A list whose subscribers are reset after falling `backlog` diffs behind (at least one).
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
        }
    }

    fn change(&self, diff: VectorDiff<T>) -> Result<(), OutOfRange> {
        let mut inner = self.lock();
        diff.clone().apply(&mut inner.values)?;
        // An error means no one is subscribed, which is fine.
        let _ = inner.tx.send(diff);
        Ok(())
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
    rx: broadcast::Receiver<VectorDiff<T>>,
    first: Option<Vec<T>>,
}

impl<T: Clone + Send + 'static> ListSubscription<T> {
    /// The next batch: `[Reset]` first, then every diff waiting, at least one. A subscriber that fell behind gets `[Reset]` again.
    /// `None` once the list is dropped.
    pub async fn next_batch(&mut self) -> Option<Vec<VectorDiff<T>>> {
        if let Some(values) = self.first.take() {
            return Some(vec![VectorDiff::Reset { values }]);
        }
        let first = match self.rx.recv().await {
            Ok(diff) => diff,
            Err(RecvError::Lagged(_)) => return self.resync(),
            Err(RecvError::Closed) => return None,
        };
        let mut batch = vec![first];
        loop {
            match self.rx.try_recv() {
                Ok(diff) => batch.push(diff),
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
