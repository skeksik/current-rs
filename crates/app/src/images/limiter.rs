//! At most six image fetches per server at a time, the newest waiting request first: the user scrolled to it (ADR 0018, section
//! 4).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

/// Fetches per server at a time.
pub(crate) const FETCHES_PER_SERVER: usize = 6;

#[derive(Default)]
struct State {
    running: usize,
    /// Oldest first; the newest is served first.
    waiting: Vec<oneshot::Sender<Permit>>,
}

#[derive(Default)]
pub(crate) struct Limiter {
    state: Mutex<State>,
}

/// One running fetch. Dropping it hands the slot to the newest waiter still waiting, or frees it.
pub(crate) struct Permit {
    limiter: Option<Arc<Limiter>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(limiter) = self.limiter.take() {
            limiter.release();
        }
    }
}

impl Limiter {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Each change is one push, pop or count: a poisoned lock is still consistent.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for a slot. Dropping the future gives up its place; a slot handed to it meanwhile goes on to the next waiter.
    pub(crate) async fn acquire(self: &Arc<Self>) -> Permit {
        loop {
            let receiver = {
                let mut state = self.lock();
                if state.running < FETCHES_PER_SERVER {
                    state.running += 1;
                    return Permit {
                        limiter: Some(self.clone()),
                    };
                }
                let (sender, receiver) = oneshot::channel();
                state.waiting.push(sender);
                receiver
            };
            // A sender is dropped unsent only with the limiter, which `self` keeps alive: the loop is a formality.
            if let Ok(permit) = receiver.await {
                return permit;
            }
        }
    }

    fn release(self: &Arc<Self>) {
        loop {
            let Some(next) = ({
                let mut state = self.lock();
                let next = state.waiting.pop();
                if next.is_none() {
                    state.running -= 1;
                }
                next
            }) else {
                return;
            };
            match next.send(Permit {
                limiter: Some(self.clone()),
            }) {
                Ok(()) => return,
                // That waiter gave up: disarm the permit and try the next one.
                Err(mut permit) => permit.limiter = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    fn poll<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    fn ready(future: impl Future<Output = Permit>) -> Permit {
        match poll(pin!(future)) {
            Poll::Ready(permit) => permit,
            Poll::Pending => panic!("waiting"),
        }
    }

    #[test]
    fn six_run_and_the_newest_waiter_goes_next() {
        let limiter = Arc::new(Limiter::default());
        let mut running: Vec<Permit> = (0..FETCHES_PER_SERVER)
            .map(|_| ready(limiter.acquire()))
            .collect();
        let mut older = pin!(limiter.acquire());
        let mut newer = pin!(limiter.acquire());
        assert!(poll(older.as_mut()).is_pending());
        assert!(poll(newer.as_mut()).is_pending());

        running.pop();
        assert!(poll(older.as_mut()).is_pending());
        let Poll::Ready(newest) = poll(newer.as_mut()) else {
            panic!("the newest waiter goes first");
        };
        running.pop();
        let Poll::Ready(oldest) = poll(older.as_mut()) else {
            panic!("then the older one");
        };
        assert_eq!(limiter.lock().running, FETCHES_PER_SERVER);
        drop((running, newest, oldest));
        assert_eq!(limiter.lock().running, 0);
    }

    #[test]
    fn a_waiter_that_gave_up_passes_its_slot_on() {
        let limiter = Arc::new(Limiter::default());
        let mut running: Vec<Permit> = (0..FETCHES_PER_SERVER)
            .map(|_| ready(limiter.acquire()))
            .collect();
        let mut older = pin!(limiter.acquire());
        assert!(poll(older.as_mut()).is_pending());
        {
            let mut gave_up = pin!(limiter.acquire());
            assert!(poll(gave_up.as_mut()).is_pending());
        }
        running.pop();
        let Poll::Ready(_older) = poll(older.as_mut()) else {
            panic!("the slot skips the waiter that gave up");
        };
        // Handed a slot, then dropped before taking it: the slot goes on, and with no one waiting it is freed.
        let mut handed = Box::pin(limiter.acquire());
        assert!(poll(handed.as_mut()).is_pending());
        running.pop();
        assert_eq!(limiter.lock().running, FETCHES_PER_SERVER);
        drop(handed);
        assert_eq!(limiter.lock().running, FETCHES_PER_SERVER - 1);
    }
}
