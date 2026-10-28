//! Settings' "Remember sort", "Remember filter" and "Remember view" (all off by default), and the choices remembered per library: a
//! library opens with the last sort, filter and view chosen there for each part whose switch is on. A part whose switch is off is
//! neither saved nor given back, and turning a switch off forgets that part for every library, so turning it on again starts afresh.
//! A kind's page across a server's libraries (the Movies and Series tabs, owner, 2026-10-28) remembers its own, as a library does.
//! The global search page has choices of its own under the same switches: its filter and its view ([`SearchMemory`]).
//! Saved in `settings.json` (`current-store`'s `browse`, open values) when the app has a data folder, else kept for the run only.

use std::sync::{Mutex, PoisonError};

use current_model::{ItemRef, ServerRef};
use current_store::{
    BrowseSettings, KindPage, LibraryChoices, RememberedSearchFilter, RememberedView,
    SettingsStore, StoreError,
};

use crate::browse::{SearchFilter, SearchSource};

/// The global search page's remembered choices, each `None` when nothing is (or its switch is off): the page then opens with every
/// type and source, and its first view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchMemory {
    /// A remembered source this version does not know reads as every source.
    pub filter: Option<SearchFilter>,
    pub view: Option<RememberedView>,
}

/// The three switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LibraryMemory {
    pub remember_sort: bool,
    pub remember_filter: bool,
    pub remember_view: bool,
}

/// One part of a library's choices, each with its switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Sort,
    Filter,
    View,
}

impl Part {
    fn switch(self, browse: &mut BrowseSettings) -> &mut bool {
        match self {
            Self::Sort => &mut browse.remember_sort,
            Self::Filter => &mut browse.remember_filter,
            Self::View => &mut browse.remember_view,
        }
    }
}

/// A grid whose choices are remembered: a library's, or a kind's page across a server's libraries.
enum Page<'a> {
    Library(&'a ItemRef),
    Kind(&'a KindPage),
}

impl Page<'_> {
    fn get<'b>(&self, browse: &'b BrowseSettings) -> Option<&'b LibraryChoices> {
        match self {
            Self::Library(library) => browse.libraries.get(*library),
            Self::Kind(page) => browse.kinds.get(*page),
        }
    }

    fn insert(&self, browse: &mut BrowseSettings, choices: LibraryChoices) {
        match self {
            Self::Library(library) => browse.libraries.insert((*library).clone(), choices),
            Self::Kind(page) => browse.kinds.insert((*page).clone(), choices),
        };
    }
}

pub(crate) struct LibraryPrefs {
    /// The settings of an app that stores nothing; unused when there is a settings store, which is then the only copy.
    memory: Mutex<BrowseSettings>,
}

impl LibraryPrefs {
    pub(crate) fn new() -> Self {
        Self {
            memory: Mutex::new(BrowseSettings::default()),
        }
    }

    fn read(&self, store: Option<&SettingsStore>) -> BrowseSettings {
        match store {
            Some(store) => store.get().browse,
            None => self.lock().clone(),
        }
    }

    fn change(
        &self,
        store: Option<&SettingsStore>,
        change: impl FnOnce(&mut BrowseSettings),
    ) -> Result<(), StoreError> {
        match store {
            Some(store) => store.update(|settings| change(&mut settings.browse)),
            None => {
                change(&mut self.lock());
                Ok(())
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrowseSettings> {
        // A panic while holding the lock leaves usable settings behind (each change is a few assignments), so poisoning is safe to
        // ignore.
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn switches(&self, store: Option<&SettingsStore>) -> LibraryMemory {
        let browse = self.read(store);
        LibraryMemory {
            remember_sort: browse.remember_sort,
            remember_filter: browse.remember_filter,
            remember_view: browse.remember_view,
        }
    }

    /// Turns a part's switch on or off and saves it; off forgets that part for every library in the same save. An unchanged switch
    /// writes nothing.
    pub(crate) fn set(
        &self,
        store: Option<&SettingsStore>,
        part: Part,
        on: bool,
    ) -> Result<(), StoreError> {
        if *part.switch(&mut self.read(store)) == on {
            return Ok(());
        }
        self.change(store, |browse| {
            *part.switch(browse) = on;
            if !on {
                match part {
                    Part::Sort => browse.forget_sorts(),
                    Part::Filter => browse.forget_filters(),
                    Part::View => browse.forget_views(),
                }
            }
        })
    }

    /// What `library` opens with: the remembered parts whose switch is on.
    pub(crate) fn choices(
        &self,
        store: Option<&SettingsStore>,
        library: &ItemRef,
    ) -> LibraryChoices {
        self.page_choices(store, &Page::Library(library))
    }

    /// What a kind's page opens with, as a library's ([`LibraryPrefs::choices`]).
    pub(crate) fn kind_choices(
        &self,
        store: Option<&SettingsStore>,
        page: &KindPage,
    ) -> LibraryChoices {
        self.page_choices(store, &Page::Kind(page))
    }

    /// Remembers the choices on a kind's page, as a library's ([`LibraryPrefs::remember`]).
    pub(crate) fn remember_kind(
        &self,
        store: Option<&SettingsStore>,
        page: &KindPage,
        choices: &LibraryChoices,
    ) -> Result<(), StoreError> {
        self.remember_page(store, &Page::Kind(page), choices)
    }

    fn page_choices(&self, store: Option<&SettingsStore>, page: &Page<'_>) -> LibraryChoices {
        let browse = self.read(store);
        let Some(remembered) = page.get(&browse) else {
            return LibraryChoices::default();
        };
        LibraryChoices {
            sort: remembered.sort.clone().filter(|_| browse.remember_sort),
            filters: remembered
                .filters
                .clone()
                .filter(|_| browse.remember_filter),
            view: remembered.view.clone().filter(|_| browse.remember_view),
        }
    }

    /// Remembers the parts of `choices` that are given and whose switch is on; the others stay as they were. Writes nothing when
    /// that changes nothing.
    pub(crate) fn remember(
        &self,
        store: Option<&SettingsStore>,
        library: &ItemRef,
        choices: &LibraryChoices,
    ) -> Result<(), StoreError> {
        self.remember_page(store, &Page::Library(library), choices)
    }

    fn remember_page(
        &self,
        store: Option<&SettingsStore>,
        page: &Page<'_>,
        choices: &LibraryChoices,
    ) -> Result<(), StoreError> {
        let apply = |browse: &mut BrowseSettings| {
            let current = page.get(browse).cloned().unwrap_or_default();
            let mut next = current.clone();
            if browse.remember_sort && choices.sort.is_some() {
                next.sort.clone_from(&choices.sort);
            }
            if browse.remember_filter && choices.filters.is_some() {
                next.filters.clone_from(&choices.filters);
            }
            if browse.remember_view && choices.view.is_some() {
                next.view.clone_from(&choices.view);
            }
            if next == current {
                return false;
            }
            page.insert(browse, next);
            true
        };
        // Checked on a copy first, so that nothing to save never writes, nor fails on a file this version must not write.
        if !apply(&mut self.read(store)) {
            return Ok(());
        }
        self.change(store, |browse| {
            apply(browse);
        })
    }

    /// What the search page opens with: the remembered parts whose switch is on.
    pub(crate) fn search(&self, store: Option<&SettingsStore>) -> SearchMemory {
        let browse = self.read(store);
        SearchMemory {
            filter: browse
                .search
                .filter
                .filter(|_| browse.remember_filter)
                .map(|filter| SearchFilter {
                    kind: filter.kind,
                    source: SearchSource::from_name(&filter.source).unwrap_or_default(),
                }),
            view: browse.search.view.filter(|_| browse.remember_view),
        }
    }

    /// Remembers the search page's choices as [`LibraryPrefs::remember`] does a library's.
    pub(crate) fn remember_search(
        &self,
        store: Option<&SettingsStore>,
        choices: &SearchMemory,
    ) -> Result<(), StoreError> {
        let apply = |browse: &mut BrowseSettings| {
            let current = browse.search.clone();
            let mut next = current.clone();
            if browse.remember_filter
                && let Some(filter) = &choices.filter
            {
                // Everything is the default: left out of the file, as ADR 0013 leaves defaults out.
                next.filter =
                    (*filter != SearchFilter::default()).then(|| RememberedSearchFilter {
                        kind: filter.kind.clone(),
                        source: filter.source.as_str().to_owned(),
                    });
            }
            if browse.remember_view && choices.view.is_some() {
                next.view.clone_from(&choices.view);
            }
            if next == current {
                return false;
            }
            browse.search = next;
            true
        };
        if !apply(&mut self.read(store)) {
            return Ok(());
        }
        self.change(store, |browse| {
            apply(browse);
        })
    }

    /// The server was removed: its libraries and kind's pages go. A settings store forgets them in the removal's own save (`Shared::remove_server`);
    /// this is for an app that stores nothing.
    pub(crate) fn server_removed(&self, store: Option<&SettingsStore>, server: &ServerRef) {
        if store.is_none() {
            self.lock().forget_server(server);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use current_model::{ItemFilter, ItemId, ServerKey, ServiceType, SortKey, SortOrder};
    use current_store::{LoadReport, MemorySecrets, RememberedSort, RememberedView, SETTINGS_FILE};

    use crate::{App, Removal};

    use super::*;

    fn library(id: &str) -> ItemRef {
        ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new(id),
        )
    }

    fn everything() -> LibraryChoices {
        LibraryChoices {
            sort: Some(RememberedSort {
                key: SortKey::DATE_CREATED,
                order: SortOrder::Descending,
            }),
            filters: Some(vec![ItemFilter::UNPLAYED]),
            view: Some(RememberedView {
                tile: "wide".into(),
                titles: false,
            }),
        }
    }

    fn open(dir: &std::path::Path) -> App {
        App::open(dir, &dir.join("cache"), Arc::new(MemorySecrets::new()), "")
            .unwrap()
            .0
    }

    fn file(dir: &std::path::Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap()
    }

    fn search_memory() -> SearchMemory {
        SearchMemory {
            filter: Some(SearchFilter {
                kind: Some(current_model::MediaKind::PERSON),
                source: SearchSource::Library,
            }),
            view: Some(RememberedView {
                tile: "wide".into(),
                titles: true,
            }),
        }
    }

    #[test]
    fn the_search_page_remembers_its_filter_and_view_under_their_switches() {
        let temp = tempfile::tempdir().unwrap();
        let app = open(temp.path());
        app.remember_search_choices(search_memory()).unwrap();
        assert_eq!(app.search_choices(), SearchMemory::default());
        assert!(file(temp.path()).get("browse").is_none());

        app.set_remember_filter(true).unwrap();
        app.remember_search_choices(search_memory()).unwrap();
        assert_eq!(
            app.search_choices(),
            SearchMemory {
                view: None,
                ..search_memory()
            }
        );
        assert_eq!(
            file(temp.path())["browse"]["search"],
            serde_json::json!({"filter": {"type": "person", "source": "library"}})
        );
        app.set_remember_view(true).unwrap();
        app.remember_search_choices(search_memory()).unwrap();
        assert_eq!(app.search_choices(), search_memory());
        // Read again from the file.
        drop(app);
        let app = open(temp.path());
        assert_eq!(app.search_choices(), search_memory());
        // Everything is the default: nothing is written for it.
        app.remember_search_choices(SearchMemory {
            filter: Some(SearchFilter::default()),
            view: None,
        })
        .unwrap();
        assert_eq!(app.search_choices().filter, None);
        assert!(
            file(temp.path())["browse"]["search"]
                .get("filter")
                .is_none()
        );
        // Off forgets it.
        app.set_remember_filter(false).unwrap();
        app.set_remember_view(false).unwrap();
        assert_eq!(app.search_choices(), SearchMemory::default());
        assert!(file(temp.path())["browse"].get("search").is_none());
    }

    #[test]
    fn a_remembered_source_this_version_does_not_know_reads_as_every_source() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(SETTINGS_FILE),
            r#"{"version": 1, "browse": {"remember_filter": true, "search": {"filter": {"type": "series", "source": "a_newer_source"}}}}"#,
        )
        .unwrap();
        let app = open(temp.path());
        let filter = app.search_choices().filter.unwrap();
        assert_eq!(filter.kind, Some(current_model::MediaKind::SERIES));
        assert_eq!(filter.source, SearchSource::All);
    }

    #[test]
    fn nothing_is_remembered_while_the_switches_are_off() {
        let temp = tempfile::tempdir().unwrap();
        let app = open(temp.path());
        assert_eq!(app.library_memory(), LibraryMemory::default());
        app.remember_library_choices(&library("a"), everything())
            .unwrap();
        assert_eq!(
            app.library_choices(&library("a")),
            LibraryChoices::default()
        );
        assert!(file(temp.path()).get("browse").is_none());
    }

    #[test]
    fn only_the_parts_switched_on_are_saved_and_given_back() {
        let temp = tempfile::tempdir().unwrap();
        let app = open(temp.path());
        app.set_remember_sort(true).unwrap();
        app.set_remember_view(true).unwrap();
        app.remember_library_choices(&library("a"), everything())
            .unwrap();
        let expected = LibraryChoices {
            filters: None,
            ..everything()
        };
        assert_eq!(app.library_choices(&library("a")), expected);
        // Another library has its own choices: none yet.
        assert_eq!(
            app.library_choices(&library("b")),
            LibraryChoices::default()
        );
        // A part not given stays as it was.
        app.remember_library_choices(
            &library("a"),
            LibraryChoices {
                view: Some(RememberedView {
                    tile: "poster".into(),
                    titles: true,
                }),
                ..LibraryChoices::default()
            },
        )
        .unwrap();
        let choices = app.library_choices(&library("a"));
        assert_eq!(choices.sort, expected.sort);
        assert_eq!(choices.view.unwrap().tile, "poster");
        drop(app);

        // Kept for the next start.
        let app = open(temp.path());
        assert_eq!(
            app.library_memory(),
            LibraryMemory {
                remember_sort: true,
                remember_filter: false,
                remember_view: true
            }
        );
        assert_eq!(app.library_choices(&library("a")).sort, expected.sort);
        let json = file(temp.path());
        assert!(
            json["browse"]["libraries"]["jellyfin/1/a"]
                .get("filter")
                .is_none()
        );
    }

    #[test]
    fn turning_a_switch_off_forgets_that_part_for_every_library() {
        let temp = tempfile::tempdir().unwrap();
        let app = open(temp.path());
        app.set_remember_sort(true).unwrap();
        app.set_remember_filter(true).unwrap();
        for id in ["a", "b"] {
            app.remember_library_choices(&library(id), everything())
                .unwrap();
        }
        app.set_remember_sort(false).unwrap();
        assert_eq!(app.library_choices(&library("a")).sort, None);
        // On again: it starts afresh.
        app.set_remember_sort(true).unwrap();
        for id in ["a", "b"] {
            let choices = app.library_choices(&library(id));
            assert_eq!(choices.sort, None);
            assert_eq!(choices.filters, everything().filters);
        }
        let json = file(temp.path());
        assert!(
            json["browse"]["libraries"]["jellyfin/1/b"]
                .get("sort")
                .is_none()
        );
        assert_eq!(json["browse"]["remember_sort"], true);
    }

    #[test]
    fn nothing_changed_writes_nothing() {
        // A newer version's file refuses every write: what reaches it fails.
        let temp = tempfile::tempdir().unwrap();
        let text = r#"{"version": 99, "device_id": "abc", "browse": {"remember_sort": true, "libraries": {"jellyfin/1/a": {"sort": {"key": "date_created", "order": "descending"}}}}}"#;
        std::fs::write(temp.path().join(SETTINGS_FILE), text).unwrap();
        let (app, report) = App::open(
            temp.path(),
            &temp.path().join("cache"),
            Arc::new(MemorySecrets::new()),
            "",
        )
        .unwrap();
        assert_eq!(report, LoadReport::NewerVersion { found: 99 });
        // The same sort again, a part whose switch is off, and the switch as it is: nothing to save.
        app.remember_library_choices(&library("a"), everything())
            .unwrap();
        app.set_remember_sort(true).unwrap();
        app.set_remember_view(false).unwrap();
        // A change is refused.
        let other = LibraryChoices {
            sort: Some(RememberedSort {
                key: SortKey::SORT_NAME,
                order: SortOrder::Ascending,
            }),
            ..LibraryChoices::default()
        };
        assert_eq!(
            app.remember_library_choices(&library("a"), other),
            Err(StoreError::NewerVersion { found: 99 })
        );
        assert_eq!(
            app.set_remember_filter(true),
            Err(StoreError::NewerVersion { found: 99 })
        );
        assert_eq!(app.library_choices(&library("a")).sort, everything().sort);
        assert_eq!(
            std::fs::read_to_string(temp.path().join(SETTINGS_FILE)).unwrap(),
            text
        );
    }

    #[test]
    fn an_app_that_stores_nothing_remembers_for_the_run() {
        let app = App::new();
        app.set_remember_filter(true).unwrap();
        app.remember_library_choices(&library("a"), everything())
            .unwrap();
        assert_eq!(
            app.library_choices(&library("a")),
            LibraryChoices {
                filters: everything().filters,
                ..LibraryChoices::default()
            }
        );
        app.set_remember_filter(false).unwrap();
        app.set_remember_filter(true).unwrap();
        assert_eq!(
            app.library_choices(&library("a")),
            LibraryChoices::default()
        );
    }

    #[test]
    fn a_library_opens_with_its_remembered_choices_unless_a_home_header_opened_it() {
        let app = App::new();
        app.set_remember_sort(true).unwrap();
        app.set_remember_filter(true).unwrap();
        app.set_remember_view(true).unwrap();
        app.remember_library_choices(&library("a"), everything())
            .unwrap();
        let films = current_model::Library {
            item: library("a"),
            name: "Films".into(),
            collection: Some(current_model::CollectionKind::MOVIES),
            artwork: Vec::new(),
        };
        let opening = app.library_opening(&films, None);
        assert_eq!(opening.query.sort, vec![SortKey::DATE_CREATED]);
        assert_eq!(opening.query.filters, vec![ItemFilter::UNPLAYED]);
        assert!(opening.wide);
        assert!(!opening.show_titles);
        let home = crate::recently_added_query(&films);
        let opening = app.library_opening(&films, Some(home.clone()));
        assert_eq!(opening.query, home);
        assert!(!opening.wide);
        assert!(opening.show_titles);
    }

    #[test]
    fn a_kind_tab_remembers_its_own_choices_under_the_switches() {
        use crate::BrowseView;
        use current_model::MediaKind;
        let temp = tempfile::tempdir().unwrap();
        let app = open(temp.path());
        let server = library("a").server;
        let movies = Some(&MediaKind::MOVIE);
        // Switches off: nothing is saved.
        app.remember_view_choices(&server, BrowseView::Everything, movies, everything())
            .unwrap();
        assert_eq!(
            app.view_choices(&server, BrowseView::Everything, movies),
            LibraryChoices::default()
        );
        app.set_remember_sort(true).unwrap();
        app.set_remember_view(true).unwrap();
        app.remember_view_choices(&server, BrowseView::Everything, movies, everything())
            .unwrap();
        let remembered = app.view_choices(&server, BrowseView::Everything, movies);
        assert_eq!(remembered.sort, everything().sort);
        assert_eq!(remembered.filters, None);
        assert_eq!(remembered.view, everything().view);
        assert_eq!(
            file(temp.path())["browse"]["kinds"]["jellyfin/1/movie"]["sort"]["key"],
            "date_created"
        );
        // Its own: not the Series tab's, nor a library's.
        assert_eq!(
            app.view_choices(&server, BrowseView::Everything, Some(&MediaKind::SERIES)),
            LibraryChoices::default()
        );
        assert_eq!(
            app.library_choices(&library("a")),
            LibraryChoices::default()
        );
        // Opened from its tab with them; from a Favourites row's header without.
        let opening = app.view_opening(&server, BrowseView::Everything, movies, false);
        assert_eq!(opening.query.sort, vec![SortKey::DATE_CREATED]);
        assert!(opening.wide);
        let opening = app.view_opening(&server, BrowseView::Everything, movies, true);
        assert_eq!(
            opening.query,
            crate::favourites_query(Some(MediaKind::MOVIE))
        );
        assert!(!opening.wide);
        // Views without a tab remember nothing, and saving their choices writes nothing.
        let before = file(temp.path());
        for (view, kind) in [
            (BrowseView::Everything, Some(&MediaKind::EPISODE)),
            (BrowseView::RecentlyAdded, None),
        ] {
            app.remember_view_choices(&server, view, kind, everything())
                .unwrap();
            assert_eq!(
                app.view_choices(&server, view, kind),
                LibraryChoices::default()
            );
        }
        assert_eq!(file(temp.path()), before);
        // A switch turned off forgets that part on the tab too.
        app.set_remember_sort(false).unwrap();
        app.set_remember_sort(true).unwrap();
        assert_eq!(
            app.view_choices(&server, BrowseView::Everything, movies)
                .sort,
            None
        );
    }

    #[test]
    fn numbers_use_the_decimal_separator_the_ui_passed() {
        let app = App::new();
        assert_eq!(app.number_format().decimal_separator, ".");
        app.set_decimal_separator(",");
        assert_eq!(
            crate::bitrate_label(26_100_000, &app.number_format()),
            "26,1 Mbit/s"
        );
        app.set_decimal_separator("");
        assert_eq!(app.number_format().decimal_separator, ",");
    }

    #[tokio::test]
    async fn removing_a_server_forgets_its_libraries_and_kind_pages() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(SETTINGS_FILE),
            r#"{"version": 1, "device_id": "abc", "servers": [{"service": "jellyfin", "key": "1"}, {"service": "jellyfin", "key": "2"}],
                "browse": {"remember_filter": true, "libraries": {"jellyfin/1/a": {"filter": []}, "jellyfin/2/a": {"filter": []}},
                "kinds": {"jellyfin/1/movie": {"filter": []}, "jellyfin/2/movie": {"filter": []}}}}"#,
        )
        .unwrap();
        let app = open(temp.path());
        let other = ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("2")),
            ItemId::new("a"),
        );
        assert_eq!(app.library_choices(&library("a")).filters, Some(Vec::new()));
        let removed = app.remove_server(&library("a").server).await.unwrap();
        assert!(matches!(removed, Removal::Removed { .. }));
        assert_eq!(
            app.library_choices(&library("a")),
            LibraryChoices::default()
        );
        assert_eq!(app.library_choices(&other).filters, Some(Vec::new()));
        let json = file(temp.path());
        assert!(json["browse"]["kinds"].get("jellyfin/1/movie").is_none());
        assert!(json["browse"]["kinds"].get("jellyfin/2/movie").is_some());
        assert!(json["browse"]["libraries"].get("jellyfin/1/a").is_none());
        assert!(json["browse"]["libraries"].get("jellyfin/2/a").is_some());
    }
}
