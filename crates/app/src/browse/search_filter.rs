//! The global search's filters (owner, 2026-10-26 and 2026-10-27; ADR 0018, "Global search filters"): a Type (all of them, or films,
//! series or people only) and a Source (all of them, the library, or the catalog). The UIs list [`search_tools`]'s choices as given:
//! their names, which is checked, and which are unavailable and why. No sort: results stay in the server's relevance order.
//!
//! The catalog comes with Seerr (M4). Without a Seerr server its choice is still listed, unavailable, with the reason in its own name
//! and its tooltip (owner, 2026-10-27: the app is also used without Seerr, and the option should be visible). Without it, All and
//! Library find the same.

use current_jellyfin::{SEARCH_GROUPS, SEARCH_LIMIT};
use current_model::{Capabilities, ItemQuery, ItemRef, MediaKind, ServiceType, SortKey, SortOrder};

/// Where a search looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SearchSource {
    /// Every source there is: the library, and the catalog once a Seerr server is set up.
    #[default]
    All,
    /// The signed-in Jellyfin servers' libraries.
    Library,
    /// What can be requested (Seerr, M4).
    Catalog,
}

impl SearchSource {
    /// Its name in the settings file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Library => "library",
            Self::Catalog => "catalog",
        }
    }

    /// The source `name` names; `None` for one this version does not know.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::All, Self::Library, Self::Catalog]
            .into_iter()
            .find(|source| source.as_str() == name)
    }
}

/// The search's filters: [`SearchFilter::default`] finds everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SearchFilter {
    /// One of [`search_types`], or `None` for all of them.
    pub kind: Option<MediaKind>,
    pub source: SearchSource,
}

/// The types a search can be narrowed to, in the order the Type choices list them after "All" (owner, 2026-10-27). People are every
/// person the server has, actors, directors and writers alike: Jellyfin's search does not tell them apart (ADR 0018).
pub fn search_types() -> Vec<MediaKind> {
    vec![MediaKind::MOVIE, MediaKind::SERIES, MediaKind::PERSON]
}

/// How many matches a search narrowed to one type shows: one request, of which Jellyfin's search considers three times as many
/// candidates (`docs/notes/jellyfin-browse.md`, "Search"). A search of every type asks for [`SEARCH_LIMIT`] of each.
pub const NARROWED_SEARCH_LIMIT: u32 = 100;

/// The Catalog choice's tooltip while no Seerr server is set up.
pub const CATALOG_NEEDS_SEERR: &str = "Needs a Seerr server";
/// The Catalog choice's name while no Seerr server is set up: the reason in the name too, as a disabled menu item shows no tooltip
/// (ADR 0018, "Global search filters").
pub const CATALOG_NEEDS_SEERR_LABEL: &str = "Catalog (needs Seerr)";

/// One choice of a filter: its name, whether it is the one in effect, and whether it can be chosen; `tooltip` says why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchChoice {
    pub label: String,
    pub checked: bool,
    pub enabled: bool,
    pub tooltip: Option<String>,
}

/// One Type choice: `kind` is `None` for "All".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTypeChoice {
    pub kind: Option<MediaKind>,
    pub choice: SearchChoice,
}

/// One Source choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchSourceChoice {
    pub source: SearchSource,
    pub choice: SearchChoice,
}

/// What the search page's Filter offers, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTools {
    pub types: Vec<SearchTypeChoice>,
    pub sources: Vec<SearchSourceChoice>,
}

/// The name of a Type choice: "All", or the type's.
pub fn search_type_label(kind: Option<&MediaKind>) -> String {
    match kind {
        None => "All".to_owned(),
        Some(kind) if *kind == MediaKind::MOVIE => "Movies".to_owned(),
        Some(kind) if *kind == MediaKind::SERIES => "Series".to_owned(),
        Some(kind) if *kind == MediaKind::PERSON => "People".to_owned(),
        Some(kind) => kind.as_str().to_owned(),
    }
}

/// Whether the catalog can be searched: a Seerr server is signed in. None can be yet: M4 adds the Seerr service, and this follows it.
pub fn catalog_available(capabilities: &Capabilities) -> bool {
    capabilities.is_available(&ServiceType::SEERR)
}

/// `filter` as it can apply now: a type the search does not offer (a remembered one from another version) reads as all of them, and
/// the catalog while it is unavailable as every source.
pub fn search_filter_in_effect(capabilities: &Capabilities, filter: &SearchFilter) -> SearchFilter {
    let kind = filter
        .kind
        .clone()
        .filter(|kind| search_types().contains(kind));
    let source = match filter.source {
        SearchSource::Catalog if !catalog_available(capabilities) => SearchSource::All,
        source => source,
    };
    SearchFilter { kind, source }
}

/// The search page's Filter choices for `filter`, with the one in effect checked.
pub fn search_tools(capabilities: &Capabilities, filter: &SearchFilter) -> SearchTools {
    let shown = search_filter_in_effect(capabilities, filter);
    let types = std::iter::once(None)
        .chain(search_types().into_iter().map(Some))
        .map(|kind| SearchTypeChoice {
            choice: SearchChoice {
                label: search_type_label(kind.as_ref()),
                checked: kind == shown.kind,
                enabled: true,
                tooltip: None,
            },
            kind,
        })
        .collect();
    let catalog = catalog_available(capabilities);
    let sources = [
        SearchSource::All,
        SearchSource::Library,
        SearchSource::Catalog,
    ]
    .into_iter()
    .map(|source| {
        let unavailable = source == SearchSource::Catalog && !catalog;
        let label = match source {
            SearchSource::All => "All",
            SearchSource::Library => "Library",
            SearchSource::Catalog if unavailable => CATALOG_NEEDS_SEERR_LABEL,
            SearchSource::Catalog => "Catalog",
        };
        SearchSourceChoice {
            source,
            choice: SearchChoice {
                label: label.to_owned(),
                checked: source == shown.source,
                enabled: !unavailable,
                tooltip: unavailable.then(|| CATALOG_NEEDS_SEERR.to_owned()),
            },
        }
    })
    .collect();
    SearchTools { types, sources }
}

/// What a search with `filter` asks a Jellyfin server for: the kinds, a group each in that order, and how many of each. `None` when
/// the filter leaves the library out (the catalog only).
pub(crate) fn library_groups(filter: &SearchFilter) -> Option<(Vec<MediaKind>, u32)> {
    if filter.source == SearchSource::Catalog {
        return None;
    }
    Some(match &filter.kind {
        Some(kind) => (vec![kind.clone()], NARROWED_SEARCH_LIMIT),
        None => (SEARCH_GROUPS.to_vec(), SEARCH_LIMIT),
    })
}

/// The line a search group with `count` matches shows (owner, 2026-10-27): once it holds as many as the search asks for, the server
/// may have left out what matched less well, so the user is told to narrow the search. `None` otherwise.
pub fn search_note(filter: &SearchFilter, count: u32) -> Option<String> {
    let limit = if filter.kind.is_some() {
        NARROWED_SEARCH_LIMIT
    } else {
        SEARCH_LIMIT
    };
    (count >= limit).then(|| format!("Showing the best {limit} matches. Type more to narrow it."))
}

/// What opening a person shows (owner, 2026-10-27): the films and series they are credited on, in every library, the newest first.
pub fn person_query(person: &ItemRef) -> ItemQuery {
    ItemQuery {
        recursive: true,
        kinds: vec![MediaKind::MOVIE, MediaKind::SERIES],
        sort: vec![SortKey::PREMIERE_DATE],
        order: SortOrder::Descending,
        person: Some(person.clone()),
        ..ItemQuery::default()
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ItemId, Reachability, ServerKey, ServerRef, ServerStatus};

    use super::*;

    fn with_seerr(signed_in: bool) -> Capabilities {
        let mut seerr = ServerStatus::new(ServerRef::new(ServiceType::SEERR, ServerKey::new("2")));
        seerr.signed_in = signed_in;
        seerr.reachability = Reachability::Reachable;
        Capabilities {
            servers: vec![seerr],
        }
    }

    fn labels<T>(choices: &[T], choice: impl Fn(&T) -> &SearchChoice) -> Vec<String> {
        choices.iter().map(|c| choice(c).label.clone()).collect()
    }

    #[test]
    fn the_choices_are_all_then_each_type_and_all_library_catalog() {
        let tools = search_tools(&Capabilities::default(), &SearchFilter::default());
        assert_eq!(
            labels(&tools.types, |t| &t.choice),
            ["All", "Movies", "Series", "People"]
        );
        let kinds: Vec<_> = tools.types.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                None,
                Some(MediaKind::MOVIE),
                Some(MediaKind::SERIES),
                Some(MediaKind::PERSON)
            ]
        );
        assert!(tools.types[0].choice.checked);
        assert!(tools.types.iter().all(|t| t.choice.enabled));
        let sources: Vec<_> = tools.sources.iter().map(|s| s.source).collect();
        assert_eq!(
            sources,
            [
                SearchSource::All,
                SearchSource::Library,
                SearchSource::Catalog
            ]
        );
        assert!(tools.sources[0].choice.checked);
    }

    #[test]
    fn the_catalog_is_listed_but_unavailable_without_a_seerr_server() {
        for capabilities in [Capabilities::default(), with_seerr(false)] {
            let tools = search_tools(&capabilities, &SearchFilter::default());
            let catalog = &tools.sources[2].choice;
            assert!(!catalog.enabled);
            assert_eq!(catalog.label, CATALOG_NEEDS_SEERR_LABEL);
            assert_eq!(catalog.tooltip.as_deref(), Some(CATALOG_NEEDS_SEERR));
            assert!(tools.sources[..2].iter().all(|s| s.choice.enabled));
        }
        let tools = search_tools(&with_seerr(true), &SearchFilter::default());
        let catalog = &tools.sources[2].choice;
        assert!(catalog.enabled);
        assert_eq!(catalog.label, "Catalog");
        assert_eq!(catalog.tooltip, None);
    }

    #[test]
    fn a_filter_that_cannot_apply_reads_as_everything() {
        let catalog = SearchFilter {
            kind: Some(MediaKind::SERIES),
            source: SearchSource::Catalog,
        };
        let shown = search_filter_in_effect(&Capabilities::default(), &catalog);
        assert_eq!(shown.source, SearchSource::All);
        assert_eq!(shown.kind, Some(MediaKind::SERIES));
        let tools = search_tools(&Capabilities::default(), &catalog);
        assert!(tools.sources[0].choice.checked && !tools.sources[2].choice.checked);
        assert!(tools.types[2].choice.checked);
        // With Seerr the catalog stands.
        assert_eq!(
            search_filter_in_effect(&with_seerr(true), &catalog),
            catalog
        );
        // A type the search does not offer (a remembered one from another version).
        let episodes = SearchFilter {
            kind: Some(MediaKind::EPISODE),
            ..SearchFilter::default()
        };
        assert_eq!(
            search_filter_in_effect(&Capabilities::default(), &episodes).kind,
            None
        );
    }

    #[test]
    fn all_and_library_ask_for_every_group_and_a_type_for_its_own() {
        for source in [SearchSource::All, SearchSource::Library] {
            let every = SearchFilter { kind: None, source };
            let (kinds, limit) = library_groups(&every).unwrap();
            assert_eq!(kinds, SEARCH_GROUPS);
            assert_eq!(kinds.last(), Some(&MediaKind::PERSON));
            assert_eq!(limit, SEARCH_LIMIT);
            for kind in search_types() {
                let narrowed = SearchFilter {
                    kind: Some(kind.clone()),
                    source,
                };
                assert_eq!(
                    library_groups(&narrowed),
                    Some((vec![kind], NARROWED_SEARCH_LIMIT))
                );
            }
        }
        let catalog = SearchFilter {
            kind: None,
            source: SearchSource::Catalog,
        };
        assert_eq!(library_groups(&catalog), None);
    }

    #[test]
    fn the_note_shows_once_a_group_is_full() {
        let every = SearchFilter::default();
        assert_eq!(search_note(&every, SEARCH_LIMIT - 1), None);
        assert_eq!(
            search_note(&every, SEARCH_LIMIT).as_deref(),
            Some("Showing the best 24 matches. Type more to narrow it.")
        );
        let films = SearchFilter {
            kind: Some(MediaKind::MOVIE),
            ..SearchFilter::default()
        };
        assert_eq!(search_note(&films, SEARCH_LIMIT), None);
        assert_eq!(
            search_note(&films, NARROWED_SEARCH_LIMIT).as_deref(),
            Some("Showing the best 100 matches. Type more to narrow it.")
        );
    }

    #[test]
    fn sources_have_names_and_unknown_ones_read_as_none() {
        for source in [
            SearchSource::All,
            SearchSource::Library,
            SearchSource::Catalog,
        ] {
            assert_eq!(SearchSource::from_name(source.as_str()), Some(source));
        }
        assert_eq!(SearchSource::from_name("a_newer_source"), None);
    }

    #[test]
    fn a_person_opens_their_films_and_series_newest_first() {
        let person = ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new("57a2d1e0000000000000000000004001"),
        );
        let query = person_query(&person);
        assert_eq!(query.person, Some(person));
        assert_eq!(query.parent, None);
        assert!(query.recursive);
        assert_eq!(query.kinds, [MediaKind::MOVIE, MediaKind::SERIES]);
        assert_eq!(query.sort, [SortKey::PREMIERE_DATE]);
        assert_eq!(query.order, SortOrder::Descending);
        assert!(!query.depends_on_marks());
    }
}
