//! The global search's filters across the bridge (`current-app`'s rules, ADR 0018, "Global search filters"): the UI lists
//! [`Core::search_tools`]'s choices as given, passes the filter chosen to [`super::Browse::search`], shows [`search_note`] under a
//! group that is full, and keeps the page's choices with [`Core::search_choices`] and [`Core::remember_search_choices`]. Media kinds
//! cross as their open names (`movie`, `series`, `person`).

use crate::{Core, CoreError};

use super::{ItemQuery, ItemRef, LibraryView};

/// Where a search looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Enum)]
pub enum SearchSource {
    /// Every source there is: the library, and the catalog once a Seerr server is set up.
    #[default]
    All,
    /// The signed-in Jellyfin servers' libraries.
    Library,
    /// What can be requested (Seerr).
    Catalog,
}

impl From<current_app::SearchSource> for SearchSource {
    fn from(source: current_app::SearchSource) -> Self {
        match source {
            current_app::SearchSource::All => Self::All,
            current_app::SearchSource::Library => Self::Library,
            current_app::SearchSource::Catalog => Self::Catalog,
        }
    }
}

impl From<SearchSource> for current_app::SearchSource {
    fn from(source: SearchSource) -> Self {
        match source {
            SearchSource::All => Self::All,
            SearchSource::Library => Self::Library,
            SearchSource::Catalog => Self::Catalog,
        }
    }
}

/// The search's filters. The default finds everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct SearchFilter {
    /// One type only (`movie`, `series`, `person`), or `None` for every type.
    pub kind: Option<String>,
    pub source: SearchSource,
}

impl From<&SearchFilter> for current_app::SearchFilter {
    fn from(filter: &SearchFilter) -> Self {
        Self {
            kind: filter.kind.clone().map(current_model::MediaKind::new),
            source: filter.source.into(),
        }
    }
}

impl From<current_app::SearchFilter> for SearchFilter {
    fn from(filter: current_app::SearchFilter) -> Self {
        Self {
            kind: filter.kind.map(|kind| kind.as_str().to_owned()),
            source: filter.source.into(),
        }
    }
}

/// One choice of a filter: its name, whether it is the one in effect (`chosen`), and whether it can be
/// chosen; `tooltip` says why not. An unavailable choice names the reason in `label` too: a disabled menu item takes no input, and may
/// show no tooltip (not verified).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SearchChoice {
    pub label: String,
    pub chosen: bool,
    pub enabled: bool,
    pub tooltip: Option<String>,
}

impl From<current_app::SearchChoice> for SearchChoice {
    fn from(choice: current_app::SearchChoice) -> Self {
        Self {
            label: choice.label,
            chosen: choice.chosen,
            enabled: choice.enabled,
            tooltip: choice.tooltip,
        }
    }
}

/// One Type choice: `kind` is `None` for "All".
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SearchTypeChoice {
    pub kind: Option<String>,
    pub choice: SearchChoice,
}

/// One Source choice.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SearchSourceChoice {
    pub source: SearchSource,
    pub choice: SearchChoice,
}

/// What the search page's Filter offers, in order: the Type choices, then the Source choices.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SearchTools {
    pub types: Vec<SearchTypeChoice>,
    pub sources: Vec<SearchSourceChoice>,
}

impl From<current_app::SearchTools> for SearchTools {
    fn from(tools: current_app::SearchTools) -> Self {
        Self {
            types: tools
                .types
                .into_iter()
                .map(|choice| SearchTypeChoice {
                    kind: choice.kind.map(|kind| kind.as_str().to_owned()),
                    choice: choice.choice.into(),
                })
                .collect(),
            sources: tools
                .sources
                .into_iter()
                .map(|choice| SearchSourceChoice {
                    source: choice.source.into(),
                    choice: choice.choice.into(),
                })
                .collect(),
        }
    }
}

/// The search page's remembered choices, each `None` when there is none: from [`Core::search_choices`], nothing remembered (or its
/// switch is off) and the page opens with everything and its first view; to [`Core::remember_search_choices`], left as it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct SearchChoices {
    pub filter: Option<SearchFilter>,
    pub view: Option<LibraryView>,
}

/// The line under a search group of `count` matches: once it holds as many as the search asks for (24 of each type, or 100 of the
/// one type chosen), that only the best matches show and more text narrows them; `None` otherwise.
#[uniffi::export]
pub fn search_note(filter: SearchFilter, count: u32) -> Option<String> {
    current_app::search_note(&(&filter).into(), count)
}

/// How a group of `kind` on the search page shows with `filter` and the page's View (`wide`, `show_titles`): its title and tiles,
/// whether it wraps, what opening an item does, whether it has the item menu, and its note once full.
#[uniffi::export]
pub fn search_result_display(
    kind: String,
    filter: SearchFilter,
    wide: bool,
    show_titles: bool,
) -> super::RowDisplay {
    current_app::search_result_display(
        &current_model::MediaKind::new(kind),
        &(&filter).into(),
        wide,
        show_titles,
    )
    .into()
}

/// What opening a person shows: the films and series they are credited on, in every library, the newest first. Open it with
/// [`super::Browse::open_items`].
#[uniffi::export]
pub fn person_query(person: ItemRef) -> ItemQuery {
    (&current_app::person_query(&(&person).into())).into()
}

#[uniffi::export]
impl Core {
    /// The search page's Filter choices for `filter`, with the one in effect checked: the catalog is listed, unavailable until a
    /// Seerr server is signed in. Ask again when the capabilities change.
    pub fn search_tools(&self, filter: SearchFilter) -> SearchTools {
        self.app.search_tools(&(&filter).into()).into()
    }

    /// What the search page opens with: its last filter and view, each only while its switch ("Remember filter", "Remember view")
    /// is on.
    pub fn search_choices(&self) -> SearchChoices {
        let memory = self.app.search_choices();
        SearchChoices {
            filter: memory.filter.map(Into::into),
            view: memory.view.map(|view| LibraryView {
                tile: view.tile,
                show_titles: view.titles,
            }),
        }
    }

    /// Remembers the search page's choices: each part given whose switch is on; the other stays as it was. Saved at once on the
    /// core's blocking pool; nothing is written when nothing changed. Not cancellable.
    ///
    /// # Errors
    /// As [`Core::set_remember_sort`].
    pub async fn remember_search_choices(&self, choices: SearchChoices) -> Result<(), CoreError> {
        let memory = current_app::SearchMemory {
            filter: choices.filter.as_ref().map(Into::into),
            view: choices.view.map(|view| current_store::RememberedView {
                tile: view.tile,
                titles: view.show_titles,
            }),
        };
        self.save_settings(move |app| app.remember_search_choices(memory))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::block_on;
    use crate::{SecretStore, open_core};

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
    fn the_choices_cross_with_the_catalog_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let core = open(&temp);
        let tools = core.search_tools(SearchFilter {
            kind: Some("person".into()),
            source: SearchSource::Catalog,
        });
        let types: Vec<_> = tools.types.iter().map(|t| t.kind.as_deref()).collect();
        assert_eq!(types, [None, Some("movie"), Some("series"), Some("person")]);
        assert_eq!(tools.types[3].choice.label, "People");
        assert!(tools.types[3].choice.chosen);
        let catalog = &tools.sources[2];
        assert_eq!(catalog.source, SearchSource::Catalog);
        assert!(!catalog.choice.enabled && !catalog.choice.chosen);
        assert_eq!(catalog.choice.label, current_app::CATALOG_NEEDS_SEERR_LABEL);
        assert_eq!(
            catalog.choice.tooltip.as_deref(),
            Some(current_app::CATALOG_NEEDS_SEERR)
        );
        // Without the catalog, every source is the one in effect.
        assert!(tools.sources[0].choice.chosen);
    }

    #[test]
    fn the_note_and_a_persons_titles_cross() {
        let every = SearchFilter::default();
        assert_eq!(search_note(every.clone(), 23), None);
        assert!(search_note(every, 24).is_some());
        let person = ItemRef {
            server: crate::ServerRef {
                service: "jellyfin".into(),
                key: "1".into(),
            },
            id: "57a2d1e0000000000000000000004001".into(),
        };
        let query = person_query(person.clone());
        assert_eq!(query.person, Some(person));
        assert_eq!(query.kinds, ["movie", "series"]);
        assert_eq!(query.sort, ["premiere_date"]);
        let people = search_result_display("person".into(), SearchFilter::default(), false, true);
        assert!(people.cards && !people.item_menu);
        assert_eq!(people.opens, super::super::ItemOpens::PersonTitles);
        assert_eq!(people.note.map(|note| note.full_at), Some(24));
    }

    #[test]
    fn the_pages_choices_are_remembered_under_their_switches() {
        let temp = tempfile::tempdir().unwrap();
        let core = open(&temp);
        let chosen = SearchChoices {
            filter: Some(SearchFilter {
                kind: Some("series".into()),
                source: SearchSource::Library,
            }),
            view: Some(LibraryView {
                tile: "wide".into(),
                show_titles: false,
            }),
        };
        block_on(core.remember_search_choices(chosen.clone())).unwrap();
        assert_eq!(core.search_choices(), SearchChoices::default());
        block_on(core.set_remember_filter(true)).unwrap();
        block_on(core.set_remember_view(true)).unwrap();
        block_on(core.remember_search_choices(chosen.clone())).unwrap();
        drop(core);
        assert_eq!(open(&temp).search_choices(), chosen);
    }
}
