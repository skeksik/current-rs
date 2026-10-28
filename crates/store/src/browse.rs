//! The browse settings in `settings.json`: Settings' three switches "Remember sort", "Remember filter" and "Remember view" (all off by
//! default) and, per library, the last choice made there for each part whose switch is on. All open values (ADR 0013), as
//! `playback`: a value this version cannot read reads as the default (off, nothing remembered) and is written back unchanged until it
//! is changed here, and fields this version does not know are kept, in the object and in each library's entry.
//!
//! The file keeps them under `browse`, each left out at its default:
//!
//! ```json
//! "browse": {
//!   "remember_sort": true, "remember_filter": true, "remember_view": true,
//!   "libraries": {
//!     "jellyfin/1/<library id>": {
//!       "sort": {"key": "date_created", "order": "descending"},
//!       "filter": ["unplayed"],
//!       "view": {"tile": "wide", "titles": false}
//!     }
//!   }
//! }
//! ```
//!
//! A library's key is its item reference as `service/server key/item id`: the server part is how [`ServerRef`] displays, which holds
//! no address, and neither the service nor Current's server keys contain `/`. An empty `filter` list is a choice ("All"); a missing
//! one is none remembered.
//!
//! A kind's page across every library of a server (the Movies and Series tabs, owner, 2026-10-28) has an entry of its own under
//! `kinds`, of the same shape, keyed `service/server key/kind` with the kind's open name (Jellyfin's kinds hold no `/`):
//!
//! ```json
//! "browse": {
//!   "kinds": {"jellyfin/1/movie": {"sort": {"key": "date_created", "order": "descending"}}}
//! }
//! ```
//!
//! The global search page has one entry of its own (owner, 2026-10-27), not per server, under the same switches: its filter while
//! "Remember filter" is on and its view while "Remember view" is. Its type is `all` or a media kind's name, its source `all`, `library`,
//! `catalog` or one a later version names: open values.
//!
//! ```json
//! "browse": {
//!   "search": {"filter": {"type": "movie", "source": "library"}, "view": {"tile": "wide", "titles": true}}
//! }
//! ```

use std::collections::BTreeMap;

use current_model::{
    ItemFilter, ItemId, ItemRef, MediaKind, ServerKey, ServerRef, ServiceType, SortKey, SortOrder,
};
use serde_json::{Map, Value};

const REMEMBER_SORT: &str = "remember_sort";
const REMEMBER_FILTER: &str = "remember_filter";
const REMEMBER_VIEW: &str = "remember_view";
const LIBRARIES: &str = "libraries";
const KINDS: &str = "kinds";
const SEARCH: &str = "search";

const SORT: &str = "sort";
const FILTER: &str = "filter";
const VIEW: &str = "view";

const SORT_KEY: &str = "key";
const SORT_ORDER: &str = "order";
const ASCENDING: &str = "ascending";
const DESCENDING: &str = "descending";
const VIEW_TILE: &str = "tile";
const VIEW_TITLES: &str = "titles";
const SEARCH_TYPE: &str = "type";
const SEARCH_SOURCE: &str = "source";
/// The search type that is every type.
const ALL: &str = "all";

/// A library grid's sort: its key and the order it goes in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedSort {
    pub key: SortKey,
    pub order: SortOrder,
}

/// A library grid's view: the tiles' shape and whether their titles show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedView {
    /// `poster` or `wide`, or a shape a later version names: an open value.
    pub tile: String,
    pub titles: bool,
}

/// What is remembered for one library, each part `None` when nothing is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryChoices {
    pub sort: Option<RememberedSort>,
    /// The filters every listed item meets; empty for "All".
    pub filters: Option<Vec<ItemFilter>>,
    pub view: Option<RememberedView>,
}

impl LibraryChoices {
    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.sort.is_none() && self.filters.is_none() && self.view.is_none()
    }

    fn read(entry: &Map<String, Value>) -> Self {
        Self {
            sort: entry.get(SORT).and_then(read_sort),
            filters: entry.get(FILTER).and_then(read_filters),
            view: entry.get(VIEW).and_then(read_view),
        }
    }

    /// Writes each part into the stored `entry`: a stored part this version cannot read stays as it was.
    fn write_into(&self, entry: &mut Map<String, Value>) {
        set(entry, SORT, read_sort, self.sort.as_ref().map(write_sort));
        set(
            entry,
            FILTER,
            read_filters,
            self.filters.as_ref().map(|filters| {
                Value::from(
                    filters
                        .iter()
                        .map(|filter| filter.as_str().to_owned())
                        .collect::<Vec<_>>(),
                )
            }),
        );
        set(entry, VIEW, read_view, self.view.as_ref().map(write_view));
    }
}

/// The global search page's filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedSearchFilter {
    /// One type only, or `None` for every type.
    pub kind: Option<MediaKind>,
    /// `all`, `library` or `catalog`, or a source a later version names: an open value.
    pub source: String,
}

/// What is remembered for the global search page, each part `None` when nothing is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchChoices {
    pub filter: Option<RememberedSearchFilter>,
    pub view: Option<RememberedView>,
}

impl SearchChoices {
    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.filter.is_none() && self.view.is_none()
    }

    fn read(entry: &Map<String, Value>) -> Self {
        Self {
            filter: entry.get(FILTER).and_then(read_search_filter),
            view: entry.get(VIEW).and_then(read_view),
        }
    }

    /// Writes each part into the stored `entry`, as [`LibraryChoices`] does.
    fn write_into(&self, entry: &mut Map<String, Value>) {
        set(
            entry,
            FILTER,
            read_search_filter,
            self.filter.as_ref().map(write_search_filter),
        );
        set(entry, VIEW, read_view, self.view.as_ref().map(write_view));
    }
}

fn read_search_filter(value: &Value) -> Option<RememberedSearchFilter> {
    let kind = value
        .get(SEARCH_TYPE)?
        .as_str()
        .filter(|kind| !kind.is_empty())?;
    let source = value
        .get(SEARCH_SOURCE)?
        .as_str()
        .filter(|source| !source.is_empty())?;
    Some(RememberedSearchFilter {
        kind: (kind != ALL).then(|| MediaKind::new(kind)),
        source: source.to_owned(),
    })
}

fn write_search_filter(filter: &RememberedSearchFilter) -> Value {
    let kind = filter.kind.as_ref().map_or(ALL, MediaKind::as_str);
    serde_json::json!({ SEARCH_TYPE: kind, SEARCH_SOURCE: filter.source })
}

/// Sets `key` in `object` to `value` (removes it for `None`), unless what is stored there cannot be read by `read`: then it stays.
fn set<T>(
    object: &mut Map<String, Value>,
    key: &str,
    read: impl Fn(&Value) -> Option<T>,
    value: Option<Value>,
) {
    if object.get(key).is_some_and(|stored| read(stored).is_none()) {
        return;
    }
    match value {
        Some(value) => object.insert(key.to_owned(), value),
        None => object.remove(key),
    };
}

fn read_sort(value: &Value) -> Option<RememberedSort> {
    let key = value
        .get(SORT_KEY)?
        .as_str()
        .filter(|key| !key.is_empty())?;
    let order = match value.get(SORT_ORDER)?.as_str()? {
        ASCENDING => SortOrder::Ascending,
        DESCENDING => SortOrder::Descending,
        _ => return None,
    };
    Some(RememberedSort {
        key: SortKey::new(key),
        order,
    })
}

fn write_sort(sort: &RememberedSort) -> Value {
    let order = match sort.order {
        SortOrder::Ascending => ASCENDING,
        SortOrder::Descending => DESCENDING,
    };
    serde_json::json!({ SORT_KEY: sort.key.as_str(), SORT_ORDER: order })
}

fn read_filters(value: &Value) -> Option<Vec<ItemFilter>> {
    value
        .as_array()?
        .iter()
        .map(|filter| filter.as_str().map(ItemFilter::new))
        .collect()
}

fn read_view(value: &Value) -> Option<RememberedView> {
    let tile = value
        .get(VIEW_TILE)?
        .as_str()
        .filter(|tile| !tile.is_empty())?;
    let titles = value.get(VIEW_TITLES)?.as_bool()?;
    Some(RememberedView {
        tile: tile.to_owned(),
        titles,
    })
}

fn write_view(view: &RememberedView) -> Value {
    serde_json::json!({ VIEW_TILE: view.tile, VIEW_TITLES: view.titles })
}

/// A page of every item of one kind across a server's libraries (the Movies and Series tabs, owner, 2026-10-28): what its choices are
/// remembered under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KindPage {
    pub server: ServerRef,
    pub kind: MediaKind,
}

impl KindPage {
    pub fn new(server: ServerRef, kind: MediaKind) -> Self {
        Self { server, kind }
    }
}

/// What an entry under `libraries` or `kinds` is keyed by: written as `service/server key/rest`, read back the same way.
trait EntryKey: Ord + Clone {
    fn key(&self) -> String;
    /// The key's value; `None` for a key this version cannot read.
    fn parse(key: &str) -> Option<Self>;
    fn server(&self) -> &ServerRef;
}

impl EntryKey for ItemRef {
    fn key(&self) -> String {
        format!("{}/{}", self.server, self.id)
    }

    fn parse(key: &str) -> Option<Self> {
        let (server, id) = parse_key(key)?;
        Some(ItemRef::new(server, ItemId::new(id)))
    }

    fn server(&self) -> &ServerRef {
        &self.server
    }
}

impl EntryKey for KindPage {
    fn key(&self) -> String {
        format!("{}/{}", self.server, self.kind.as_str())
    }

    fn parse(key: &str) -> Option<Self> {
        let (server, kind) = parse_key(key)?;
        Some(KindPage::new(server, MediaKind::new(kind)))
    }

    fn server(&self) -> &ServerRef {
        &self.server
    }
}

/// The server and the rest of a key, `service/server key/rest`; `None` for a key this version cannot read.
pub(crate) fn parse_key(key: &str) -> Option<(ServerRef, &str)> {
    let mut parts = key.splitn(3, '/');
    let (service, server, rest) = (parts.next()?, parts.next()?, parts.next()?);
    if service.is_empty() || server.is_empty() || rest.is_empty() {
        return None;
    }
    Some((
        ServerRef::new(ServiceType::new(service), ServerKey::new(server)),
        rest,
    ))
}

/// The entries stored under `section`, keyed by what their keys name; an entry this version cannot read is left out.
fn read_entries<K: EntryKey>(
    stored: &Map<String, Value>,
    section: &str,
) -> BTreeMap<K, LibraryChoices> {
    match stored.get(section) {
        Some(Value::Object(entries)) => entries
            .iter()
            .filter_map(|(key, entry)| {
                let key = K::parse(key)?;
                let Value::Object(entry) = entry else {
                    return None;
                };
                Some((key, LibraryChoices::read(entry)))
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// Forgets `part` of every entry under `section`, also one this version could not read.
fn forget_part_in<K>(
    entries: &mut BTreeMap<K, LibraryChoices>,
    stored: &mut Map<String, Value>,
    section: &str,
    part: &str,
    forget: impl Fn(&mut LibraryChoices),
) {
    entries.values_mut().for_each(forget);
    if let Some(Value::Object(stored)) = stored.get_mut(section) {
        for entry in stored.values_mut() {
            if let Value::Object(entry) = entry {
                entry.remove(part);
            }
        }
    }
}

/// Forgets every entry of `server` under `section`, also entries this version could not read.
fn forget_server_in<K: EntryKey>(
    entries: &mut BTreeMap<K, LibraryChoices>,
    stored: &mut Map<String, Value>,
    section: &str,
    server: &ServerRef,
) {
    entries.retain(|key, _| key.server() != server);
    if let Some(Value::Object(stored)) = stored.get_mut(section) {
        stored.retain(|key, _| K::parse(key).is_none_or(|key| key.server() != server));
    }
}

/// Forgets the stored value of each entry part under `section` that differs between `now` and `before`.
fn changed_in<K: EntryKey>(
    stored: &mut Map<String, Value>,
    section: &str,
    now: &BTreeMap<K, LibraryChoices>,
    before: &BTreeMap<K, LibraryChoices>,
) {
    if now == before {
        return;
    }
    let entries = match stored.get_mut(section) {
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            // A section this version could not read gives way to the entries remembered here.
            stored.remove(section);
            return;
        }
        None => return,
    };
    for (key, then) in before {
        let text = key.key();
        let Some(now) = now.get(key) else {
            entries.remove(&text);
            continue;
        };
        if let Some(Value::Object(entry)) = entries.get_mut(&text) {
            forget_changed(entry, now, then);
        }
    }
    for key in now.keys() {
        if !before.contains_key(key) {
            // New here: anything stored under its key could not be read, and gives way.
            entries.remove(&key.key());
        }
    }
}

/// Writes `entries` under `section` into `object`, over what was stored there.
fn write_entries<K: EntryKey>(
    object: &mut Map<String, Value>,
    section: &str,
    entries: &BTreeMap<K, LibraryChoices>,
) {
    match object.remove(section) {
        // A section this version could not read, and none remembered here since: kept as it was.
        Some(unreadable) if !unreadable.is_object() => {
            object.insert(section.to_owned(), unreadable);
        }
        stored => {
            let mut stored = match stored {
                Some(Value::Object(stored)) => stored,
                _ => Map::new(),
            };
            for (key, choices) in entries {
                let key = key.key();
                let mut entry = match stored.remove(&key) {
                    Some(Value::Object(entry)) => entry,
                    _ => Map::new(),
                };
                choices.write_into(&mut entry);
                if !entry.is_empty() {
                    stored.insert(key, Value::Object(entry));
                }
            }
            if !stored.is_empty() {
                object.insert(section.to_owned(), Value::Object(stored));
            }
        }
    }
}

/// The browse settings. All off, nothing remembered by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowseSettings {
    pub remember_sort: bool,
    pub remember_filter: bool,
    pub remember_view: bool,
    /// What is remembered per library. A library whose entry this version cannot read is not listed here and is kept in the file.
    pub libraries: BTreeMap<ItemRef, LibraryChoices>,
    /// What is remembered per kind's page of a server (the Movies and Series tabs, owner, 2026-10-28), as per library.
    pub kinds: BTreeMap<KindPage, LibraryChoices>,
    /// What is remembered for the global search page. An entry this version cannot read reads as nothing and is kept in the file.
    pub search: SearchChoices,
    /// The stored object as read: fields this version does not know, known fields whose value it could not read, and the libraries'
    /// entries as stored.
    stored: Map<String, Value>,
}

impl BrowseSettings {
    /// The settings from the stored `browse` value; anything but an object reads as the defaults and is kept.
    pub(crate) fn read(value: Option<Value>) -> (Self, Option<Value>) {
        let Some(value) = value else {
            return (Self::default(), None);
        };
        let Value::Object(stored) = value else {
            return (Self::default(), Some(value));
        };
        let switch = |key: &str| stored.get(key).and_then(Value::as_bool).unwrap_or(false);
        let libraries = read_entries(&stored, LIBRARIES);
        let kinds = read_entries(&stored, KINDS);
        let search = match stored.get(SEARCH) {
            Some(Value::Object(entry)) => SearchChoices::read(entry),
            _ => SearchChoices::default(),
        };
        (
            Self {
                remember_sort: switch(REMEMBER_SORT),
                remember_filter: switch(REMEMBER_FILTER),
                remember_view: switch(REMEMBER_VIEW),
                libraries,
                kinds,
                search,
                stored,
            },
            None,
        )
    }

    /// Forgets the sort of every library and kind's page, also one this version could not read.
    pub fn forget_sorts(&mut self) {
        self.forget_part(SORT, |choices| choices.sort = None);
    }

    /// Forgets the filters of every library, every kind's page and the search page, also ones this version could not read.
    pub fn forget_filters(&mut self) {
        self.forget_part(FILTER, |choices| choices.filters = None);
        self.search.filter = None;
        self.forget_search_part(FILTER);
    }

    /// Forgets the view of every library, every kind's page and the search page, also one this version could not read.
    pub fn forget_views(&mut self) {
        self.forget_part(VIEW, |choices| choices.view = None);
        self.search.view = None;
        self.forget_search_part(VIEW);
    }

    fn forget_search_part(&mut self, part: &str) {
        if let Some(Value::Object(entry)) = self.stored.get_mut(SEARCH) {
            entry.remove(part);
        }
    }

    /// Forgets every library and kind's page of `server`, also entries this version could not read: the server was removed.
    pub fn forget_server(&mut self, server: &ServerRef) {
        forget_server_in(&mut self.libraries, &mut self.stored, LIBRARIES, server);
        forget_server_in(&mut self.kinds, &mut self.stored, KINDS, server);
    }

    fn forget_part(&mut self, part: &str, forget: impl Fn(&mut LibraryChoices)) {
        forget_part_in(
            &mut self.libraries,
            &mut self.stored,
            LIBRARIES,
            part,
            &forget,
        );
        forget_part_in(&mut self.kinds, &mut self.stored, KINDS, part, &forget);
    }

    /// Forgets the stored value of each setting that differs from `before`: it was changed here, so what was read no longer stands.
    pub(crate) fn changed_from(&mut self, before: &Self) {
        for (key, now, then) in [
            (REMEMBER_SORT, self.remember_sort, before.remember_sort),
            (
                REMEMBER_FILTER,
                self.remember_filter,
                before.remember_filter,
            ),
            (REMEMBER_VIEW, self.remember_view, before.remember_view),
        ] {
            if now != then {
                self.stored.remove(key);
            }
        }
        if self.search != before.search {
            match self.stored.get_mut(SEARCH) {
                Some(Value::Object(entry)) => {
                    if self.search.filter != before.search.filter {
                        entry.remove(FILTER);
                    }
                    if self.search.view != before.search.view {
                        entry.remove(VIEW);
                    }
                }
                // A `search` this version could not read gives way to what is remembered here.
                Some(_) => {
                    self.stored.remove(SEARCH);
                }
                None => {}
            }
        }
        changed_in(
            &mut self.stored,
            LIBRARIES,
            &self.libraries,
            &before.libraries,
        );
        changed_in(&mut self.stored, KINDS, &self.kinds, &before.kinds);
    }

    /// The value to store: what was read, with each known setting written where it reads as something here or was changed here.
    /// `None` when nothing is left (all defaults).
    pub(crate) fn write(&self) -> Option<Value> {
        let mut object = self.stored.clone();
        for (key, on) in [
            (REMEMBER_SORT, self.remember_sort),
            (REMEMBER_FILTER, self.remember_filter),
            (REMEMBER_VIEW, self.remember_view),
        ] {
            set(
                &mut object,
                key,
                Value::as_bool,
                on.then(|| Value::from(true)),
            );
        }
        write_entries(&mut object, LIBRARIES, &self.libraries);
        write_entries(&mut object, KINDS, &self.kinds);
        match object.remove(SEARCH) {
            // A `search` this version could not read, and nothing remembered here since: kept as it was.
            Some(unreadable) if !unreadable.is_object() => {
                object.insert(SEARCH.to_owned(), unreadable);
            }
            stored => {
                let mut entry = match stored {
                    Some(Value::Object(entry)) => entry,
                    _ => Map::new(),
                };
                self.search.write_into(&mut entry);
                if !entry.is_empty() {
                    object.insert(SEARCH.to_owned(), Value::Object(entry));
                }
            }
        }
        (!object.is_empty()).then_some(Value::Object(object))
    }
}

/// Removes from a library's stored `entry` each part that differs between `now` and `then`.
fn forget_changed(entry: &mut Map<String, Value>, now: &LibraryChoices, then: &LibraryChoices) {
    if now.sort != then.sort {
        entry.remove(SORT);
    }
    if now.filters != then.filters {
        entry.remove(FILTER);
    }
    if now.view != then.view {
        entry.remove(VIEW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> BrowseSettings {
        let (settings, kept) = BrowseSettings::read(Some(serde_json::from_str(text).unwrap()));
        assert!(kept.is_none());
        settings
    }

    fn library(server: &str, id: &str) -> ItemRef {
        ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(server)),
            ItemId::new(id),
        )
    }

    fn all_choices() -> LibraryChoices {
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

    #[test]
    fn values_round_trip_and_defaults_are_left_out() {
        let mut settings = BrowseSettings {
            remember_sort: true,
            remember_filter: true,
            remember_view: true,
            ..BrowseSettings::default()
        };
        settings
            .libraries
            .insert(library("1", "abc"), all_choices());
        // "All" is a choice too: an empty list, written.
        settings.libraries.insert(
            library("2", "def"),
            LibraryChoices {
                filters: Some(Vec::new()),
                ..LibraryChoices::default()
            },
        );
        let written = settings.write().unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "remember_sort": true, "remember_filter": true, "remember_view": true,
                "libraries": {
                    "jellyfin/1/abc": {
                        "sort": {"key": "date_created", "order": "descending"},
                        "filter": ["unplayed"],
                        "view": {"tile": "wide", "titles": false}
                    },
                    "jellyfin/2/def": {"filter": []}
                }
            })
        );
        let (reread, _) = BrowseSettings::read(Some(written));
        assert_eq!(reread.libraries, settings.libraries);
        assert!(reread.remember_sort && reread.remember_filter && reread.remember_view);

        // Back to the defaults: nothing is written.
        let mut cleared = reread.clone();
        cleared.remember_sort = false;
        cleared.remember_filter = false;
        cleared.remember_view = false;
        cleared.libraries.clear();
        cleared.changed_from(&reread);
        assert_eq!(cleared.write(), None);
        assert_eq!(BrowseSettings::default().write(), None);
        // A library with nothing remembered is left out.
        let mut empty = BrowseSettings::default();
        empty
            .libraries
            .insert(library("1", "abc"), LibraryChoices::default());
        assert_eq!(empty.write(), None);
    }

    #[test]
    fn open_values_pass_through() {
        let settings = read(
            r#"{"libraries": {"jellyfin/1/a/b": {"sort": {"key": "a_newer_key", "order": "ascending"}, "filter": ["likes"],
                "view": {"tile": "a_newer_shape", "titles": true}}}}"#,
        );
        let choices = &settings.libraries[&library("1", "a/b")];
        assert_eq!(
            choices.sort.as_ref().unwrap().key,
            SortKey::new("a_newer_key")
        );
        assert_eq!(choices.filters, Some(vec![ItemFilter::new("likes")]));
        assert_eq!(choices.view.as_ref().unwrap().tile, "a_newer_shape");
        assert_eq!(
            settings.write().unwrap()["libraries"]["jellyfin/1/a/b"]["sort"]["key"],
            "a_newer_key"
        );
    }

    #[test]
    fn unreadable_values_and_unknown_fields_are_kept_until_changed_here() {
        let settings = read(
            r#"{"remember_sort": "yes", "remember_view": true, "later": 3, "libraries": {
                "jellyfin/1/abc": {"sort": {"key": "sort_name", "order": "sideways"}, "filter": "played", "view": {"tile": "wide", "titles": true}, "zoom": 2},
                "not a key": {"sort": 1},
                "jellyfin/1/def": 7
            }}"#,
        );
        assert!(!settings.remember_sort && settings.remember_view);
        assert_eq!(settings.libraries.len(), 1);
        let abc = &settings.libraries[&library("1", "abc")];
        assert_eq!((&abc.sort, &abc.filters), (&None, &None));
        assert!(abc.view.is_some());
        let written = settings.write().unwrap();
        assert_eq!(written, serde_json::to_value(&settings.stored).unwrap());

        let mut changed = settings.clone();
        changed.remember_sort = true;
        changed.libraries.insert(library("1", "abc"), all_choices());
        changed.changed_from(&settings);
        let written = changed.write().unwrap();
        assert_eq!(written["remember_sort"], true);
        assert_eq!(written["later"], 3);
        let entry = &written["libraries"]["jellyfin/1/abc"];
        assert_eq!(entry["sort"]["order"], "descending");
        assert_eq!(entry["filter"], serde_json::json!(["unplayed"]));
        assert_eq!(entry["zoom"], 2);
        assert_eq!(written["libraries"]["not a key"]["sort"], 1);
        assert_eq!(written["libraries"]["jellyfin/1/def"], 7);

        // A library first remembered here replaces an entry under its key that could not be read.
        let mut replaced = settings.clone();
        replaced
            .libraries
            .insert(library("1", "def"), all_choices());
        replaced.changed_from(&settings);
        assert_eq!(
            replaced.write().unwrap()["libraries"]["jellyfin/1/def"]["filter"],
            serde_json::json!(["unplayed"])
        );
    }

    #[test]
    fn forgetting_a_part_forgets_it_for_every_library_also_unreadable_values() {
        let settings = read(
            r#"{"remember_sort": true, "libraries": {
                "jellyfin/1/abc": {"sort": {"key": "sort_name", "order": "descending"}, "view": {"tile": "wide", "titles": true}},
                "jellyfin/2/def": {"sort": "unreadable", "zoom": 2}
            }}"#,
        );
        let mut forgotten = settings.clone();
        forgotten.remember_sort = false;
        forgotten.forget_sorts();
        forgotten.changed_from(&settings);
        assert!(forgotten.libraries.values().all(|c| c.sort.is_none()));
        assert_eq!(
            forgotten.write().unwrap(),
            serde_json::json!({"libraries": {
                "jellyfin/1/abc": {"view": {"tile": "wide", "titles": true}},
                "jellyfin/2/def": {"zoom": 2}
            }})
        );
        let mut views = forgotten.clone();
        views.forget_views();
        views.forget_filters();
        views.changed_from(&forgotten);
        assert_eq!(
            views.write().unwrap(),
            serde_json::json!({"libraries": {"jellyfin/2/def": {"zoom": 2}}})
        );
    }

    #[test]
    fn forgetting_a_server_drops_its_libraries_only() {
        let settings = read(
            r#"{"libraries": {
                "jellyfin/1/abc": {"filter": []},
                "jellyfin/1/def": "unreadable",
                "jellyfin/12/abc": {"filter": []},
                "stash/1/abc": {"filter": []}
            }}"#,
        );
        let mut forgotten = settings.clone();
        forgotten.forget_server(&ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")));
        forgotten.changed_from(&settings);
        let written = forgotten.write().unwrap();
        let keys: Vec<_> = written["libraries"].as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(written["libraries"].get("jellyfin/12/abc").is_some());
        assert!(written["libraries"].get("stash/1/abc").is_some());
    }

    fn search_choices() -> SearchChoices {
        SearchChoices {
            filter: Some(RememberedSearchFilter {
                kind: Some(MediaKind::PERSON),
                source: "library".into(),
            }),
            view: Some(RememberedView {
                tile: "wide".into(),
                titles: true,
            }),
        }
    }

    #[test]
    fn the_search_pages_choices_round_trip_and_an_older_file_has_none() {
        // A file from before the search page remembered anything.
        let older = read(
            r#"{"remember_filter": true, "remember_view": true, "libraries": {"jellyfin/1/abc": {"filter": []}}}"#,
        );
        assert!(older.search.is_empty());
        let mut settings = older.clone();
        settings.search = search_choices();
        settings.changed_from(&older);
        let written = settings.write().unwrap();
        assert_eq!(
            written["search"],
            serde_json::json!({
                "filter": {"type": "person", "source": "library"},
                "view": {"tile": "wide", "titles": true}
            })
        );
        assert_eq!(
            written["libraries"]["jellyfin/1/abc"]["filter"],
            serde_json::json!([])
        );
        let (reread, _) = BrowseSettings::read(Some(written));
        assert_eq!(reread.search, search_choices());

        // Every type is `all`.
        let mut every = reread.clone();
        every.search.filter = Some(RememberedSearchFilter {
            kind: None,
            source: "all".into(),
        });
        every.changed_from(&reread);
        assert_eq!(
            every.write().unwrap()["search"]["filter"],
            serde_json::json!({"type": "all", "source": "all"})
        );
        let (again, _) = BrowseSettings::read(every.write());
        assert_eq!(again.search.filter.unwrap().kind, None);

        // Forgetting filters and views forgets the search page's too.
        let mut forgotten = reread.clone();
        forgotten.forget_filters();
        forgotten.changed_from(&reread);
        assert_eq!(forgotten.search.filter, None);
        assert_eq!(
            forgotten.write().unwrap()["search"],
            serde_json::json!({"view": {"tile": "wide", "titles": true}})
        );
        let mut nothing = forgotten.clone();
        nothing.forget_views();
        nothing.changed_from(&forgotten);
        assert!(nothing.write().unwrap().get("search").is_none());
    }

    #[test]
    fn the_search_pages_open_and_unreadable_values_are_kept() {
        let settings = read(
            r#"{"search": {"filter": {"type": "a_newer_kind", "source": "a_newer_source"}, "view": {"tile": "wide"}, "zoom": 2}}"#,
        );
        let filter = settings.search.filter.as_ref().unwrap();
        assert_eq!(filter.kind, Some(MediaKind::new("a_newer_kind")));
        assert_eq!(filter.source, "a_newer_source");
        // The view has no `titles`: unreadable, kept until changed here.
        assert_eq!(settings.search.view, None);
        let written = settings.write().unwrap();
        assert_eq!(
            written["search"]["view"],
            serde_json::json!({"tile": "wide"})
        );
        assert_eq!(written["search"]["zoom"], 2);
        let mut changed = settings.clone();
        changed.search = search_choices();
        changed.changed_from(&settings);
        let written = changed.write().unwrap();
        assert_eq!(written["search"]["view"]["titles"], true);
        assert_eq!(written["search"]["zoom"], 2);

        // A `search` that is not an object is kept until something is remembered here.
        let odd = read(r#"{"search": [1]}"#);
        assert!(odd.search.is_empty());
        assert_eq!(odd.write().unwrap()["search"], serde_json::json!([1]));
        let mut replaced = odd.clone();
        replaced.search = search_choices();
        replaced.changed_from(&odd);
        assert_eq!(
            replaced.write().unwrap()["search"]["filter"]["type"],
            "person"
        );
    }

    #[test]
    fn a_browse_value_that_is_not_an_object_is_kept() {
        let (settings, kept) = BrowseSettings::read(Some(Value::from("everything")));
        assert_eq!(settings, BrowseSettings::default());
        assert_eq!(kept, Some(Value::from("everything")));
        // As is a `libraries` that is not an object, until a library is remembered here.
        let settings = read(r#"{"libraries": [1, 2]}"#);
        assert!(settings.libraries.is_empty());
        assert_eq!(
            settings.write().unwrap(),
            serde_json::json!({"libraries": [1, 2]})
        );
        let mut changed = settings.clone();
        changed.libraries.insert(
            library("1", "abc"),
            LibraryChoices {
                filters: Some(Vec::new()),
                ..LibraryChoices::default()
            },
        );
        changed.changed_from(&settings);
        assert_eq!(
            changed.write().unwrap(),
            serde_json::json!({"libraries": {"jellyfin/1/abc": {"filter": []}}})
        );
    }

    fn kind(server: &str, kind: MediaKind) -> KindPage {
        KindPage::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(server)),
            kind,
        )
    }

    #[test]
    fn kind_pages_round_trip_beside_libraries_and_are_forgotten_with_them() {
        // An older file has no `kinds`: nothing remembered, nothing written.
        let older =
            read(r#"{"remember_sort": true, "libraries": {"jellyfin/1/abc": {"filter": []}}}"#);
        assert!(older.kinds.is_empty());
        assert!(older.write().unwrap().get("kinds").is_none());

        let mut settings = older.clone();
        settings
            .kinds
            .insert(kind("1", MediaKind::MOVIE), all_choices());
        settings.kinds.insert(
            kind("2", MediaKind::new("a_newer_kind")),
            LibraryChoices {
                filters: Some(Vec::new()),
                ..LibraryChoices::default()
            },
        );
        settings.changed_from(&older);
        let written = settings.write().unwrap();
        assert_eq!(
            written["kinds"],
            serde_json::json!({
                "jellyfin/1/movie": {
                    "sort": {"key": "date_created", "order": "descending"},
                    "filter": ["unplayed"],
                    "view": {"tile": "wide", "titles": false}
                },
                "jellyfin/2/a_newer_kind": {"filter": []}
            })
        );
        assert_eq!(
            written["libraries"]["jellyfin/1/abc"]["filter"],
            serde_json::json!([])
        );
        let (reread, _) = BrowseSettings::read(Some(written));
        assert_eq!(reread.kinds, settings.kinds);

        // Unknown fields in an entry and unreadable keys stay.
        let mut kept = read(
            r#"{"kinds": {"jellyfin/1/series": {"filter": ["played"], "later": 1}, "nonsense": {"filter": []}}}"#,
        );
        assert_eq!(
            kept.kinds[&kind("1", MediaKind::SERIES)].filters,
            Some(vec![ItemFilter::new("played")])
        );
        let before = kept.clone();
        kept.kinds
            .get_mut(&kind("1", MediaKind::SERIES))
            .unwrap()
            .view = all_choices().view;
        kept.changed_from(&before);
        let written = kept.write().unwrap();
        assert_eq!(written["kinds"]["jellyfin/1/series"]["later"], 1);
        assert_eq!(
            written["kinds"]["jellyfin/1/series"]["view"]["tile"],
            "wide"
        );
        assert_eq!(
            written["kinds"]["nonsense"],
            serde_json::json!({"filter": []})
        );

        // Forgetting a part forgets it on kind pages too; forgetting a server drops its kind pages.
        let mut forgot = reread.clone();
        forgot.forget_sorts();
        assert!(forgot.kinds.values().all(|choices| choices.sort.is_none()));
        assert!(
            forgot.write().unwrap()["kinds"]["jellyfin/1/movie"]
                .get("sort")
                .is_none()
        );
        forgot.forget_server(&ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")));
        assert_eq!(
            forgot.kinds.keys().cloned().collect::<Vec<_>>(),
            [kind("2", MediaKind::new("a_newer_kind"))]
        );
        assert!(
            forgot.write().unwrap()["kinds"]
                .get("jellyfin/1/movie")
                .is_none()
        );
    }
}
