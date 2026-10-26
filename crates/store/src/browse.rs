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

use std::collections::BTreeMap;

use current_model::{
    ItemFilter, ItemId, ItemRef, ServerKey, ServerRef, ServiceType, SortKey, SortOrder,
};
use serde_json::{Map, Value};

const REMEMBER_SORT: &str = "remember_sort";
const REMEMBER_FILTER: &str = "remember_filter";
const REMEMBER_VIEW: &str = "remember_view";
const LIBRARIES: &str = "libraries";

const SORT: &str = "sort";
const FILTER: &str = "filter";
const VIEW: &str = "view";

const SORT_KEY: &str = "key";
const SORT_ORDER: &str = "order";
const ASCENDING: &str = "ascending";
const DESCENDING: &str = "descending";
const VIEW_TILE: &str = "tile";
const VIEW_TITLES: &str = "titles";

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

/// The key of `library` under `libraries`.
fn library_key(library: &ItemRef) -> String {
    format!("{}/{}", library.server, library.id)
}

/// The library a key names; `None` for a key this version cannot read.
fn parse_library_key(key: &str) -> Option<ItemRef> {
    let mut parts = key.splitn(3, '/');
    let (service, server, id) = (parts.next()?, parts.next()?, parts.next()?);
    if service.is_empty() || server.is_empty() || id.is_empty() {
        return None;
    }
    Some(ItemRef::new(
        ServerRef::new(ServiceType::new(service), ServerKey::new(server)),
        ItemId::new(id),
    ))
}

/// The browse settings. All off, nothing remembered by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowseSettings {
    pub remember_sort: bool,
    pub remember_filter: bool,
    pub remember_view: bool,
    /// What is remembered per library. A library whose entry this version cannot read is not listed here and is kept in the file.
    pub libraries: BTreeMap<ItemRef, LibraryChoices>,
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
        let libraries = match stored.get(LIBRARIES) {
            Some(Value::Object(entries)) => entries
                .iter()
                .filter_map(|(key, entry)| {
                    let library = parse_library_key(key)?;
                    let Value::Object(entry) = entry else {
                        return None;
                    };
                    Some((library, LibraryChoices::read(entry)))
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        (
            Self {
                remember_sort: switch(REMEMBER_SORT),
                remember_filter: switch(REMEMBER_FILTER),
                remember_view: switch(REMEMBER_VIEW),
                libraries,
                stored,
            },
            None,
        )
    }

    /// Forgets the sort of every library, also one this version could not read.
    pub fn forget_sorts(&mut self) {
        self.forget_part(SORT, |choices| choices.sort = None);
    }

    /// Forgets the filters of every library, also ones this version could not read.
    pub fn forget_filters(&mut self) {
        self.forget_part(FILTER, |choices| choices.filters = None);
    }

    /// Forgets the view of every library, also one this version could not read.
    pub fn forget_views(&mut self) {
        self.forget_part(VIEW, |choices| choices.view = None);
    }

    /// Forgets every library of `server`, also entries this version could not read: the server was removed.
    pub fn forget_server(&mut self, server: &ServerRef) {
        self.libraries
            .retain(|library, _| library.server != *server);
        if let Some(Value::Object(entries)) = self.stored.get_mut(LIBRARIES) {
            entries.retain(|key, _| {
                parse_library_key(key).is_none_or(|library| library.server != *server)
            });
        }
    }

    fn forget_part(&mut self, part: &str, forget: impl Fn(&mut LibraryChoices)) {
        self.libraries.values_mut().for_each(forget);
        if let Some(Value::Object(entries)) = self.stored.get_mut(LIBRARIES) {
            for entry in entries.values_mut() {
                if let Value::Object(entry) = entry {
                    entry.remove(part);
                }
            }
        }
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
        if self.libraries == before.libraries {
            return;
        }
        let entries = match self.stored.get_mut(LIBRARIES) {
            Some(Value::Object(entries)) => entries,
            Some(_) => {
                // A `libraries` this version could not read gives way to the libraries remembered here.
                self.stored.remove(LIBRARIES);
                return;
            }
            None => return,
        };
        for (library, then) in &before.libraries {
            let key = library_key(library);
            let Some(now) = self.libraries.get(library) else {
                entries.remove(&key);
                continue;
            };
            if let Some(Value::Object(entry)) = entries.get_mut(&key) {
                forget_changed(entry, now, then);
            }
        }
        for library in self.libraries.keys() {
            if !before.libraries.contains_key(library) {
                // New here: anything stored under its key could not be read, and gives way.
                entries.remove(&library_key(library));
            }
        }
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
        match object.remove(LIBRARIES) {
            // A `libraries` this version could not read, and none remembered here since: kept as it was.
            Some(unreadable) if !unreadable.is_object() => {
                object.insert(LIBRARIES.to_owned(), unreadable);
            }
            stored => {
                let mut entries = match stored {
                    Some(Value::Object(entries)) => entries,
                    _ => Map::new(),
                };
                for (library, choices) in &self.libraries {
                    let key = library_key(library);
                    let mut entry = match entries.remove(&key) {
                        Some(Value::Object(entry)) => entry,
                        _ => Map::new(),
                    };
                    choices.write_into(&mut entry);
                    if !entry.is_empty() {
                        entries.insert(key, Value::Object(entry));
                    }
                }
                if !entries.is_empty() {
                    object.insert(LIBRARIES.to_owned(), Value::Object(entries));
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
}
