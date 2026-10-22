//! The core's one tokio runtime. UIs never see it: every call returns at once or is a UniFFI async call, and
//! every listener callback comes from one of its worker threads.

use std::sync::OnceLock;

use crate::BridgeError;

const WORKER_PREFIX: &str = "core-worker";

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Handle to the global runtime, created on first use. If two threads race, the loser's runtime is dropped
/// unused (outside any async context, so the drop cannot panic).
pub(crate) fn handle() -> Result<tokio::runtime::Handle, BridgeError> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt.handle().clone());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(WORKER_PREFIX)
        .enable_time()
        .build()
        .map_err(|e| BridgeError::Runtime {
            message: e.to_string(),
        })?;
    Ok(RUNTIME.get_or_init(|| rt).handle().clone())
}

pub(crate) fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_runtime_for_every_caller() {
        let a = handle().unwrap();
        let b = handle().unwrap();
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn tasks_run_on_named_worker_threads() {
        let name = handle()
            .unwrap()
            .block_on(async { tokio::spawn(async { thread_name() }).await.unwrap() });
        assert_eq!(name, WORKER_PREFIX);
    }
}
