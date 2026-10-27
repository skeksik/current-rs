//! What the player's quality menu and its playback info show, worded once for every platform (CLAUDE.md: all logic lives in the
//! core): bitrates in kbit/s and Mbit/s, a quality entry's label ("Original · 4K · 26.1 Mbit/s"), and the menu itself, with the
//! entry in use ticked. Numbers use the decimal separator the UI passed ([`NumberFormat`]), as the platform writes numbers.

use current_playback::{Quality, QualityOption};

use crate::numbers::{NumberFormat, rounded};

/// "40 Mbit/s", "1.5 Mbit/s"; below 1 Mbit/s in kbit/s, "720 kbit/s", which one decimal of Mbit/s would round to "0.7".
pub fn bitrate_label(bits_per_second: u32, format: &NumberFormat) -> String {
    let bits = u64::from(bits_per_second);
    if bits_per_second < 1_000_000 {
        format!("{} kbit/s", rounded(bits, 1_000))
    } else {
        format!("{} Mbit/s", format.one_decimal(bits, 1_000_000))
    }
}

/// One entry of the quality menu as the UI gets it back: the choice, its resolution and bitrate as [`QualityOption`] names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityEntry {
    pub quality: Quality,
    pub resolution: Option<String>,
    pub bitrate: Option<u32>,
}

impl From<&QualityOption> for QualityEntry {
    fn from(option: &QualityOption) -> Self {
        Self {
            quality: option.quality,
            resolution: option.resolution.map(str::to_owned),
            bitrate: option.bitrate,
        }
    }
}

/// A quality entry's label: "Original · 1080p · 5.2 Mbit/s", "720p · 4 Mbit/s", or without the resolution when Settings hides it
/// ("Show resolution in quality menu"); the parts not known are left out.
pub fn quality_label(entry: &QualityEntry, with_resolution: bool, format: &NumberFormat) -> String {
    let mut parts = Vec::new();
    if entry.quality == Quality::Maximum {
        parts.push("Original".to_owned());
    }
    if let Some(resolution) = entry
        .resolution
        .as_ref()
        .filter(|resolution| with_resolution && !resolution.is_empty())
    {
        parts.push(resolution.clone());
    }
    if let Some(bitrate) = entry.bitrate {
        parts.push(bitrate_label(bitrate, format));
    }
    parts.join(" · ")
}

/// One line of the quality menu: its entry, its label, and whether it is the quality in use (its radio item checked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityMenuItem {
    pub entry: QualityEntry,
    pub label: String,
    pub checked: bool,
}

/// The quality menu's entries before the core has named them for the version playing: "Original", then the common steps, each
/// with its bitrate and no resolution.
pub fn fallback_quality_entries() -> Vec<QualityEntry> {
    std::iter::once(Quality::Maximum)
        .chain(Quality::STEPS)
        .map(|quality| QualityEntry {
            quality,
            resolution: None,
            bitrate: match quality {
                Quality::Bitrate(bits) => Some(bits),
                Quality::Maximum => None,
            },
        })
        .collect()
}

/// The quality menu for `entries` (the core's for the version playing, else [`fallback_quality_entries`]) while playing at
/// `current`. The core plays each item at an entry of its menu (the highest not above the preferred quality), or at the preferred
/// bitrate itself when every entry is above it: that limit is listed as its own entry, among the steps in bitrate order ("Original"
/// stays first), so the menu never claims Original while the stream is capped (ADR 0019, choice 10). The entry in use is checked.
pub fn quality_menu(
    entries: Option<Vec<QualityEntry>>,
    current: Quality,
    with_resolution: bool,
    format: &NumberFormat,
) -> Vec<QualityMenuItem> {
    let mut entries = entries.unwrap_or_else(fallback_quality_entries);
    if let Quality::Bitrate(limit) = current
        && !entries.iter().any(|entry| entry.quality == current)
    {
        // A 10 Mbit/s limit sits between 20 and 4; an entry whose bitrate is not known is passed over.
        let at = entries
            .iter()
            .enumerate()
            .find(|(index, entry)| *index > 0 && entry.bitrate.is_some_and(|bits| bits < limit))
            .map_or(entries.len(), |(index, _)| index);
        entries.insert(
            at,
            QualityEntry {
                quality: current,
                resolution: None,
                bitrate: Some(limit),
            },
        );
    }
    entries
        .into_iter()
        .map(|entry| QualityMenuItem {
            label: quality_label(&entry, with_resolution, format),
            checked: entry.quality == current,
            entry,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot() -> NumberFormat {
        NumberFormat::default()
    }

    fn comma() -> NumberFormat {
        NumberFormat {
            decimal_separator: ",".to_owned(),
            ..NumberFormat::default()
        }
    }

    #[test]
    fn bitrates_read_as_the_windows_app_wrote_them() {
        let cases = [
            (40_000_000, "40 Mbit/s"),
            (1_000_000, "1 Mbit/s"),
            (1_500_000, "1.5 Mbit/s"),
            (26_100_000, "26.1 Mbit/s"),
            (26_140_000, "26.1 Mbit/s"),
            (26_160_000, "26.2 Mbit/s"),
            // Halfway goes up, as .NET's 15 digits then half up did (26.15 is just below as a double).
            (26_150_000, "26.2 Mbit/s"),
            (1_250_000, "1.3 Mbit/s"),
            (1_750_000, "1.8 Mbit/s"),
            (1_960_000, "2 Mbit/s"),
            (999_999, "1000 kbit/s"),
            (720_000, "720 kbit/s"),
            (720_499, "720 kbit/s"),
            (720_500, "721 kbit/s"),
            (0, "0 kbit/s"),
            (u32::MAX, "4295 Mbit/s"),
        ];
        for (bits, text) in cases {
            assert_eq!(bitrate_label(bits, &dot()), text, "{bits}");
        }
    }

    #[test]
    fn the_decimal_separator_is_the_platforms() {
        assert_eq!(bitrate_label(26_100_000, &comma()), "26,1 Mbit/s");
        assert_eq!(bitrate_label(40_000_000, &comma()), "40 Mbit/s");
        assert_eq!(bitrate_label(720_000, &comma()), "720 kbit/s");
    }

    #[test]
    fn a_label_leaves_out_what_is_not_known() {
        let original = QualityEntry {
            quality: Quality::Maximum,
            resolution: Some("4K".into()),
            bitrate: Some(26_100_000),
        };
        assert_eq!(
            quality_label(&original, true, &dot()),
            "Original · 4K · 26.1 Mbit/s"
        );
        assert_eq!(
            quality_label(&original, true, &comma()),
            "Original · 4K · 26,1 Mbit/s"
        );
        assert_eq!(
            quality_label(&original, false, &dot()),
            "Original · 26.1 Mbit/s"
        );
        let bare = QualityEntry {
            quality: Quality::Maximum,
            resolution: Some(String::new()),
            bitrate: None,
        };
        assert_eq!(quality_label(&bare, true, &dot()), "Original");
        let step = QualityEntry {
            quality: Quality::Bitrate(4_000_000),
            resolution: Some("720p".into()),
            bitrate: Some(4_000_000),
        };
        assert_eq!(quality_label(&step, true, &dot()), "720p · 4 Mbit/s");
    }

    fn labels(menu: &[QualityMenuItem]) -> Vec<(&str, bool)> {
        menu.iter()
            .map(|item| (item.label.as_str(), item.checked))
            .collect()
    }

    #[test]
    fn the_fallback_menu_is_original_and_the_steps() {
        let menu = quality_menu(None, Quality::Maximum, true, &dot());
        assert_eq!(
            labels(&menu),
            [
                ("Original", true),
                ("40 Mbit/s", false),
                ("20 Mbit/s", false),
                ("4 Mbit/s", false),
                ("1 Mbit/s", false),
            ]
        );
    }

    #[test]
    fn a_limit_below_or_between_the_entries_is_listed_and_checked() {
        let menu = quality_menu(None, Quality::Bitrate(10_000_000), true, &dot());
        assert_eq!(
            labels(&menu),
            [
                ("Original", false),
                ("40 Mbit/s", false),
                ("20 Mbit/s", false),
                ("10 Mbit/s", true),
                ("4 Mbit/s", false),
                ("1 Mbit/s", false),
            ]
        );
        let menu = quality_menu(None, Quality::Bitrate(500_000), true, &dot());
        assert_eq!(
            menu.last().map(|item| item.label.as_str()),
            Some("500 kbit/s")
        );
        assert!(menu.last().is_some_and(|item| item.checked));
        // An entry of the menu is checked, not added again.
        let menu = quality_menu(None, Quality::Bitrate(4_000_000), true, &dot());
        assert_eq!(menu.len(), 5);
        assert!(menu[3].checked);
    }

    #[test]
    fn the_cores_entries_are_named_and_original_stays_first() {
        let entries = vec![
            QualityEntry {
                quality: Quality::Maximum,
                resolution: Some("1080p".into()),
                bitrate: Some(5_200_000),
            },
            QualityEntry {
                quality: Quality::Bitrate(4_000_000),
                resolution: Some("720p".into()),
                bitrate: Some(4_000_000),
            },
        ];
        // A limit above every step but the original's bitrate still goes after Original.
        let menu = quality_menu(
            Some(entries.clone()),
            Quality::Bitrate(6_000_000),
            true,
            &dot(),
        );
        assert_eq!(
            labels(&menu),
            [
                ("Original · 1080p · 5.2 Mbit/s", false),
                ("6 Mbit/s", true),
                ("720p · 4 Mbit/s", false),
            ]
        );
        let menu = quality_menu(Some(entries), Quality::Maximum, false, &dot());
        assert_eq!(
            labels(&menu),
            [("Original · 5.2 Mbit/s", true), ("4 Mbit/s", false)]
        );
    }
}
