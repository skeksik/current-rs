//! The user's arrangement of the pane and Home across the bridge (ADR 0024, `current-app`'s rules): the pane's groups as arranged,
//! hidden items included, Home's rows for the editor, and the editor's moves, hides and resets. Keys cross as their open names.

use std::sync::Arc;

use crate::subscribe;
use crate::{CoreError, TaskHandle};

use super::{Browse, HomeRow, Library};

/// A group the user arranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum NavGroup {
    /// The Movies and Series tabs, keyed by the kind each shows (`movie`, `series`).
    Tabs,
    /// Favourites and Recently added, keyed `favourites` and `recently_added`.
    Views,
    /// The libraries, keyed by their item id.
    Libraries,
    /// Home's rows, keyed as [`HomeRow::key`].
    HomeRows,
}

impl From<NavGroup> for current_app::NavGroup {
    fn from(group: NavGroup) -> Self {
        match group {
            NavGroup::Tabs => Self::Tabs,
            NavGroup::Views => Self::Views,
            NavGroup::Libraries => Self::Libraries,
            NavGroup::HomeRows => Self::HomeRows,
        }
    }
}

/// A tab or a view in the pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaneEntry {
    pub key: String,
    pub hidden: bool,
}

/// A library in the pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaneLibrary {
    pub library: Library,
    pub hidden: bool,
    /// The server keeps it out of "My media": hidden until the user shows it in Current.
    pub hidden_on_server: bool,
}

/// One server's pane groups as the user arranged them, hidden items included: the pane leaves them out, its edit mode and Settings
/// show them.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaneLayout {
    pub tabs: Vec<PaneEntry>,
    pub views: Vec<PaneEntry>,
    pub libraries: Vec<PaneLibrary>,
}

impl From<&current_app::PaneLayout> for PaneLayout {
    fn from(layout: &current_app::PaneLayout) -> Self {
        let entries = |entries: &[current_app::PaneEntry]| {
            entries
                .iter()
                .map(|entry| PaneEntry {
                    key: entry.key.clone(),
                    hidden: entry.hidden,
                })
                .collect()
        };
        Self {
            tabs: entries(&layout.tabs),
            views: entries(&layout.views),
            libraries: layout
                .libraries
                .iter()
                .map(|entry| PaneLibrary {
                    library: (&entry.library).into(),
                    hidden: entry.hidden,
                    hidden_on_server: entry.hidden_on_server,
                })
                .collect(),
        }
    }
}

/// One of Home's rows for the editor, also one without items now.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct HomeRowSetting {
    pub row: HomeRow,
    pub hidden: bool,
}

/// Receives the pane's layout (`None` until the libraries first loaded), as [`super::LibrariesListener`] does.
#[uniffi::export(callback_interface)]
pub trait PaneListener: Send + Sync {
    fn on_value(&self, value: Option<PaneLayout>);
}

impl Browse {
    /// Runs an arrangement change on the core's blocking pool.
    async fn arrange(
        &self,
        change: impl FnOnce(&current_app::Browse) -> Result<(), current_store::StoreError>
        + Send
        + 'static,
    ) -> Result<(), CoreError> {
        let browse = self.browse.clone();
        self.host
            .runtime
            .spawn_blocking(move || change(&browse))
            .await
            .map_err(|error| CoreError::Runtime {
                message: error.to_string(),
            })?
            .map_err(|error| CoreError::Store {
                problem: (&error).into(),
            })
    }
}

#[uniffi::export]
impl Browse {
    /// Subscribes to the pane's groups as the user arranged them: `None` until the libraries first loaded. It follows the
    /// libraries and every change saved on this device. Keep the handle while listening.
    pub fn subscribe_pane(&self, listener: Box<dyn PaneListener>) -> Arc<TaskHandle> {
        let listener = self.host.listeners.track(listener);
        subscribe::watch_value(&self.host.runtime, self.browse.pane(), move |layout| {
            listener.listener.on_value(layout.as_ref().map(Into::into));
        })
    }

    /// Home's rows as the user arranged them, hidden ones and those without items now included, for the editor.
    pub fn home_layout(&self) -> Vec<HomeRowSetting> {
        self.browse
            .home_layout()
            .into_iter()
            .map(|setting| HomeRowSetting {
                row: current_app::HomeRow {
                    key: setting.key,
                    kind: setting.kind,
                    title: setting.title,
                    library: setting.library,
                }
                .into(),
                hidden: setting.hidden,
            })
            .collect()
    }

    /// Moves `key` to `to` in `group` (an index in the group as arranged, hidden items included) and saves it at once on the
    /// core's blocking pool. The pane's subscription and Home follow. Not cancellable: it is one small write.
    ///
    /// # Errors
    /// [`CoreError::Store`] if the settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub async fn move_item(&self, group: NavGroup, key: String, to: u32) -> Result<(), CoreError> {
        let to = usize::try_from(to).unwrap_or(usize::MAX);
        self.arrange(move |browse| browse.move_item(group.into(), &key, to))
            .await
    }

    /// Hides or shows `key` in `group`, as [`Browse::move_item`].
    ///
    /// # Errors
    /// As [`Browse::move_item`].
    pub async fn set_hidden(
        &self,
        group: NavGroup,
        key: String,
        hidden: bool,
    ) -> Result<(), CoreError> {
        self.arrange(move |browse| browse.set_hidden(group.into(), &key, hidden))
            .await
    }

    /// Forgets the user's arrangement of `group` (Reset): back to the server's or the app's order, as [`Browse::move_item`].
    ///
    /// # Errors
    /// As [`Browse::move_item`].
    pub async fn reset_group(&self, group: NavGroup) -> Result<(), CoreError> {
        self.arrange(move |browse| browse.reset_group(group.into()))
            .await
    }
}
