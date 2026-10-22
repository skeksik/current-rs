//! Tests for the bridge: delivery, threading, release of listeners, list diffs and cancellation.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use current_app::ObservableList;
use current_model::{Capabilities, ServerKey, ServerRef, ServerStatus, ServiceType};

use super::*;
use crate::diff::list_diff;

const WAIT: Duration = Duration::from_secs(5);

/// Records every value with the name of the thread it arrived on, and counts its own drops.
struct Recorder {
    tx: Mutex<Sender<Delivery>>,
    drops: Arc<AtomicUsize>,
}

impl CapabilitiesListener for Recorder {
    fn on_value(&self, value: CapabilitiesSnapshot) {
        let thread = thread::current().name().map(str::to_owned);
        let _ = self.tx.lock().unwrap().send((value, thread));
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

/// A value and the name of the thread it arrived on.
type Delivery = (CapabilitiesSnapshot, Option<String>);

fn recorder() -> (
    Box<dyn CapabilitiesListener>,
    Receiver<Delivery>,
    Arc<AtomicUsize>,
) {
    let (tx, rx) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let listener = Recorder {
        tx: Mutex::new(tx),
        drops: Arc::clone(&drops),
    };
    (Box::new(listener), rx, drops)
}

fn one_server(key: &str) -> Capabilities {
    Capabilities {
        servers: vec![ServerStatus::new(ServerRef::new(
            ServiceType::JELLYFIN,
            ServerKey::new(key),
        ))],
    }
}

/// Waits until `done` holds, failing the test after [`WAIT`].
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out waiting for: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Polls a future to completion on this thread, with no tokio context: as the foreign side does.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        thread::park_timeout(Duration::from_millis(50));
    }
}

#[test]
fn app_info_has_the_workspace_version() {
    assert_eq!(Core::new().unwrap().app_info().version, "0.1.0");
}

#[test]
fn sends_the_current_value_then_changes_on_a_worker_thread() {
    let core = Core::new().unwrap();
    let (listener, rx, _) = recorder();
    let _handle = core.subscribe_capabilities(listener);

    let (first, thread) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(first, CapabilitiesSnapshot::default());
    assert_eq!(thread.as_deref(), Some(WORKER_THREAD_NAME));

    core.app.set_capabilities(one_server("1"));
    let (next, thread) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(next.servers.len(), 1);
    assert_eq!(next.servers[0].service, "jellyfin");
    assert_eq!(next.servers[0].key, "1");
    assert_eq!(thread.as_deref(), Some(WORKER_THREAD_NAME));
}

#[test]
fn cancel_releases_the_listener() {
    let core = Core::new().unwrap();
    let (listener, rx, drops) = recorder();
    let handle = core.subscribe_capabilities(listener);
    rx.recv_timeout(WAIT).unwrap();
    assert_eq!(core.live_listeners(), 1);

    handle.cancel();
    eventually("the listener is dropped", || {
        drops.load(Ordering::SeqCst) == 1
    });
    assert_eq!(core.live_listeners(), 0);
    eventually("the task ends", || handle.is_finished());

    core.app.set_capabilities(one_server("2"));
    thread::sleep(Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "no delivery after cancel");
}

#[test]
fn dropping_the_handle_releases_the_listener() {
    let core = Core::new().unwrap();
    let (listener, rx, drops) = recorder();
    let handle = core.subscribe_capabilities(listener);
    rx.recv_timeout(WAIT).unwrap();

    drop(handle);
    eventually("the listener is dropped", || {
        drops.load(Ordering::SeqCst) == 1
    });
    assert_eq!(core.live_listeners(), 0);
}

#[test]
fn each_subscription_is_counted_and_released_on_its_own() {
    let core = Core::new().unwrap();
    let (first, first_rx, _) = recorder();
    let (second, second_rx, _) = recorder();
    let first = core.subscribe_capabilities(first);
    let _second = core.subscribe_capabilities(second);
    first_rx.recv_timeout(WAIT).unwrap();
    second_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(core.live_listeners(), 2);

    first.cancel();
    eventually("one listener is left", || core.live_listeners() == 1);
    core.app.set_capabilities(one_server("3"));
    assert_eq!(second_rx.recv_timeout(WAIT).unwrap().0.servers.len(), 1);
}

#[test]
fn a_slow_listener_skips_to_the_latest_value() {
    let core = Core::new().unwrap();
    let (tx, rx) = mpsc::channel();
    let gate = Arc::new(Mutex::new(()));
    let (entered_tx, entered) = mpsc::channel();
    struct Slow {
        tx: Mutex<Sender<CapabilitiesSnapshot>>,
        entered: Mutex<Sender<()>>,
        gate: Arc<Mutex<()>>,
    }
    impl CapabilitiesListener for Slow {
        fn on_value(&self, value: CapabilitiesSnapshot) {
            let _ = self.entered.lock().unwrap().send(());
            let _open = self.gate.lock().unwrap();
            let _ = self.tx.lock().unwrap().send(value);
        }
    }
    let closed = gate.lock().unwrap();
    let _handle = core.subscribe_capabilities(Box::new(Slow {
        tx: Mutex::new(tx),
        entered: Mutex::new(entered_tx),
        gate: Arc::clone(&gate),
    }));
    // The first delivery is stuck behind the gate while three changes happen.
    entered.recv_timeout(WAIT).unwrap();
    for key in ["a", "b", "c"] {
        core.app.set_capabilities(one_server(key));
    }
    drop(closed);

    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        CapabilitiesSnapshot::default()
    );
    assert_eq!(rx.recv_timeout(WAIT).unwrap().servers[0].key, "c");
    thread::sleep(Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "intermediate values are skipped");
}

list_diff!(
    /// A diff over plain strings, for the tests.
    TextDiff,
    String
);

#[test]
fn list_diffs_convert_from_the_core() {
    use current_app::VectorDiff;
    assert_eq!(
        TextDiff::from_core(VectorDiff::Insert {
            index: 3,
            value: String::from("x")
        }),
        TextDiff::Insert {
            index: 3,
            value: "x".into()
        }
    );
    assert_eq!(
        TextDiff::from_core(VectorDiff::<String>::Remove { index: usize::MAX }),
        TextDiff::Remove { index: u32::MAX }
    );
}

#[test]
fn a_list_subscription_starts_with_a_reset_and_mirrors_the_core() {
    let runtime = runtime::handle().unwrap();
    let list = ObservableList::new();
    list.append(vec![String::from("a")]);
    let (tx, rx) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let guard = DropCounter(Arc::clone(&drops));
    let handle = subscribe::watch_list(&runtime, list.subscribe(), move |batch| {
        let _ = &guard;
        let _ = tx.send(
            batch
                .into_iter()
                .map(TextDiff::from_core)
                .collect::<Vec<_>>(),
        );
    });
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        vec![TextDiff::Reset {
            values: vec!["a".into()]
        }]
    );

    let mut mirror = vec![String::from("a")];
    for step in 0..100_usize {
        match step % 4 {
            0 => list.insert(0, step.to_string()).unwrap(),
            1 => list.set(list.len() - 1, step.to_string()).unwrap(),
            2 => list.append(vec![step.to_string()]),
            _ => list.remove(list.len() / 2).unwrap(),
        }
    }
    let expected = list.snapshot();
    eventually("the mirror catches up", || {
        while let Ok(batch) = rx.try_recv() {
            for diff in batch {
                apply(&mut mirror, diff);
            }
        }
        mirror == expected
    });

    assert!(!handle.is_finished(), "running until cancelled");
    handle.cancel();
    eventually("the listener is dropped", || {
        drops.load(Ordering::SeqCst) == 1
    });
    eventually("the task ends", || handle.is_finished());
}

/// Counts its own drop: stands in for a foreign listener the core must release.
struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_failing_listener_keeps_its_subscription() {
    // UniFFI turns a foreign exception into a panic in the callback; this listener panics on its first call.
    struct FailsOnce {
        calls: AtomicUsize,
        tx: Mutex<Sender<CapabilitiesSnapshot>>,
    }
    impl CapabilitiesListener for FailsOnce {
        fn on_value(&self, value: CapabilitiesSnapshot) {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("the foreign callback threw");
            }
            let _ = self.tx.lock().unwrap().send(value);
        }
    }
    let core = Core::new().unwrap();
    let (tx, rx) = mpsc::channel();
    let listener = Arc::new(FailsOnce {
        calls: AtomicUsize::new(0),
        tx: Mutex::new(tx),
    });
    struct Shared(Arc<FailsOnce>);
    impl CapabilitiesListener for Shared {
        fn on_value(&self, value: CapabilitiesSnapshot) {
            self.0.on_value(value);
        }
    }
    let handle = core.subscribe_capabilities(Box::new(Shared(Arc::clone(&listener))));
    eventually("the first call fails", || {
        listener.calls.load(Ordering::SeqCst) == 1
    });

    core.app.set_capabilities(one_server("after"));
    assert_eq!(rx.recv_timeout(WAIT).unwrap().servers[0].key, "after");
    assert!(!handle.is_finished());
    assert_eq!(core.live_listeners(), 1);
}

#[test]
fn a_failing_list_listener_gets_a_reset_next() {
    let runtime = runtime::handle().unwrap();
    let list = ObservableList::new();
    let (tx, rx) = mpsc::channel();
    let calls = AtomicUsize::new(0);
    let _handle = subscribe::watch_list(&runtime, list.subscribe(), move |batch| {
        // The second batch (the first diffs) is lost.
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            panic!("the foreign callback threw");
        }
        let _ = tx.send(
            batch
                .into_iter()
                .map(TextDiff::from_core)
                .collect::<Vec<_>>(),
        );
    });
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        vec![TextDiff::Reset { values: vec![] }]
    );

    list.append(vec![String::from("lost")]);
    // Whether "next" shares the lost batch or comes after it, what arrives next is the whole list.
    list.append(vec![String::from("next")]);
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        vec![TextDiff::Reset {
            values: vec!["lost".into(), "next".into()]
        }]
    );
}

/// Applies a diff as a UI does.
fn apply(list: &mut Vec<String>, diff: TextDiff) {
    match diff {
        TextDiff::Append { values } => list.extend(values),
        TextDiff::Insert { index, value } => list.insert(index as usize, value),
        TextDiff::Set { index, value } => list[index as usize] = value,
        TextDiff::Remove { index } => {
            list.remove(index as usize);
        }
        TextDiff::Reset { values } => *list = values,
    }
}

#[test]
fn export_diagnostics_completes_without_a_tokio_context() {
    let core = Core::new().unwrap();
    let report = block_on(core.export_diagnostics(CancelToken::new())).unwrap();
    assert_eq!(report, "Current 0.1.0\n\nRequests, oldest first:\n");
}

#[test]
fn a_token_cancelled_first_wins() {
    let core = Core::new().unwrap();
    let token = CancelToken::new();
    token.cancel();
    assert!(token.is_cancelled());
    assert_eq!(
        block_on(core.export_diagnostics(token)),
        Err(CoreError::Cancelled)
    );
}

#[test]
fn cancel_ends_the_call_early() {
    let runtime = runtime::handle().unwrap();
    let token = CancelToken::new();
    let canceller = Arc::clone(&token);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        canceller.cancel();
    });
    let start = Instant::now();
    let result = block_on(cancel::run_cancellable(&runtime, token, async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    }));
    assert_eq!(result, Err(CoreError::Cancelled));
    assert!(start.elapsed() < WAIT);
}

#[test]
fn dropping_the_future_aborts_the_work() {
    struct SetOnDrop(Arc<AtomicBool>);
    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let runtime = runtime::handle().unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let call = {
        let (started, dropped, finished) = (
            Arc::clone(&started),
            Arc::clone(&dropped),
            Arc::clone(&finished),
        );
        cancel::run_cancellable(&runtime, CancelToken::new(), async move {
            let _guard = SetOnDrop(dropped);
            started.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(30)).await;
            finished.store(true, Ordering::SeqCst);
            Ok(())
        })
    };
    {
        let mut call = Box::pin(call);
        let waker = Waker::noop();
        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(waker))
                .is_pending()
        );
        eventually("the work starts", || started.load(Ordering::SeqCst));
    }
    eventually("the work is aborted", || dropped.load(Ordering::SeqCst));
    assert!(!finished.load(Ordering::SeqCst));
}

#[test]
fn errors_display_their_cause() {
    assert_eq!(CoreError::Cancelled.to_string(), "cancelled");
    assert_eq!(
        CoreError::Runtime {
            message: "no threads".into()
        }
        .to_string(),
        "runtime failure: no threads"
    );
}
