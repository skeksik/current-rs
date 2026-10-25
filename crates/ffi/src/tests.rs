//! Tests for the bridge: delivery, threading, release of listeners, list diffs and cancellation.
//!
//! Every test in this binary shares the core's one runtime and its two worker threads (`runtime.rs`). Listener callbacks run on the
//! runtime's blocking pool, so a listener may block or panic; a test must still not block a worker itself, or every other test's
//! deliveries stall with it.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Once};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use current_app::ObservableList;
use current_model::{Capabilities, ServerKey, ServerRef, ServerStatus, ServiceType};

use super::*;
use crate::diff::list_diff;

pub(crate) const WAIT: Duration = Duration::from_secs(5);

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
pub(crate) fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out waiting for: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Polls a future to completion on this thread, with no tokio context: as the foreign side does.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
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
fn sends_the_current_value_then_changes_on_a_runtime_thread() {
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

/// Stands in for a foreign callback that threw: UniFFI turns the exception into a panic, which the bridge catches. A real `panic!`,
/// so the process's panic hook runs as it would for a foreign exception (with a backtrace where `RUST_BACKTRACE` is set): on a
/// blocking-pool thread, where it delays that one subscription only.
fn foreign_exception() -> ! {
    panic!("the foreign callback threw")
}

/// The payload of a panic the test hook makes slow.
const SLOW_HOOK_PANIC: &str = "a foreign exception with a slow panic hook";

/// How long the test hook takes for a [`SLOW_HOOK_PANIC`].
const SLOW_HOOK: Duration = Duration::from_secs(2);

/// Installs, once for the whole test binary, a panic hook that takes [`SLOW_HOOK`] for a [`SLOW_HOOK_PANIC`] and hands every other
/// panic to the hook that was there before. Installed once and never restored: tests run in parallel and would race on `set_hook`.
fn install_slow_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
            if message == Some(SLOW_HOOK_PANIC) {
                thread::sleep(SLOW_HOOK);
            } else {
                previous(info);
            }
        }));
    });
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
                foreign_exception();
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
            foreign_exception();
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
    list.append(vec![String::from("next")]);
    recovers_with_a_reset(&rx, &["lost", "next"]);
}

/// Checks what follows a lost batch: a batch starting with a Reset, then batches that bring a mirror to `expected`. The Reset holds
/// the list as it was when the subscription resynced, which may be before the test's last change: that change then follows as a
/// diff. Whether it does depends on when the failed delivery ends, on a pool thread, against the test thread's next change.
fn recovers_with_a_reset(rx: &Receiver<Vec<TextDiff>>, expected: &[&str]) {
    let first = rx.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(first.first(), Some(TextDiff::Reset { .. })),
        "a Reset first: {first:?}"
    );
    let mut mirror = Vec::new();
    for diff in first {
        apply(&mut mirror, diff);
    }
    while mirror != expected {
        let batch = rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("timed out with the mirror at {mirror:?}"));
        for diff in batch {
            apply(&mut mirror, diff);
        }
    }
}

/// How long a delivery to a well-behaved listener, or a short core task, may take while other listeners misbehave. Well below the
/// misbehaving listeners' delays, and below [`WAIT`].
const PROMPT: Duration = Duration::from_secs(1);

/// More misbehaving listeners than the runtime has workers, so that one per worker would stall the core.
const MISBEHAVING: usize = 4;

/// Checks that, with misbehaving listeners in their callbacks, a fresh subscription gets its current value and a change promptly,
/// and a core task completes promptly.
fn the_core_stays_responsive(core: &Core) {
    let (listener, rx, _) = recorder();
    let start = Instant::now();
    let _handle = core.subscribe_capabilities(listener);
    rx.recv_timeout(PROMPT)
        .expect("the current value arrives promptly");

    core.app.set_capabilities(one_server("prompt"));
    let (value, _) = rx.recv_timeout(PROMPT).expect("a change arrives promptly");
    assert_eq!(value.servers[0].key, "prompt");

    let task = runtime::handle().unwrap().spawn(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
    });
    eventually("a core task completes", || task.is_finished());
    assert!(
        start.elapsed() < PROMPT * 2,
        "the core stalled for {:?}",
        start.elapsed()
    );
}

/// Waits until `count` callbacks have been entered.
fn wait_for_entries(entered: &Receiver<()>, count: usize) {
    for _ in 0..count {
        entered
            .recv_timeout(WAIT)
            .expect("the misbehaving callback is entered");
    }
}

#[test]
fn a_blocking_listener_does_not_delay_other_subscriptions_or_tasks() {
    struct Blocks(Mutex<Sender<()>>);
    impl CapabilitiesListener for Blocks {
        fn on_value(&self, _: CapabilitiesSnapshot) {
            let _ = self.0.lock().unwrap().send(());
            thread::sleep(Duration::from_secs(3));
        }
    }
    let core = Core::new().unwrap();
    let (entered_tx, entered) = mpsc::channel();
    let _blocking: Vec<_> = (0..MISBEHAVING)
        .map(|_| core.subscribe_capabilities(Box::new(Blocks(Mutex::new(entered_tx.clone())))))
        .collect();
    wait_for_entries(&entered, MISBEHAVING);

    the_core_stays_responsive(&core);
}

#[test]
fn a_panicking_listener_with_a_slow_hook_does_not_delay_other_subscriptions_or_tasks() {
    install_slow_panic_hook();
    // Panics on every call, as a listener whose foreign code always throws, and reports each call first.
    struct Panics(Mutex<Sender<()>>);
    impl CapabilitiesListener for Panics {
        fn on_value(&self, _: CapabilitiesSnapshot) {
            let _ = self.0.lock().unwrap().send(());
            panic!("{SLOW_HOOK_PANIC}");
        }
    }
    let core = Core::new().unwrap();
    let (entered_tx, entered) = mpsc::channel();
    let panicking: Vec<_> = (0..MISBEHAVING)
        .map(|_| core.subscribe_capabilities(Box::new(Panics(Mutex::new(entered_tx.clone())))))
        .collect();
    wait_for_entries(&entered, MISBEHAVING);

    the_core_stays_responsive(&core);

    // Each panicking listener kept its subscription: it is called again for the change, once its hook has run.
    wait_for_entries(&entered, MISBEHAVING);
    assert!(panicking.iter().all(|handle| !handle.is_finished()));
}

#[test]
fn a_panicking_list_listener_with_a_slow_hook_gets_a_reset_next() {
    install_slow_panic_hook();
    let runtime = runtime::handle().unwrap();
    let list = ObservableList::new();
    let (tx, rx) = mpsc::channel();
    let calls = AtomicUsize::new(0);
    let _handle = subscribe::watch_list(&runtime, list.subscribe(), move |batch| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            panic!("{SLOW_HOOK_PANIC}");
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
    list.append(vec![String::from("next")]);
    recovers_with_a_reset(&rx, &["lost", "next"]);
}

#[test]
fn values_arrive_in_order_one_call_at_a_time() {
    struct Ordered {
        tx: Mutex<Sender<String>>,
        inside: AtomicBool,
    }
    impl CapabilitiesListener for Ordered {
        fn on_value(&self, value: CapabilitiesSnapshot) {
            assert!(
                !self.inside.swap(true, Ordering::SeqCst),
                "one call at a time"
            );
            let key = value
                .servers
                .first()
                .map_or_else(String::new, |server| server.key.clone());
            let _ = self.tx.lock().unwrap().send(key);
            self.inside.store(false, Ordering::SeqCst);
        }
    }
    let core = Core::new().unwrap();
    let (tx, rx) = mpsc::channel();
    let _handle = core.subscribe_capabilities(Box::new(Ordered {
        tx: Mutex::new(tx),
        inside: AtomicBool::new(false),
    }));
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), "");

    for step in 0..200 {
        core.app.set_capabilities(one_server(&format!("{step:03}")));
    }
    // A slow listener may skip values, but never goes back.
    let mut seen = Vec::new();
    while seen.last().map(String::as_str) != Some("199") {
        seen.push(rx.recv_timeout(WAIT).expect("the last value arrives"));
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "in order: {seen:?}"
    );
}

/// The child half of [`a_stuck_listener_does_not_hold_up_exit`]: subscribes a listener that never returns, then ends, so the test
/// binary's process exits with that callback still running.
#[test]
#[ignore = "run only as a child process by a_stuck_listener_does_not_hold_up_exit"]
fn exits_with_a_stuck_listener() {
    struct Stuck(Mutex<Sender<()>>);
    impl CapabilitiesListener for Stuck {
        fn on_value(&self, _: CapabilitiesSnapshot) {
            let _ = self.0.lock().unwrap().send(());
            loop {
                thread::park();
            }
        }
    }
    let core = Core::new().unwrap();
    let (tx, entered) = mpsc::channel();
    let handle = core.subscribe_capabilities(Box::new(Stuck(Mutex::new(tx))));
    entered
        .recv_timeout(WAIT)
        .expect("the stuck callback is entered");
    // Neither cancelling nor leaving the subscription running may hold up exit.
    handle.cancel();
}

#[test]
fn a_stuck_listener_does_not_hold_up_exit() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::exits_with_a_stuck_listener",
            "--ignored",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > WAIT * 2 {
            let _ = child.kill();
            panic!("the process did not exit with a stuck listener");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        status.success(),
        "the child failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The child really ran the one test, rather than matching nothing.
    assert!(stdout.contains("1 passed"), "the child ran: {stdout}");
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
    assert_eq!(
        report,
        format!(
            "Current 0.1.0\nPlatform: {} {}\nSettings: not stored\n\nRequests, oldest first:\n",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    );
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
