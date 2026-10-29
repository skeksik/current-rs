//! The playback settings while the app runs (ADR 0019, choices 8 and 10): the preferred quality and the quality menu's switches,
//! which players read for each item they play, and the automatic skipping, which live players follow at once. A quality picked in the
//! player is not saved: it changes only the item playing (owner, 2026-10-27). Saved in `settings.json` when the app has a data folder
//! (`current-store`'s open values), else for the run only.

use std::sync::{Mutex, PoisonError};

use current_playback::{EnginePreference, Quality, QualityPreference, listed_quality};
use current_store::{PlayerChoice, SettingsStore, StoreError, StreamQuality};
use tokio::sync::watch;

use crate::session::Shared;

pub(crate) struct PlaybackPrefs {
    preferred_quality: watch::Sender<Quality>,
    auto_skip: watch::Sender<Vec<String>>,
    quality_resolution: watch::Sender<bool>,
    quality_all: watch::Sender<bool>,
    /// "Player" (ADR 0025, choice 3).
    player: watch::Sender<PlayerChoice>,
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
            // One the list does not offer (a hand-edited file) reads as the nearest it does, as switching "Show all" off moves it.
            preferred_quality: watch::Sender::new(listed_quality(
                quality(playback.quality_preferred),
                playback.quality_all,
            )),
            auto_skip: watch::Sender::new(playback.auto_skip),
            quality_resolution: watch::Sender::new(playback.quality_resolution),
            quality_all: watch::Sender::new(playback.quality_all),
            player: watch::Sender::new(playback.player),
            write: Mutex::new(()),
        }
    }

    /// The preferred quality; "Maximum" by default.
    pub(crate) fn preferred_quality(&self) -> Quality {
        *self.preferred_quality.borrow()
    }

    /// Sets the preferred quality and saves it. Blocking file I/O. It applies from the next item played, also when the save fails (the
    /// error says so; the setting stands for this run).
    pub(crate) fn set_preferred_quality(
        &self,
        shared: &Shared,
        preferred: Quality,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        // Always an entry of Settings' list, as the list promises: a step it does not offer becomes the nearest it does (review 2).
        let preferred = listed_quality(preferred, self.quality_all());
        self.preferred_quality.send_replace(preferred);
        match &shared.storage {
            Some(storage) => storage
                .settings
                .update(|settings| settings.playback.quality_preferred = stored(preferred)),
            None => Ok(()),
        }
    }

    /// What a player plays each item at: the preferred quality, snapped to the item's quality menu as Settings lists it.
    pub(crate) fn quality_preference(&self) -> QualityPreference {
        QualityPreference {
            preferred: self.preferred_quality(),
            all: self.quality_all(),
        }
    }

    /// Which engine plays a file, for the next item played (ADR 0025, choice 3); "Automatic" by default.
    pub(crate) fn engine_preference(&self) -> EnginePreference {
        match *self.player.borrow() {
            PlayerChoice::Automatic => EnginePreference::Automatic,
            PlayerChoice::AlwaysAppEngine => EnginePreference::AlwaysAppEngine,
        }
    }

    /// The "Player" setting as stored.
    pub(crate) fn player_choice(&self) -> PlayerChoice {
        *self.player.borrow()
    }

    /// Sets "Player" and saves it. Blocking file I/O. It applies from the next item played, also when the save fails (the error says
    /// so; the setting stands for this run).
    pub(crate) fn set_player_choice(
        &self,
        shared: &Shared,
        choice: PlayerChoice,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.player.send_replace(choice);
        match &shared.storage {
            Some(storage) => storage
                .settings
                .update(|settings| settings.playback.player = choice),
            None => Ok(()),
        }
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
        // A preferred step the list no longer offers moves to the nearest one listed, in the same save (owner, 2026-10-27).
        let preferred = listed_quality(self.preferred_quality(), all);
        self.preferred_quality.send_replace(preferred);
        match &shared.storage {
            Some(storage) => storage.settings.update(|settings| {
                settings.playback.quality_all = all;
                settings.playback.quality_preferred = stored(preferred);
            }),
            None => Ok(()),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_player_setting_becomes_the_engine_preference() {
        assert_eq!(
            PlaybackPrefs::load(None).engine_preference(),
            EnginePreference::Automatic
        );
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path()).unwrap();
        assert_eq!(
            PlaybackPrefs::load(Some(&store)).engine_preference(),
            EnginePreference::Automatic
        );
        store
            .update(|settings| settings.playback.player = PlayerChoice::AlwaysAppEngine)
            .unwrap();
        assert_eq!(
            PlaybackPrefs::load(Some(&store)).engine_preference(),
            EnginePreference::AlwaysAppEngine
        );
    }
}
