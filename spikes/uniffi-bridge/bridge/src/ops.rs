//! Async calls with explicit cancellation.
//!
//! What UniFFI 0.31 does with a foreign await (read in its generated code, see docs/notes/uniffi-bridge.md): Swift and
//! C# poll the Rust future until it completes and free it only then. Cancelling the Swift `Task` or abandoning the C#
//! `Task` does not reach Rust, so the work runs to its end. Hence the rule: every cancellable call takes a
//! [`CancelToken`], and the UI wrapper cancels it when its own task is cancelled. If the future is dropped instead
//! (Kotlin cancels by dropping it; so would Rust callers), the work is aborted too ([`AbortOnDrop`]).

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{BridgeError, Counted};

/// Created by the UI per call (or per group of calls) and cancelled from any thread.
#[derive(uniffi::Object)]
pub struct CancelToken {
    tx: watch::Sender<bool>,
}

#[uniffi::export]
impl CancelToken {
    #[uniffi::constructor]
    #[allow(clippy::new_without_default)] // UniFFI constructors are plain functions; Default is not exported.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tx: watch::channel(false).0,
        })
    }

    /// Idempotent. Every call waiting on this token ends with `BridgeError::Cancelled`.
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort(); // No-op once the task has finished.
    }
}

pub(crate) async fn slow_reverse(
    rt: &tokio::runtime::Handle,
    in_flight: Arc<AtomicU32>,
    text: String,
    delay_ms: u64,
    token: Arc<CancelToken>,
) -> Result<String, BridgeError> {
    if text.is_empty() {
        return Err(BridgeError::InvalidInput {
            reason: "text is empty".into(),
        });
    }
    let mut cancelled = token.tx.subscribe();
    if *cancelled.borrow_and_update() {
        return Err(BridgeError::Cancelled);
    }
    let guard = Counted::hold(&in_flight); // released when the work's future is dropped, finished or aborted
    // The work runs on the core's runtime: the future UniFFI polls only waits for it, so it needs no tokio context.
    let mut work = AbortOnDrop(rt.spawn(async move {
        let _guard = guard;
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(delay_ms)) => Ok(text.chars().rev().collect()),
            _ = cancelled.wait_for(|c| *c) => Err(BridgeError::Cancelled),
        }
    }));
    match (&mut work.0).await {
        Ok(result) => result,
        Err(e) => Err(BridgeError::Runtime {
            message: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Core;
    use crate::test_support::wait_until;

    fn block_on<F: Future>(f: F) -> F::Output {
        // A plain executor, as the foreign side is: not a tokio runtime thread.
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn completes() {
        let core = Core::new().unwrap();
        let r = block_on(core.slow_reverse("abc".into(), 1, CancelToken::new()));
        assert_eq!(r, Ok("cba".into()));
        assert_eq!(core.in_flight(), 0);
    }

    #[test]
    fn typed_error_for_bad_input() {
        let core = Core::new().unwrap();
        let r = block_on(core.slow_reverse(String::new(), 1, CancelToken::new()));
        assert!(matches!(r, Err(BridgeError::InvalidInput { .. })));
    }

    #[test]
    fn cancel_ends_the_call_early() {
        let core = Core::new().unwrap();
        let token = CancelToken::new();
        let t = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            t.cancel();
        });
        let started = std::time::Instant::now();
        let r = block_on(core.slow_reverse("abc".into(), 10_000, token));
        assert_eq!(r, Err(BridgeError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2));
        wait_until("work dropped", || core.in_flight() == 0);
    }

    #[test]
    fn already_cancelled_token_fails_at_once() {
        let core = Core::new().unwrap();
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
        assert_eq!(
            block_on(core.slow_reverse("abc".into(), 10_000, token)),
            Err(BridgeError::Cancelled)
        );
        assert_eq!(core.in_flight(), 0);
    }

    /// What Swift and C# do when their task is cancelled: keep the future, stop caring. The work keeps running.
    #[test]
    fn abandoned_await_keeps_running_until_its_token_is_cancelled() {
        let core = Core::new().unwrap();
        let token = CancelToken::new();
        let c = core.clone();
        let t = token.clone();
        let waiter = std::thread::spawn(move || block_on(c.slow_reverse("abc".into(), 10_000, t)));
        wait_until("work started", || core.in_flight() == 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            core.in_flight(),
            1,
            "nobody cancelled the token, so the work is still running"
        );
        token.cancel();
        assert_eq!(waiter.join().unwrap(), Err(BridgeError::Cancelled));
        wait_until("work dropped", || core.in_flight() == 0);
    }

    /// What Kotlin (and Rust callers) do: drop the future. The work is aborted with it.
    #[test]
    fn dropping_the_future_aborts_the_work() {
        let core = Core::new().unwrap();
        let r = block_on(async {
            let call = core.slow_reverse("abc".into(), 10_000, CancelToken::new());
            tokio::time::timeout(Duration::from_millis(30), call).await
        });
        assert!(r.is_err(), "timed out");
        wait_until("work aborted", || core.in_flight() == 0);
    }
}
