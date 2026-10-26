//! The playback settings in `settings.json` (ADR 0019, choices 8 and 10): the quality remembered for local and for internet addresses,
//! and the kinds of segment skipped automatically. All open values (ADR 0013): a value this version cannot read reads as the default
//! and is written back unchanged until the setting is changed here, and fields this version does not know are kept.
//!
//! The file keeps them under `playback`: `{"quality_local": "maximum" | <bits per second>, "quality_internet": ..., "auto_skip":
//! ["intro", "outro", ...], "quality_resolution": false, "quality_all": true}`, each left out at its default.

use serde_json::{Map, Value};

/// The most a stream may carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StreamQuality {
    /// The server's ceiling: only its own limits apply.
    #[default]
    Maximum,
    /// Bits per second.
    Bitrate(u32),
}

impl StreamQuality {
    fn read(value: &Value) -> Option<Self> {
        match value {
            Value::String(name) if name == "maximum" => Some(Self::Maximum),
            Value::Number(number) => number
                .as_u64()
                .filter(|bits| *bits > 0)
                .and_then(|bits| u32::try_from(bits).ok())
                .map(Self::Bitrate),
            _ => None,
        }
    }

    fn write(self) -> Value {
        match self {
            Self::Maximum => Value::from("maximum"),
            Self::Bitrate(bits) => Value::from(bits),
        }
    }
}

const QUALITY_LOCAL: &str = "quality_local";
const QUALITY_INTERNET: &str = "quality_internet";
const AUTO_SKIP: &str = "auto_skip";
const QUALITY_RESOLUTION: &str = "quality_resolution";
const QUALITY_ALL: &str = "quality_all";

/// The playback settings. "Maximum", no automatic skipping, and the quality menu naming resolutions by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackSettings {
    pub quality_local: StreamQuality,
    pub quality_internet: StreamQuality,
    /// The segment kinds skipped automatically, by their names in `current-model` (`intro`, `outro`, ...); empty when off. Kinds this
    /// version does not know are kept.
    pub auto_skip: Vec<String>,
    /// Whether the quality menu names the resolution each choice gives ("Show resolution in quality menu"); on by default (owner,
    /// 2026-10-26).
    pub quality_resolution: bool,
    /// Whether the quality menu lists every step below the original's bitrate, not only the common ones ("Show all quality
    /// options"); off by default (owner, 2026-10-26).
    pub quality_all: bool,
    /// The stored object as read: fields this version does not know, and known fields whose value it could not read.
    stored: Map<String, Value>,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            quality_local: StreamQuality::default(),
            quality_internet: StreamQuality::default(),
            auto_skip: Vec::new(),
            quality_resolution: true,
            quality_all: false,
            stored: Map::new(),
        }
    }
}

impl PlaybackSettings {
    /// The settings from the stored `playback` value; anything but an object reads as the defaults and is kept.
    pub(crate) fn read(value: Option<Value>) -> (Self, Option<Value>) {
        let Some(value) = value else {
            return (Self::default(), None);
        };
        let Value::Object(stored) = value else {
            return (Self::default(), Some(value));
        };
        let quality = |key: &str| {
            stored
                .get(key)
                .and_then(StreamQuality::read)
                .unwrap_or_default()
        };
        let auto_skip = match stored.get(AUTO_SKIP) {
            Some(Value::Array(kinds)) if kinds.iter().all(Value::is_string) => kinds
                .iter()
                .filter_map(|kind| kind.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        let quality_resolution = stored
            .get(QUALITY_RESOLUTION)
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let quality_all = stored
            .get(QUALITY_ALL)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        (
            Self {
                quality_local: quality(QUALITY_LOCAL),
                quality_internet: quality(QUALITY_INTERNET),
                auto_skip,
                quality_resolution,
                quality_all,
                stored,
            },
            None,
        )
    }

    /// Forgets the stored value of each setting that differs from `before`: it was changed here, so what was read no longer stands.
    pub(crate) fn changed_from(&mut self, before: &Self) {
        if self.quality_local != before.quality_local {
            self.stored.remove(QUALITY_LOCAL);
        }
        if self.quality_internet != before.quality_internet {
            self.stored.remove(QUALITY_INTERNET);
        }
        if self.auto_skip != before.auto_skip {
            self.stored.remove(AUTO_SKIP);
        }
        if self.quality_resolution != before.quality_resolution {
            self.stored.remove(QUALITY_RESOLUTION);
        }
        if self.quality_all != before.quality_all {
            self.stored.remove(QUALITY_ALL);
        }
    }

    /// The value to store: what was read, with each known setting written where it reads as something here or was changed here.
    /// `None` when nothing is left (all defaults).
    pub(crate) fn write(&self) -> Option<Value> {
        let mut object = self.stored.clone();
        let mut set = |key: &str, readable: bool, value: Option<Value>| {
            // A stored value this version cannot read stays as it was.
            if object.contains_key(key) && !readable {
                return;
            }
            match value {
                Some(value) => object.insert(key.to_owned(), value),
                None => object.remove(key),
            };
        };
        let quality_readable = |key: &str| {
            self.stored
                .get(key)
                .is_none_or(|value| StreamQuality::read(value).is_some())
        };
        let quality =
            |quality: StreamQuality| (quality != StreamQuality::Maximum).then(|| quality.write());
        set(
            QUALITY_LOCAL,
            quality_readable(QUALITY_LOCAL),
            quality(self.quality_local),
        );
        set(
            QUALITY_INTERNET,
            quality_readable(QUALITY_INTERNET),
            quality(self.quality_internet),
        );
        let skip_readable = self.stored.get(AUTO_SKIP).is_none_or(
            |value| matches!(value, Value::Array(kinds) if kinds.iter().all(Value::is_string)),
        );
        set(
            AUTO_SKIP,
            skip_readable,
            (!self.auto_skip.is_empty()).then(|| Value::from(self.auto_skip.clone())),
        );
        set(
            QUALITY_RESOLUTION,
            self.stored
                .get(QUALITY_RESOLUTION)
                .is_none_or(Value::is_boolean),
            (!self.quality_resolution).then_some(Value::Bool(false)),
        );
        set(
            QUALITY_ALL,
            self.stored.get(QUALITY_ALL).is_none_or(Value::is_boolean),
            self.quality_all.then_some(Value::Bool(true)),
        );
        (!object.is_empty()).then_some(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> PlaybackSettings {
        let (settings, kept) = PlaybackSettings::read(Some(serde_json::from_str(text).unwrap()));
        assert!(kept.is_none());
        settings
    }

    #[test]
    fn values_round_trip_and_defaults_are_left_out() {
        let settings = read(
            r#"{"quality_local": "maximum", "quality_internet": 4000000, "auto_skip": ["intro", "outro"]}"#,
        );
        assert_eq!(settings.quality_local, StreamQuality::Maximum);
        assert_eq!(settings.quality_internet, StreamQuality::Bitrate(4_000_000));
        assert_eq!(settings.auto_skip, ["intro", "outro"]);
        let mut changed = settings.clone();
        changed.quality_internet = StreamQuality::Maximum;
        changed.auto_skip.clear();
        changed.changed_from(&settings);
        // "maximum" was stored explicitly and still reads: written as the default, which is left out.
        assert_eq!(changed.write(), None);
        assert_eq!(PlaybackSettings::default().write(), None);
    }

    #[test]
    fn the_quality_menu_names_resolutions_unless_turned_off() {
        assert!(PlaybackSettings::default().quality_resolution);
        let off = read(r#"{"quality_resolution": false}"#);
        assert!(!off.quality_resolution);
        assert_eq!(off.write().unwrap()["quality_resolution"], false);
        let mut on = off.clone();
        on.quality_resolution = true;
        on.changed_from(&off);
        // On is the default, left out of the file.
        assert_eq!(on.write(), None);
        // A value this version cannot read reads as on and is kept until changed here.
        let odd = read(r#"{"quality_resolution": "sometimes"}"#);
        assert!(odd.quality_resolution);
        assert_eq!(odd.write().unwrap()["quality_resolution"], "sometimes");
    }

    #[test]
    fn the_quality_menu_lists_the_common_steps_unless_all_are_asked_for() {
        assert!(!PlaybackSettings::default().quality_all);
        let all = read(r#"{"quality_all": true}"#);
        assert!(all.quality_all);
        assert_eq!(all.write().unwrap()["quality_all"], true);
        let mut common = all.clone();
        common.quality_all = false;
        common.changed_from(&all);
        assert_eq!(common.write(), None);
    }

    #[test]
    fn unreadable_values_and_unknown_fields_are_kept_until_changed_here() {
        let settings = read(
            r#"{"quality_local": "automatic", "auto_skip": "everything", "subtitle_size": 3, "quality_internet": 2000000}"#,
        );
        assert_eq!(settings.quality_local, StreamQuality::Maximum);
        assert!(settings.auto_skip.is_empty());
        let written = settings.write().unwrap();
        assert_eq!(written["quality_local"], "automatic");
        assert_eq!(written["auto_skip"], "everything");
        assert_eq!(written["subtitle_size"], 3);
        assert_eq!(written["quality_internet"], 2_000_000);

        let mut changed = settings.clone();
        changed.quality_local = StreamQuality::Bitrate(8_000_000);
        changed.auto_skip = vec!["intro".into(), "a_future_kind".into()];
        changed.changed_from(&settings);
        let written = changed.write().unwrap();
        assert_eq!(written["quality_local"], 8_000_000);
        assert_eq!(
            written["auto_skip"],
            serde_json::json!(["intro", "a_future_kind"])
        );
        assert_eq!(written["subtitle_size"], 3);
    }

    #[test]
    fn a_playback_value_that_is_not_an_object_is_kept() {
        let (settings, kept) = PlaybackSettings::read(Some(Value::from(7)));
        assert_eq!(settings, PlaybackSettings::default());
        assert_eq!(kept, Some(Value::from(7)));
    }
}
