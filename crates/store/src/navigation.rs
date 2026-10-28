//! The navigation settings in `settings.json` (ADR 0024): how the user arranged the pane's groups and Home's rows, per server and
//! user. Each group is a list of placements: an item the user moved names the item it goes after (`""` for first in its group), and
//! an item the user hid or showed carries `hidden`. An item with neither has no entry, so it follows the server's or the app's
//! order. All open values (ADR 0013): group names and item keys pass through unchanged, and fields this version does not know are
//! kept, in the object and in each entry.
//!
//! The file keeps them under `navigation`, keyed by `service/server key/user id`:
//!
//! ```json
//! "navigation": {
//!   "jellyfin/1/<user id>": {
//!     "libraries": [{"key": "<library id>", "after": ""}, {"key": "<library id>", "hidden": false}],
//!     "home_rows": [{"key": "seen_recently", "after": "resume"}]
//!   }
//! }
//! ```
//!
//! A scope or a group this version cannot read is kept as it is until that group is changed here; an entry without a key is dropped
//! when its group is next changed.

use std::collections::BTreeMap;

use current_model::{ServerRef, UserId};
use serde_json::{Map, Value};

use crate::browse::parse_key;

const KEY: &str = "key";
const AFTER: &str = "after";
const HIDDEN: &str = "hidden";

/// Where an item the user moved goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// First in its group.
    First,
    /// Right after the item with this key.
    After(String),
}

/// One item's entry in a group: where the user put it, and whether they hid or showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub key: String,
    /// `None` while the user has not moved it: it keeps its place in the default order.
    pub after: Option<Anchor>,
    /// `None` while the user has not hidden or shown it: the default decides.
    pub hidden: Option<bool>,
    /// Fields this version does not know, written back unchanged.
    extra: Map<String, Value>,
}

impl Placement {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            after: None,
            hidden: None,
            extra: Map::new(),
        }
    }

    /// Whether the entry says nothing: it can be left out.
    pub fn is_empty(&self) -> bool {
        self.after.is_none() && self.hidden.is_none() && self.extra.is_empty()
    }

    fn read(value: &Value) -> Option<Self> {
        let Value::Object(entry) = value else {
            return None;
        };
        let key = entry.get(KEY)?.as_str()?.to_owned();
        let after = match entry.get(AFTER) {
            Some(Value::String(after)) if after.is_empty() => Some(Anchor::First),
            Some(Value::String(after)) => Some(Anchor::After(after.clone())),
            _ => None,
        };
        let hidden = entry.get(HIDDEN).and_then(Value::as_bool);
        let mut extra = entry.clone();
        extra.remove(KEY);
        // A value this version cannot read stays in `extra`, so it is written back.
        if after.is_some() {
            extra.remove(AFTER);
        }
        if hidden.is_some() {
            extra.remove(HIDDEN);
        }
        Some(Self {
            key,
            after,
            hidden,
            extra,
        })
    }

    fn write(&self) -> Value {
        let mut entry = Map::new();
        entry.insert(KEY.to_owned(), Value::from(self.key.as_str()));
        entry.extend(self.extra.clone());
        match &self.after {
            Some(Anchor::First) => {
                entry.insert(AFTER.to_owned(), Value::from(""));
            }
            Some(Anchor::After(key)) => {
                entry.insert(AFTER.to_owned(), Value::from(key.as_str()));
            }
            None => {}
        }
        if let Some(hidden) = self.hidden {
            entry.insert(HIDDEN.to_owned(), Value::from(hidden));
        }
        Value::Object(entry)
    }
}

/// Whose arrangement: a user on a server.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NavigationScope {
    pub server: ServerRef,
    pub user: UserId,
}

impl NavigationScope {
    pub fn new(server: ServerRef, user: UserId) -> Self {
        Self { server, user }
    }

    fn key(&self) -> String {
        format!("{}/{}", self.server, self.user)
    }

    fn parse(key: &str) -> Option<Self> {
        let (server, user) = parse_key(key)?;
        // A user id holds no `/` (Jellyfin's are GUIDs): a key with one more part is not one this version wrote.
        if user.contains('/') {
            return None;
        }
        Some(Self::new(server, UserId::new(user)))
    }
}

/// Every arrangement the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavigationSettings {
    /// Each scope's groups by name, each group's entries in the order they were placed. A group this version could not read is not
    /// listed here and is kept in the file.
    pub scopes: BTreeMap<NavigationScope, BTreeMap<String, Vec<Placement>>>,
    /// The stored object as read.
    stored: Map<String, Value>,
}

impl NavigationSettings {
    /// The settings from the stored `navigation` value; anything but an object reads as nothing arranged and is kept.
    pub(crate) fn read(value: Option<Value>) -> (Self, Option<Value>) {
        let Some(value) = value else {
            return (Self::default(), None);
        };
        let Value::Object(stored) = value else {
            return (Self::default(), Some(value));
        };
        let mut scopes = BTreeMap::new();
        for (key, groups) in &stored {
            let (Some(scope), Value::Object(groups)) = (NavigationScope::parse(key), groups) else {
                continue;
            };
            let groups: BTreeMap<String, Vec<Placement>> = groups
                .iter()
                .filter_map(|(name, entries)| {
                    let Value::Array(entries) = entries else {
                        return None;
                    };
                    Some((
                        name.clone(),
                        entries.iter().filter_map(Placement::read).collect(),
                    ))
                })
                .collect();
            scopes.insert(scope, groups);
        }
        (Self { scopes, stored }, None)
    }

    /// The value to store: the object as read, with every group changed here written over it. `None` when there is nothing.
    pub(crate) fn write(&self) -> Option<Value> {
        let mut object = self.stored.clone();
        for (scope, groups) in &self.scopes {
            let key = scope.key();
            let mut stored = match object.remove(&key) {
                Some(Value::Object(stored)) => stored,
                _ => Map::new(),
            };
            for (name, entries) in groups {
                // A group as stored that reads as what is held here is kept as it is, entries this version cannot read included.
                if let Some(Value::Array(as_stored)) = stored.get(name) {
                    let read: Vec<Placement> =
                        as_stored.iter().filter_map(Placement::read).collect();
                    if &read == entries {
                        continue;
                    }
                }
                let entries: Vec<Value> = entries
                    .iter()
                    .filter(|entry| !entry.is_empty())
                    .map(Placement::write)
                    .collect();
                if entries.is_empty() {
                    stored.remove(name);
                } else {
                    stored.insert(name.clone(), Value::Array(entries));
                }
            }
            if !stored.is_empty() {
                object.insert(key, Value::Object(stored));
            }
        }
        (!object.is_empty()).then_some(Value::Object(object))
    }

    /// `scope`'s entries in `group`, in the order they were placed; empty when there are none.
    pub fn group(&self, scope: &NavigationScope, group: &str) -> &[Placement] {
        self.scopes
            .get(scope)
            .and_then(|groups| groups.get(group))
            .map_or(&[], Vec::as_slice)
    }

    /// Replaces `scope`'s entries in `group`; an empty list forgets the group (Reset).
    pub fn set_group(&mut self, scope: &NavigationScope, group: &str, entries: Vec<Placement>) {
        let entries: Vec<Placement> = entries
            .into_iter()
            .filter(|entry| !entry.is_empty())
            .collect();
        if entries.is_empty() && self.group(scope, group).is_empty() {
            // Nothing held, nothing to forget: an unchanged settings value writes nothing.
            return;
        }
        self.scopes
            .entry(scope.clone())
            .or_default()
            .insert(group.to_owned(), entries);
    }

    /// Forgets every arrangement of `server`'s users, also those this version could not read.
    pub fn forget_server(&mut self, server: &ServerRef) {
        self.scopes.retain(|scope, _| &scope.server != server);
        self.stored
            .retain(|key, _| parse_key(key).is_none_or(|(stored, _)| &stored != server));
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ServerKey, ServiceType};
    use serde_json::json;

    use super::*;

    fn scope(key: &str) -> NavigationScope {
        NavigationScope::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(key)),
            UserId::new("u1"),
        )
    }

    #[test]
    fn entries_round_trip_with_unknown_fields_and_groups() {
        let stored = json!({
            "jellyfin/1/u1": {
                "libraries": [
                    {"key": "a", "after": "", "later": 1},
                    {"key": "b", "after": "a", "hidden": true},
                    {"key": "c", "hidden": "maybe"},
                    {"no key": true}
                ],
                "future": {"not": "a list"}
            },
            "not a scope": [1],
            "jellyfin/2/u1": "unreadable"
        });
        let (settings, unknown) = NavigationSettings::read(Some(stored.clone()));
        assert_eq!(unknown, None);
        let libraries = settings.group(&scope("1"), "libraries");
        assert_eq!(libraries.len(), 3);
        assert_eq!(libraries[0].after, Some(Anchor::First));
        assert_eq!(libraries[1].after, Some(Anchor::After("a".into())));
        assert_eq!(libraries[1].hidden, Some(true));
        // A value this version cannot read is kept.
        assert_eq!(libraries[2].hidden, None);
        assert!(!libraries[2].is_empty());
        assert!(settings.group(&scope("1"), "future").is_empty());
        assert!(settings.group(&scope("2"), "libraries").is_empty());

        let written = settings.write().unwrap();
        assert_eq!(written["jellyfin/1/u1"]["libraries"][0]["later"], 1);
        assert_eq!(written["jellyfin/1/u1"]["libraries"][1]["after"], "a");
        assert_eq!(written["jellyfin/1/u1"]["libraries"][2]["hidden"], "maybe");
        assert_eq!(written["jellyfin/1/u1"]["future"], json!({"not": "a list"}));
        assert_eq!(written["not a scope"], json!([1]));
        assert_eq!(written["jellyfin/2/u1"], "unreadable");
        // Unchanged, the entry without a key stays too: only a changed group is written over.
        assert_eq!(
            written["jellyfin/1/u1"]["libraries"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn a_changed_group_is_written_and_an_empty_one_forgotten() {
        let mut settings = NavigationSettings::default();
        assert_eq!(settings.write(), None);
        let mut moved = Placement::new("x");
        moved.after = Some(Anchor::After("y".into()));
        let mut shown = Placement::new("z");
        shown.hidden = Some(false);
        settings.set_group(
            &scope("1"),
            "home_rows",
            vec![moved, shown, Placement::new("empty")],
        );
        let written = settings.write().unwrap();
        assert_eq!(
            written,
            json!({"jellyfin/1/u1": {"home_rows": [
                {"key": "x", "after": "y"},
                {"key": "z", "hidden": false}
            ]}})
        );
        let (mut settings, _) = NavigationSettings::read(Some(written));
        settings.set_group(&scope("1"), "home_rows", Vec::new());
        assert_eq!(settings.write(), None);
    }

    #[test]
    fn a_value_that_is_not_an_object_is_kept() {
        let (settings, unknown) = NavigationSettings::read(Some(json!([1])));
        assert_eq!(settings, NavigationSettings::default());
        assert_eq!(unknown, Some(json!([1])));
    }

    #[test]
    fn forgetting_a_server_forgets_its_users_arrangements() {
        let (mut settings, _) = NavigationSettings::read(Some(json!({
            "jellyfin/1/u1": {"tabs": [{"key": "movie", "hidden": true}]},
            "jellyfin/1/u2": "unreadable",
            "jellyfin/2/u1": {"tabs": [{"key": "series", "hidden": true}]}
        })));
        settings.forget_server(&scope("1").server);
        assert!(settings.group(&scope("1"), "tabs").is_empty());
        assert_eq!(
            settings.write().unwrap(),
            json!({"jellyfin/2/u1": {"tabs": [{"key": "series", "hidden": true}]}})
        );
    }
}
