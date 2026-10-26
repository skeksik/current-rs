//! What the engine plays, for Settings' "Supported formats" (ADR 0021), without playing anything: the capabilities of the last engine
//! that started in this run, or else those of a short-lived headless engine started only to read them.
//!
//! The headless engine is libmpv with `vo=null` and `ao=null` and no `force-window` (`current-mpv`'s `Output::Headless`), so it opens
//! no window and no audio device. It runs on the blocking pool, never the UI thread, and is shut down as soon as its lists are read.
//! It takes the process's one engine slot for that moment (about 10 ms on CI's Windows runner); a player starting meanwhile waits for
//! it, as it waits for a closing one, and then wins: once a player's engine has reported, its capabilities are the ones kept. The probe
//! never waits for the slot: one that finds it taken (a player starting, playing or closing) answers with that player's capabilities
//! as soon as they are known, rather than waiting for the player to close.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use current_model::EngineCapabilities;
use tokio::sync::watch;

/// What is known about the engine of this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineReport {
    /// This build has no engine (no `libmpv`, no `test-engine`).
    Unavailable,
    /// The engine could not be started to read its capabilities.
    Failed,
    Ready(EngineCapabilities),
}

/// What one headless start gave.
#[derive(Debug)]
enum Probe {
    Report(EngineReport),
    /// Another engine holds the process's slot: a player starting or playing, or one closing.
    #[cfg_attr(not(feature = "libmpv"), allow(dead_code))]
    Busy,
}

/// How long a probe that found the slot taken waits for the holder's capabilities. A player remembers them as soon as its engine has
/// started (milliseconds; `a_headless_probe_and_a_player_start_both_succeed`), and one playing or closing already has.
const HOLDER_WAIT: Duration = Duration::from_secs(5);

/// The capabilities known in this run. One per app.
#[derive(Debug)]
pub(crate) struct KnownCapabilities {
    known: watch::Sender<Option<EngineCapabilities>>,
    /// Counts player starts that failed, so a probe waiting for a slot holder's capabilities stops when that start failed.
    failed_starts: watch::Sender<u64>,
    /// How long a probe waits for the slot holder's capabilities ([`HOLDER_WAIT`]; shorter in tests).
    holder_wait: Duration,
    /// One headless start at a time: a second caller waits and then finds the first one's answer.
    probing: tokio::sync::Mutex<()>,
}

impl Default for KnownCapabilities {
    fn default() -> Self {
        Self {
            known: watch::Sender::new(None),
            failed_starts: watch::Sender::new(0),
            holder_wait: HOLDER_WAIT,
            probing: tokio::sync::Mutex::new(()),
        }
    }
}

impl KnownCapabilities {
    /// Keeps the capabilities a player's engine reported at its start. They replace any others: a player's engine wins.
    pub(crate) fn remember(&self, capabilities: EngineCapabilities) {
        self.known.send_replace(Some(capabilities));
    }

    /// A player's engine failed to start: a probe waiting for its capabilities stops waiting.
    pub(crate) fn start_failed(&self) {
        self.failed_starts.send_modify(|count| *count += 1);
    }

    /// Keeps a headless engine's capabilities unless a player's are already known, and returns the ones kept.
    fn fill(&self, capabilities: EngineCapabilities) -> EngineCapabilities {
        let mut kept = None;
        self.known.send_if_modified(|known| {
            let filled = known.is_none();
            kept = Some(known.get_or_insert(capabilities).clone());
            filled
        });
        kept.unwrap_or_default()
    }

    pub(crate) fn known(&self) -> Option<EngineCapabilities> {
        self.known.borrow().clone()
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
        F: Future<Output = Probe> + Send + 'static,
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
        F: Future<Output = Probe>,
    {
        let _probing = self.probing.lock().await;
        if let Some(capabilities) = self.known() {
            return EngineReport::Ready(capabilities);
        }
        let mut failed_starts = self.failed_starts.subscribe();
        failed_starts.mark_unchanged();
        match probe().await {
            Probe::Report(EngineReport::Ready(capabilities)) => {
                EngineReport::Ready(self.fill(capabilities))
            }
            // The slot's holder is a player: its capabilities answer, as soon as it has them. The wait ends early when a player's start
            // fails, and at the latest after `holder_wait` (a holder that never reports, or the app shutting down): then the page says
            // "Couldn't check" rather than waiting on.
            Probe::Busy => {
                let mut known = self.known.subscribe();
                let holder = async {
                    tokio::select! {
                        found = known.wait_for(Option::is_some) => found.ok().and_then(|found| found.clone()),
                        _ = failed_starts.changed() => None,
                    }
                };
                match tokio::time::timeout(self.holder_wait, holder).await {
                    Ok(Some(capabilities)) => EngineReport::Ready(capabilities),
                    Ok(None) => self
                        .known()
                        .map_or(EngineReport::Failed, EngineReport::Ready),
                    Err(_) => EngineReport::Failed,
                }
            }
            Probe::Report(other) => self.known().map_or(other, EngineReport::Ready),
        }
    }
}

#[cfg(feature = "libmpv")]
async fn probe() -> Probe {
    match tokio::task::spawn_blocking(super::mpv::probe_capabilities).await {
        Ok(Ok(capabilities)) => Probe::Report(EngineReport::Ready(capabilities)),
        Ok(Err(current_mpv::EngineError::Busy)) => Probe::Busy,
        _ => Probe::Report(EngineReport::Failed),
    }
}

#[cfg(all(not(feature = "libmpv"), any(test, feature = "test-engine")))]
async fn probe() -> Probe {
    Probe::Report(EngineReport::Ready(super::test_engine::capabilities()))
}

#[cfg(not(any(feature = "libmpv", test, feature = "test-engine")))]
async fn probe() -> Probe {
    Probe::Report(EngineReport::Unavailable)
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
                Probe::Report(EngineReport::Failed)
            })
            .await;
        assert_eq!(report, EngineReport::Ready(other()));
        // Without a player, a failure is reported and not kept.
        let fresh = Arc::new(KnownCapabilities::default());
        assert_eq!(
            fresh
                .get_with(|| async { Probe::Report(EngineReport::Failed) })
                .await,
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
                Probe::Report(EngineReport::Ready(EngineCapabilities::default()))
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
            Probe::Report(EngineReport::Ready(other()))
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
            known
                .get_with(|| async { Probe::Report(EngineReport::Failed) })
                .await,
            EngineReport::Ready(other())
        );
    }

    #[tokio::test]
    async fn a_probe_beside_a_player_takes_its_capabilities_without_waiting_for_it_to_close() {
        let known = Arc::new(KnownCapabilities::default());
        let player = Arc::clone(&known);
        // The player took the slot and remembers its capabilities 50 ms later; it stays open.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            player.remember(other());
        });
        let started = std::time::Instant::now();
        let report = known.get_with(|| async { Probe::Busy }).await;
        assert_eq!(report, EngineReport::Ready(other()));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_probe_beside_a_failed_player_start_stops_waiting() {
        let known = Arc::new(KnownCapabilities::default());
        let player = Arc::clone(&known);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            player.start_failed();
        });
        let started = std::time::Instant::now();
        assert_eq!(
            known.get_with(|| async { Probe::Busy }).await,
            EngineReport::Failed
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_holder_that_never_reports_ends_the_wait_after_its_bound() {
        let known = Arc::new(KnownCapabilities {
            holder_wait: Duration::from_millis(100),
            ..KnownCapabilities::default()
        });
        let started = std::time::Instant::now();
        assert_eq!(
            known.get_with(|| async { Probe::Busy }).await,
            EngineReport::Failed
        );
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(known.known(), None);
    }
}
