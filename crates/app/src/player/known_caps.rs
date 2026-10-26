//! What the engine plays, for Settings' "Supported formats" (ADR 0021), without playing anything: the capabilities of the last engine
//! that started in this run, or else those of a short-lived headless engine started only to read them.
//!
//! The headless engine is libmpv with `vo=null` and `ao=null` and no `force-window` (`current-mpv`'s `Output::Headless`), so it opens
//! no window and no audio device. It runs on the blocking pool, never the UI thread, and is shut down as soon as its lists are read.
//! It takes the process's one engine slot for that moment (not measured); a player starting meanwhile waits for it, as it waits for a
//! closing one, and then wins: once a player's engine has reported, its capabilities are the ones kept. A probe that finds the slot
//! held by a player fails `Busy` after `current-mpv`'s wait, and the player's capabilities answer instead.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

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
    /// Keeps the capabilities a player's engine reported at its start. They replace any others: a player's engine wins.
    pub(crate) fn remember(&self, capabilities: EngineCapabilities) {
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = Some(capabilities);
    }

    /// Keeps a headless engine's capabilities unless a player's are already known, and returns the ones kept.
    fn fill(&self, capabilities: EngineCapabilities) -> EngineCapabilities {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(capabilities)
            .clone()
    }

    pub(crate) fn known(&self) -> Option<EngineCapabilities> {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The known capabilities, or those of a headless engine started now (on the blocking pool) when none are. Needs tokio's runtime.
    pub(crate) async fn get(self: &Arc<Self>) -> EngineReport {
        self.get_with(probe).await
    }

    /// [`KnownCapabilities::get`] with the probe given. The probe runs in a task of its own, so it runs to its end and its answer is kept
    /// even when the caller stops waiting (a page closed during it); the next caller finds it. A failed probe is not kept: the next call
    /// tries again.
    async fn get_with<P, F>(self: &Arc<Self>, probe: P) -> EngineReport
    where
        P: FnOnce() -> F + Send + 'static,
        F: Future<Output = EngineReport> + Send + 'static,
    {
        if let Some(capabilities) = self.known() {
            return EngineReport::Ready(capabilities);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move { this.probe_once(probe).await })
            .await
            .unwrap_or(EngineReport::Failed)
    }

    async fn probe_once<P, F>(&self, probe: P) -> EngineReport
    where
        P: FnOnce() -> F,
        F: Future<Output = EngineReport>,
    {
        let _probing = self.probing.lock().await;
        if let Some(capabilities) = self.known() {
            return EngineReport::Ready(capabilities);
        }
        match probe().await {
            EngineReport::Ready(capabilities) => EngineReport::Ready(self.fill(capabilities)),
            // A probe that failed behind a player's start (the slot held past `current-mpv`'s wait) finds the player's capabilities.
            other => self.known().map_or(other, EngineReport::Ready),
        }
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
    use std::time::Duration;

    use current_model::{Codec, CodecSupport};

    use super::*;

    fn other() -> EngineCapabilities {
        EngineCapabilities {
            video: vec![CodecSupport::any(Codec::AV1)],
            ..EngineCapabilities::default()
        }
    }

    #[tokio::test]
    async fn a_probe_fills_once_and_a_player_start_replaces_it() {
        let known = Arc::new(KnownCapabilities::default());
        let test_engine = super::super::test_engine::capabilities();
        assert_eq!(known.get().await, EngineReport::Ready(test_engine.clone()));
        assert_eq!(known.known(), Some(test_engine));
        known.remember(other());
        assert_eq!(known.get().await, EngineReport::Ready(other()));
    }

    #[tokio::test]
    async fn a_failed_probe_finds_what_a_player_remembered_meanwhile() {
        let known = Arc::new(KnownCapabilities::default());
        let player = Arc::clone(&known);
        let report = known
            .get_with(move || async move {
                player.remember(other());
                EngineReport::Failed
            })
            .await;
        assert_eq!(report, EngineReport::Ready(other()));
        // Without a player, a failure is reported and not kept.
        let fresh = Arc::new(KnownCapabilities::default());
        assert_eq!(
            fresh.get_with(|| async { EngineReport::Failed }).await,
            EngineReport::Failed
        );
        assert_eq!(fresh.known(), None);
    }

    #[tokio::test]
    async fn a_probe_never_replaces_a_players_capabilities() {
        let known = Arc::new(KnownCapabilities::default());
        let player = Arc::clone(&known);
        let report = known
            .get_with(move || async move {
                player.remember(other());
                EngineReport::Ready(EngineCapabilities::default())
            })
            .await;
        assert_eq!(report, EngineReport::Ready(other()));
        assert_eq!(known.known(), Some(other()));
    }

    #[tokio::test]
    async fn a_probe_whose_caller_gave_up_still_keeps_its_answer() {
        let known = Arc::new(KnownCapabilities::default());
        let slow = known.get_with(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            EngineReport::Ready(other())
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(10), slow)
                .await
                .is_err()
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while known.known().is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(known.known(), Some(other()));
        // The next caller does not probe again.
        assert_eq!(
            known.get_with(|| async { EngineReport::Failed }).await,
            EngineReport::Ready(other())
        );
    }
}
