//! The playback settings while the app runs (ADR 0019, choices 8 and 10): the quality remembered for local and internet addresses,
//! saved when the user picks one, and the automatic skipping, which live players follow at once. Saved in `settings.json` when the app
//! has a data folder (`current-store`'s open values), else for the run only.

use std::sync::{Arc, Mutex, PoisonError};

use current_playback::{Quality, QualityMemory};
use current_store::{SettingsStore, StoreError, StreamQuality};
use tokio::sync::watch;

use crate::session::Shared;

pub(crate) struct PlaybackPrefs {
    quality: Mutex<QualityMemory>,
    auto_skip: watch::Sender<Vec<String>>,
    quality_resolution: watch::Sender<bool>,
    quality_all: watch::Sender<bool>,
    /// Serializes the settings writes of this file's settings, so the last change is the one saved.
    write: Mutex<()>,
}

fn quality(stored: StreamQuality) -> Quality {
    match stored {
        StreamQuality::Maximum => Quality::Maximum,
        StreamQuality::Bitrate(bits) => Quality::Bitrate(bits),
    }
}

fn stored(quality: Quality) -> StreamQuality {
    match quality {
        Quality::Maximum => StreamQuality::Maximum,
        Quality::Bitrate(bits) => StreamQuality::Bitrate(bits),
    }
}

impl PlaybackPrefs {
    pub(crate) fn load(settings: Option<&SettingsStore>) -> Self {
        let playback = settings
            .map(|store| store.get().playback)
            .unwrap_or_default();
        Self {
            quality: Mutex::new(QualityMemory {
                local: quality(playback.quality_local),
                internet: quality(playback.quality_internet),
            }),
            auto_skip: watch::Sender::new(playback.auto_skip),
            quality_resolution: watch::Sender::new(playback.quality_resolution),
            quality_all: watch::Sender::new(playback.quality_all),
            write: Mutex::new(()),
        }
    }

    pub(crate) fn quality(&self) -> QualityMemory {
        *self.quality.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The segment kinds skipped automatically, by name; empty when off.
    pub(crate) fn auto_skip(&self) -> watch::Receiver<Vec<String>> {
        self.auto_skip.subscribe()
    }

    /// Whether the quality menu names the resolution each choice gives; on by default.
    pub(crate) fn quality_resolution(&self) -> bool {
        *self.quality_resolution.borrow()
    }

    /// Sets whether the quality menu names resolutions and saves it. Blocking file I/O. It applies at once, also when the save fails
    /// (the error says so; the setting stands for this run).
    pub(crate) fn set_quality_resolution(
        &self,
        shared: &Shared,
        on: bool,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.quality_resolution.send_replace(on);
        match &shared.storage {
            Some(storage) => storage
                .settings
                .update(|settings| settings.playback.quality_resolution = on),
            None => Ok(()),
        }
    }

    /// Whether the quality menu lists every step, not only the common ones; off by default.
    pub(crate) fn quality_all(&self) -> bool {
        *self.quality_all.borrow()
    }

    /// Sets whether the quality menu lists every step and saves it. Blocking file I/O. It applies at once, also when the save fails
    /// (the error says so; the setting stands for this run).
    pub(crate) fn set_quality_all(&self, shared: &Shared, all: bool) -> Result<(), StoreError> {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.quality_all.send_replace(all);
        match &shared.storage {
            Some(storage) => storage
                .settings
                .update(|settings| settings.playback.quality_all = all),
            None => Ok(()),
        }
    }

    /// Remembers `quality` for the kind of address and saves it in the background; a failed save keeps it for the run.
    pub(crate) fn save_quality(
        self: &Arc<Self>,
        shared: &Arc<Shared>,
        local: bool,
        quality: Quality,
    ) {
        self.quality
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set(local, quality);
        let prefs = self.clone();
        let shared = shared.clone();
        // Blocking file I/O, off the workers. Nothing to tell the player when it fails: the choice stands for this run.
        tokio::task::spawn_blocking(move || {
            let Some(storage) = &shared.storage else {
                return;
            };
            let _guard = prefs.write.lock().unwrap_or_else(PoisonError::into_inner);
            let memory = prefs.quality();
            let _ = storage.settings.update(|settings| {
                settings.playback.quality_local = stored(memory.local);
                settings.playback.quality_internet = stored(memory.internet);
            });
        });
    }

    /// Sets the automatic skipping and saves it. Blocking file I/O. Live players follow it at once, also when the save fails (the error
    /// says so; the setting stands for this run).
    pub(crate) fn set_auto_skip(
        &self,
        shared: &Shared,
        kinds: Vec<String>,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.auto_skip.send_if_modified(|current| {
            if *current == kinds {
                return false;
            }
            current.clone_from(&kinds);
            true
        });
        match &shared.storage {
            Some(storage) => storage
                .settings
                .update(|settings| settings.playback.auto_skip = kinds),
            None => Ok(()),
        }
    }
}
