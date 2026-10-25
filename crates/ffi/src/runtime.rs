//! The one async runtime the core owns (ADR 0004). Created on first use, never exposed: the foreign side only polls futures that
//! await work spawned here.

use std::sync::{Mutex, OnceLock};

use tokio::runtime::{Builder, Handle, Runtime};

use crate::CoreError;

/// The name of every thread of the core's runtime: its two workers and its blocking pool. Listener callbacks run on the blocking
/// pool (`subscribe.rs`), never on a worker; tokio gives both kinds the same name.
pub const WORKER_THREAD_NAME: &str = "core-worker";

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
/// Serialises the first start, so exactly one runtime is ever built: a second one dropped by a racing caller would panic if that
/// caller were inside an async context.
static STARTING: Mutex<()> = Mutex::new(());

/// The core's runtime, started on first use.
pub(crate) fn handle() -> Result<Handle, CoreError> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime.handle().clone());
    }
    // Nothing runs under this lock that could leave state half-changed, so a poisoned lock is safe.
    let _starting = STARTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime.handle().clone());
    }
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(WORKER_THREAD_NAME)
        // I/O for the HTTP clients and discovery's UDP socket, time for timeouts and polling.
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| CoreError::Runtime {
            message: error.to_string(),
        })?;
    Ok(RUNTIME.get_or_init(|| runtime).handle().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_is_started_once() {
        let first = handle().unwrap();
        let second = handle().unwrap();
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn racing_starts_share_one_runtime() {
        let ids: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| handle().unwrap().id()))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[tokio::test]
    async fn starts_from_inside_an_async_context() {
        assert!(handle().is_ok());
    }
}
