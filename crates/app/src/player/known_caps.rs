//! What the engine plays, for Settings' "Supported formats" (ADR 0021), without playing anything: the capabilities of the last engine
//! that started in this run, or else those of a short-lived headless engine started only to read them.
//!
//! The headless engine is libmpv with `vo=null` and `ao=null` and no `force-window` (`current-mpv`'s `Output::Headless`), so it opens
//! no window and no audio device. It takes the process's one engine slot for the moment it lives (well under a second, not measured);
//! a player starting meanwhile waits for it, as it waits for a closing one.

use std::sync::{Mutex, PoisonError};

use current_model::EngineCapabilities;

/// What is known about the engine of this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineReport {
    /// This build has no engine (no `libmpv`, no `test-engine`).
    Unavailable,
    /// The engine could not be started to read its capabilities.
    Failed,
    Ready(EngineCapabilities),
}

/// The capabilities known in this run. One per app.
#[derive(Debug, Default)]
pub(crate) struct KnownCapabilities {
    known: Mutex<Option<EngineCapabilities>>,
    /// One headless start at a time: a second caller waits and then finds the first one's answer.
    probing: tokio::sync::Mutex<()>,
}

impl KnownCapabilities {
    /// Keeps the capabilities an engine reported at its start.
    pub(crate) fn remember(&self, capabilities: EngineCapabilities) {
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = Some(capabilities);
    }

    fn known(&self) -> Option<EngineCapabilities> {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The known capabilities, or those of a headless engine started now (on the blocking pool) when none are. A failed start is not
    /// remembered: the next call tries again. Needs tokio's runtime.
    pub(crate) async fn get(&self) -> EngineReport {
        if let Some(capabilities) = self.known() {
            return EngineReport::Ready(capabilities);
        }
        let _probing = self.probing.lock().await;
        if let Some(capabilities) = self.known() {
            return EngineReport::Ready(capabilities);
        }
        let report = probe().await;
        if let EngineReport::Ready(capabilities) = &report {
            self.remember(capabilities.clone());
        }
        report
    }
}

#[cfg(feature = "libmpv")]
async fn probe() -> EngineReport {
    match tokio::task::spawn_blocking(super::mpv::probe_capabilities).await {
        Ok(Ok(capabilities)) => EngineReport::Ready(capabilities),
        _ => EngineReport::Failed,
    }
}

#[cfg(all(not(feature = "libmpv"), any(test, feature = "test-engine")))]
async fn probe() -> EngineReport {
    EngineReport::Ready(super::test_engine::capabilities())
}

#[cfg(not(any(feature = "libmpv", test, feature = "test-engine")))]
async fn probe() -> EngineReport {
    EngineReport::Unavailable
}

#[cfg(all(test, not(feature = "libmpv")))]
mod tests {
    use current_model::{Codec, CodecSupport};

    use super::*;

    #[tokio::test]
    async fn the_last_engine_start_wins_over_a_probe() {
        let known = KnownCapabilities::default();
        assert_eq!(
            known.get().await,
            EngineReport::Ready(super::super::test_engine::capabilities())
        );
        let other = EngineCapabilities {
            video: vec![CodecSupport::any(Codec::AV1)],
            ..EngineCapabilities::default()
        };
        known.remember(other.clone());
        assert_eq!(known.get().await, EngineReport::Ready(other));
    }
}
