//! Settings' "Remember sort", "Remember filter" and "Remember view" across the bridge, and the choices remembered per library
//! (`current-app`'s rules): a library opens with what [`Core::library_choices`] gives, and the UI passes each choice the user makes
//! to [`Core::remember_library_choices`]. Sort keys, filters and tile shapes cross as their open names, unchanged (ADR 0005).

use current_store as store;

use crate::{Core, CoreError};

use super::{ItemRef, SortChoice, SortOrder};

/// Settings' three switches, all off by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct LibraryMemorySettings {
    pub remember_sort: bool,
    pub remember_filter: bool,
    pub remember_view: bool,
}

/// A library grid's view.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LibraryView {
    /// The tiles' shape: `poster` or `wide`, or one a later version names.
    pub tile: String,
    /// Whether the tiles show their titles.
    pub show_titles: bool,
}

/// A library's choices, each part `None` when there is none: from [`Core::library_choices`], nothing remembered (or its switch is
/// off) and the grid opens with its first choice; to [`Core::remember_library_choices`], left as it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct LibraryChoices {
    pub sort: Option<SortChoice>,
    /// The filters (`unplayed`, `played`, `favourite`, or one the server names); empty for "All".
    pub filters: Option<Vec<String>>,
    pub view: Option<LibraryView>,
}

impl From<store::LibraryChoices> for LibraryChoices {
    fn from(choices: store::LibraryChoices) -> Self {
        Self {
            sort: choices.sort.map(|sort| SortChoice {
                key: sort.key.as_str().to_owned(),
                order: match sort.order {
                    current_model::SortOrder::Ascending => SortOrder::Ascending,
                    current_model::SortOrder::Descending => SortOrder::Descending,
                },
            }),
            filters: choices.filters.map(|filters| {
                filters
                    .iter()
                    .map(|filter| filter.as_str().to_owned())
                    .collect()
            }),
            view: choices.view.map(|view| LibraryView {
                tile: view.tile,
                show_titles: view.titles,
            }),
        }
    }
}

impl From<LibraryChoices> for store::LibraryChoices {
    fn from(choices: LibraryChoices) -> Self {
        Self {
            sort: choices.sort.map(|sort| store::RememberedSort {
                key: current_model::SortKey::new(sort.key),
                order: match sort.order {
                    SortOrder::Ascending => current_model::SortOrder::Ascending,
                    SortOrder::Descending => current_model::SortOrder::Descending,
                },
            }),
            filters: choices.filters.map(|filters| {
                filters
                    .into_iter()
                    .map(current_model::ItemFilter::new)
                    .collect()
            }),
            view: choices.view.map(|view| store::RememberedView {
                tile: view.tile,
                titles: view.show_titles,
            }),
        }
    }
}

impl Core {
    /// Runs a settings change on the core's blocking pool.
    pub(super) async fn save_settings(
        &self,
        change: impl FnOnce(&current_app::App) -> Result<(), store::StoreError> + Send + 'static,
    ) -> Result<(), CoreError> {
        let app = self.app.clone();
        self.runtime
            .spawn_blocking(move || change(&app))
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
impl Core {
    /// Settings' "Remember sort", "Remember filter" and "Remember view" now.
    pub fn library_memory(&self) -> LibraryMemorySettings {
        let memory = self.app.library_memory();
        LibraryMemorySettings {
            remember_sort: memory.remember_sort,
            remember_filter: memory.remember_filter,
            remember_view: memory.remember_view,
        }
    }

    /// The Settings switch "Remember sort". Saved at once on the core's blocking pool; off forgets every library's sort in the same
    /// save, so on again starts afresh. Not cancellable: it is one small write.
    ///
    /// # Errors
    /// [`CoreError::Store`] if the settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub async fn set_remember_sort(&self, on: bool) -> Result<(), CoreError> {
        self.save_settings(move |app| app.set_remember_sort(on))
            .await
    }

    /// The Settings switch "Remember filter", as [`Core::set_remember_sort`].
    ///
    /// # Errors
    /// As [`Core::set_remember_sort`].
    pub async fn set_remember_filter(&self, on: bool) -> Result<(), CoreError> {
        self.save_settings(move |app| app.set_remember_filter(on))
            .await
    }

    /// The Settings switch "Remember view", as [`Core::set_remember_sort`].
    ///
    /// # Errors
    /// As [`Core::set_remember_sort`].
    pub async fn set_remember_view(&self, on: bool) -> Result<(), CoreError> {
        self.save_settings(move |app| app.set_remember_view(on))
            .await
    }

    /// What `library` opens with: the last sort, filters and view chosen there, each only while its switch is on.
    pub fn library_choices(&self, library: ItemRef) -> LibraryChoices {
        self.app.library_choices(&(&library).into()).into()
    }

    /// Remembers the user's choices in `library`: each part given whose switch is on; the others stay as they were. Pass a part
    /// each time the user changes it (all of them is fine too). Saved at once on the core's blocking pool; nothing is written when
    /// nothing changed. Not cancellable.
    ///
    /// # Errors
    /// As [`Core::set_remember_sort`].
    pub async fn remember_library_choices(
        &self,
        library: ItemRef,
        choices: LibraryChoices,
    ) -> Result<(), CoreError> {
        let library: current_model::ItemRef = (&library).into();
        self.save_settings(move |app| app.remember_library_choices(&library, choices.into()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::block_on;
    use crate::{SecretStore, ServerRef, StoreProblem, open_core};

    fn library(id: &str) -> ItemRef {
        ItemRef {
            server: ServerRef {
                service: "jellyfin".into(),
                key: "1".into(),
            },
            id: id.into(),
        }
    }

    fn everything() -> LibraryChoices {
        LibraryChoices {
            sort: Some(SortChoice {
                key: "date_created".into(),
                order: SortOrder::Descending,
            }),
            filters: Some(vec!["unplayed".into()]),
            view: Some(LibraryView {
                tile: "wide".into(),
                show_titles: false,
            }),
        }
    }

    fn open(temp: &tempfile::TempDir) -> std::sync::Arc<Core> {
        block_on(open_core(
            temp.path().to_str().unwrap().to_owned(),
            temp.path().join("cache").to_str().unwrap().to_owned(),
            String::new(),
            SecretStore::Memory,
        ))
        .unwrap()
    }

    #[test]
    fn choices_are_remembered_per_library_while_their_switch_is_on() {
        let temp = tempfile::tempdir().unwrap();
        let core = open(&temp);
        assert_eq!(core.library_memory(), LibraryMemorySettings::default());
        // Off: nothing is saved.
        block_on(core.remember_library_choices(library("a"), everything())).unwrap();
        assert_eq!(
            core.library_choices(library("a")),
            LibraryChoices::default()
        );

        block_on(core.set_remember_sort(true)).unwrap();
        block_on(core.set_remember_filter(true)).unwrap();
        block_on(core.set_remember_view(true)).unwrap();
        block_on(core.remember_library_choices(library("a"), everything())).unwrap();
        assert_eq!(core.library_choices(library("a")), everything());
        // "All" crosses as an empty list, and a part not given stays.
        block_on(core.remember_library_choices(
            library("a"),
            LibraryChoices {
                filters: Some(Vec::new()),
                ..LibraryChoices::default()
            },
        ))
        .unwrap();
        assert_eq!(core.library_choices(library("a")).filters, Some(Vec::new()));
        assert_eq!(core.library_choices(library("a")).sort, everything().sort);
        assert_eq!(
            core.library_choices(library("b")),
            LibraryChoices::default()
        );

        // Off forgets: on again starts afresh.
        block_on(core.set_remember_view(false)).unwrap();
        block_on(core.set_remember_view(true)).unwrap();
        assert_eq!(core.library_choices(library("a")).view, None);
        drop(core);

        let core = open(&temp);
        assert_eq!(
            core.library_memory(),
            LibraryMemorySettings {
                remember_sort: true,
                remember_filter: true,
                remember_view: true
            }
        );
        assert_eq!(core.library_choices(library("a")).sort, everything().sort);
    }

    #[test]
    fn a_choice_that_cannot_be_saved_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(current_store::SETTINGS_FILE),
            r#"{"version": 99, "device_id": "abc"}"#,
        )
        .unwrap();
        let core = open(&temp);
        let result = block_on(core.set_remember_sort(true));
        assert!(
            matches!(
                result,
                Err(CoreError::Store {
                    problem: StoreProblem::NewerVersion { found: 99 }
                })
            ),
            "{result:?}"
        );
        assert_eq!(core.library_memory(), LibraryMemorySettings::default());
        // Nothing to save (every switch is off) is no write, so no error.
        block_on(core.remember_library_choices(library("a"), everything())).unwrap();
    }
}
