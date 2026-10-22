//! The one async runtime the core owns (ADR 0004). Created on first use, never exposed: the foreign side only polls futures that
//! await work spawned here.

use std::sync::OnceLock;

use tokio::runtime::{Builder, Handle, Runtime};

use crate::CoreError;

/// The name of the core's worker threads. Every listener callback runs on one of them.
pub const WORKER_THREAD_NAME: &str = "core-worker";

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The core's runtime, started on first use.
pub(crate) fn handle() -> Result<Handle, CoreError> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime.handle().clone());
    }
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(WORKER_THREAD_NAME)
        .enable_time()
        .build()
        .map_err(|error| CoreError::Runtime {
            message: error.to_string(),
        })?;
    // Two threads may race to start it: the loser's runtime is dropped unused.
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
}
