//! The quality limit (ADR 0019, choice 10): a preferred quality in Settings, "Maximum" by default and the same for local and internet
//! addresses, and a per-playback menu whose pick changes only the item playing (owner, 2026-10-27). Each item plays at the highest
//! entry of its own menu that is not above the preferred quality ([`snap_quality`]). No automatic bandwidth test.
//!
//! The menu names each choice by the resolution the Jellyfin server would send it at and its bitrate (owner, 2026-10-26): the widest
//! picture the server sends at a bitrate is its `ResolutionNormalizer` table, and a picture's name its
//! `MediaStream.GetResolutionText` (jellyfin/jellyfin `MediaBrowser.Model/Dlna/ResolutionNormalizer.cs`, last changed 2025-06-15, and
//! `MediaBrowser.Model/Entities/MediaStream.cs`, last changed 2026-09-05; both read 2026-10-26). By default the menu offers the
//! common resolutions, each at a bitrate the server's table gives that name for; Settings can list every step instead.

use current_model::MediaSource;

/// The highest bitrate the user wants a stream at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quality {
    /// No limit of Current's: the service crate sends its server's ceiling, so the server's own limits are the only cap.
    #[default]
    Maximum,
    /// Bits per second.
    Bitrate(u32),
}

impl Quality {
    /// The menu's common steps below "Maximum", highest first (owner, 2026-10-26): 4K, 1080p, 720p and 480p, each at a bitrate the
    /// server's table names so: 3840, 2560 (which the server calls 1080p), 1280 and 768 wide.
    pub const STEPS: [Quality; 4] = [
        Quality::Bitrate(40_000_000),
        Quality::Bitrate(20_000_000),
        Quality::Bitrate(4_000_000),
        Quality::Bitrate(1_000_000),
    ];

    /// Every step, for Settings' "Show all quality options": the server's web client's (jellyfin/jellyfin-web
    /// `src/components/qualityOptions.js`, last changed 2024-09-17, read 2026-10-26) and the common 1 Mbit/s, highest first.
    pub const ALL_STEPS: [Quality; 15] = [
        Quality::Bitrate(120_000_000),
        Quality::Bitrate(80_000_000),
        Quality::Bitrate(60_000_000),
        Quality::Bitrate(40_000_000),
        Quality::Bitrate(20_000_000),
        Quality::Bitrate(15_000_000),
        Quality::Bitrate(10_000_000),
        Quality::Bitrate(8_000_000),
        Quality::Bitrate(6_000_000),
        Quality::Bitrate(4_000_000),
        Quality::Bitrate(3_000_000),
        Quality::Bitrate(1_500_000),
        Quality::Bitrate(1_000_000),
        Quality::Bitrate(720_000),
        Quality::Bitrate(420_000),
    ];

    /// Whether the limit is below `bitrate` (a source's bitrate in bits per second). A source without a known bitrate counts as above
    /// any limit, as the server counts it.
    pub fn limits(self, bitrate: Option<u32>) -> bool {
        match self {
            Quality::Maximum => false,
            Quality::Bitrate(limit) => bitrate.is_none_or(|bitrate| bitrate > limit),
        }
    }

    /// Whether `self` and `other` have the server send the same for a source of `bitrate`: they are equal, or neither limits it
    /// ([`Quality::limits`]).
    pub fn sends_same(self, other: Quality, bitrate: Option<u32>) -> bool {
        self == other || (!self.limits(bitrate) && !other.limits(bitrate))
    }
}

/// The widest picture the Jellyfin server sends at most `bitrate` bits per second: the first of `ResolutionNormalizer`'s
/// configurations (maximum width, maximum bitrate) whose bitrate is not below it; none above its last, where it sets no width. Its
/// bitrates are for SDR H.264 at 30 frames per second, which this assumes: other codecs, frame rates and HDR move the server's choice
/// (not modelled).
const WIDTH_CAPS: [(u32, u32); 8] = [
    (416, 365_000),
    (640, 730_000),
    (768, 1_100_000),
    (960, 3_000_000),
    (1280, 6_000_000),
    (1920, 13_500_000),
    (2560, 28_000_000),
    (3840, 50_000_000),
];

fn width_cap(bitrate: u32) -> Option<u32> {
    WIDTH_CAPS
        .iter()
        .find(|(_, max)| bitrate <= *max)
        .map(|(width, _)| *width)
}

/// A picture's name as the Jellyfin server gives it (`MediaStream.GetResolutionText`, progressive): by its width, with its height
/// no more than the size's; `None` for one larger than 8K.
pub fn resolution_name(width: u32, height: u32) -> Option<&'static str> {
    const NAMES: [(u32, u32, &str); 13] = [
        (256, 144, "144p"),
        (426, 240, "240p"),
        (640, 360, "360p"),
        (682, 384, "384p"),
        (720, 404, "404p"),
        (854, 480, "480p"),
        (960, 544, "540p"),
        (1024, 576, "576p"),
        (1280, 962, "720p"),
        // Up to 2560 × 1440: the server counts ultra-wide 1080p (2560 × 1080) and WQHD as 1080p.
        (2560, 1440, "1080p"),
        (4096, 3072, "4K"),
        (8192, 6144, "8K"),
        (u32::MAX, u32::MAX, ""),
    ];
    NAMES
        .iter()
        .find(|(max_width, max_height, _)| width <= *max_width && height <= *max_height)
        .map(|(_, _, name)| *name)
        .filter(|name| !name.is_empty())
}

/// One entry of the quality menu: the choice, the resolution the server would send it at, and its bitrate. For "Maximum", the
/// original's own resolution and bitrate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityOption {
    pub quality: Quality,
    /// `None` when neither the original's size nor the step's width is known.
    pub resolution: Option<&'static str>,
    pub bitrate: Option<u32>,
}

/// The quality menu for `source` (the version playing; `None` before one is planned): "Maximum" (the original), then the common
/// steps, or every step when `all`, below the original's bitrate. A step at or above it changes nothing, as the limit does not apply
/// ([`Quality::limits`]), so it is left out (owner, 2026-10-26); without a known bitrate every step is listed. A step's picture is the
/// original's narrowed to the step's width cap, its height in proportion, never wider than the original; without the original's size,
/// the width cap at 16:9.
pub fn quality_options(source: Option<&MediaSource>, all: bool) -> Vec<QualityOption> {
    let size = source
        .and_then(MediaSource::video)
        .and_then(|stream| stream.video.as_ref())
        .and_then(|video| video.width.zip(video.height))
        .filter(|(width, height)| *width > 0 && *height > 0);
    let source_bitrate = source.and_then(|source| source.bitrate);
    let mut options = vec![QualityOption {
        quality: Quality::Maximum,
        resolution: size.and_then(|(width, height)| resolution_name(width, height)),
        bitrate: source_bitrate,
    }];
    let steps: &[Quality] = if all {
        &Quality::ALL_STEPS
    } else {
        &Quality::STEPS
    };
    options.extend(
        steps
            .iter()
            .filter(|quality| quality.limits(source_bitrate))
            .map(|quality| {
                let bitrate = match quality {
                    Quality::Bitrate(bitrate) => *bitrate,
                    Quality::Maximum => u32::MAX,
                };
                let picture = match (size, width_cap(bitrate)) {
                    (Some((width, height)), Some(cap)) if cap < width => Some((
                        cap,
                        u32::try_from(u64::from(height) * u64::from(cap) / u64::from(width))
                            .unwrap_or(height),
                    )),
                    (Some(size), _) => Some(size),
                    (None, Some(cap)) => Some((cap, cap * 9 / 16)),
                    (None, None) => None,
                };
                QualityOption {
                    quality: *quality,
                    resolution: picture.and_then(|(width, height)| resolution_name(width, height)),
                    bitrate: Some(bitrate),
                }
            }),
    );
    options
}

/// The entry of `source`'s quality menu ([`quality_options`], `all` as there) an item plays at for the `preferred` quality: the highest
/// entry not above it, so never more than the user chose (owner, 2026-10-27). "Maximum" stays "Maximum" whatever the original's
/// bitrate; a preferred bitrate at or above the original's gives "Maximum" (the original). Without the source's bitrate (none planned
/// yet, or the server gave none) the preferred quality as is, and so too when every entry is above it (a preferred step below the
/// menu's lowest).
pub fn snap_quality(preferred: Quality, source: Option<&MediaSource>, all: bool) -> Quality {
    let Quality::Bitrate(limit) = preferred else {
        return Quality::Maximum;
    };
    if source.and_then(|source| source.bitrate).is_none() {
        return preferred;
    }
    quality_options(source, all)
        .iter()
        .find(|option| option.bitrate.is_some_and(|bitrate| bitrate <= limit))
        .map_or(preferred, |option| option.quality)
}

/// Settings' quality choices, which the composition root loads and gives the controller: the preferred quality and whether the menu
/// lists every step ("Show all quality options"), which together pick the entry each item plays at ([`snap_quality`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct QualityPreference {
    pub preferred: Quality,
    pub all: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_applies_below_the_source_bitrate_or_without_one() {
        assert!(!Quality::Maximum.limits(Some(100_000_000)));
        assert!(!Quality::Maximum.limits(None));
        assert!(Quality::Bitrate(8_000_000).limits(Some(20_000_000)));
        assert!(!Quality::Bitrate(8_000_000).limits(Some(5_000_000)));
        assert!(Quality::Bitrate(8_000_000).limits(None));
    }

    #[test]
    fn pictures_are_named_as_the_server_names_them() {
        assert_eq!(resolution_name(3840, 2160), Some("4K"));
        assert_eq!(resolution_name(2560, 1440), Some("1080p"));
        assert_eq!(resolution_name(2560, 1080), Some("1080p"));
        assert_eq!(resolution_name(1920, 1080), Some("1080p"));
        assert_eq!(resolution_name(1920, 800), Some("1080p"));
        assert_eq!(resolution_name(1280, 720), Some("720p"));
        assert_eq!(resolution_name(960, 540), Some("540p"));
        assert_eq!(resolution_name(720, 576), Some("576p"));
        assert_eq!(resolution_name(640, 360), Some("360p"));
        assert_eq!(resolution_name(16_000, 9_000), None);
    }

    fn source(width: u32, height: u32, bitrate: u32) -> MediaSource {
        use current_model::{MediaStream, StreamKind, VideoFacts};
        let stream = MediaStream {
            video: Some(VideoFacts {
                width: Some(width),
                height: Some(height),
                bit_depth: None,
                frame_rate: None,
                range: None,
                range_type: None,
                dv_profile: None,
            }),
            ..crate::test_support::stream(0, StreamKind::VIDEO)
        };
        MediaSource {
            id: "1".into(),
            name: None,
            container: None,
            size: None,
            bitrate: Some(bitrate),
            duration: None,
            streams: vec![stream],
            default_audio: None,
            default_subtitle: None,
            has_segments: false,
            is_remote: false,
        }
    }

    fn names(options: &[QualityOption]) -> Vec<(Option<&'static str>, Option<u32>)> {
        options
            .iter()
            .map(|option| (option.resolution, option.bitrate))
            .collect()
    }

    #[test]
    fn only_steps_below_the_original_s_bitrate_are_offered() {
        // A 1080p original at 5.2 Mbit/s: 40 and 20 would change nothing.
        let options = quality_options(Some(&source(1920, 1080, 5_200_000)), false);
        assert_eq!(options[0].quality, Quality::Maximum);
        assert_eq!(
            names(&options),
            [
                (Some("1080p"), Some(5_200_000)),
                (Some("720p"), Some(4_000_000)),
                (Some("480p"), Some(1_000_000)),
            ]
        );
    }

    #[test]
    fn the_common_steps_are_named_as_the_server_sends_them() {
        let options = quality_options(Some(&source(3840, 2160, 60_000_000)), false);
        assert_eq!(
            names(&options),
            [
                (Some("4K"), Some(60_000_000)),
                (Some("4K"), Some(40_000_000)),
                // 20 Mbit/s: 2560 wide, which the server calls 1080p.
                (Some("1080p"), Some(20_000_000)),
                (Some("720p"), Some(4_000_000)),
                (Some("480p"), Some(1_000_000)),
            ]
        );
    }

    #[test]
    fn every_step_below_the_original_when_all_are_asked_for() {
        let options = quality_options(Some(&source(1920, 1080, 12_000_000)), true);
        assert_eq!(
            names(&options),
            [
                (Some("1080p"), Some(12_000_000)),
                (Some("1080p"), Some(10_000_000)),
                (Some("1080p"), Some(8_000_000)),
                (Some("720p"), Some(6_000_000)),
                (Some("720p"), Some(4_000_000)),
                (Some("540p"), Some(3_000_000)),
                (Some("540p"), Some(1_500_000)),
                (Some("480p"), Some(1_000_000)),
                (Some("360p"), Some(720_000)),
                // 420 kbit/s is still at most 730 kbit/s: 640 wide.
                (Some("360p"), Some(420_000)),
            ]
        );
    }

    #[test]
    fn a_step_never_names_more_than_the_original() {
        // A 1080p remux at 60 Mbit/s: the 40 Mbit/s step lowers the bitrate but not the picture.
        let options = quality_options(Some(&source(1920, 1080, 60_000_000)), false);
        assert_eq!(options[1].resolution, Some("1080p"));
        assert_eq!(options[1].bitrate, Some(40_000_000));
    }

    #[test]
    fn without_the_original_every_step_is_named_by_its_width_cap() {
        let options = quality_options(None, false);
        assert_eq!(options[0].resolution, None);
        assert_eq!(options[0].bitrate, None);
        assert_eq!(
            names(&options[1..]),
            [
                (Some("4K"), Some(40_000_000)),
                (Some("1080p"), Some(20_000_000)),
                (Some("720p"), Some(4_000_000)),
                (Some("480p"), Some(1_000_000)),
            ]
        );
    }

    #[test]
    fn two_limits_send_the_same_when_neither_applies() {
        let source = Some(20_000_000);
        assert!(Quality::Maximum.sends_same(Quality::Bitrate(40_000_000), source));
        assert!(Quality::Bitrate(4_000_000).sends_same(Quality::Bitrate(4_000_000), source));
        assert!(!Quality::Bitrate(10_000_000).sends_same(Quality::Bitrate(4_000_000), source));
        assert!(!Quality::Maximum.sends_same(Quality::Bitrate(10_000_000), source));
        // Without a known bitrate every limit applies.
        assert!(!Quality::Maximum.sends_same(Quality::Bitrate(40_000_000), None));
    }

    fn mbit(megabits: u32) -> Quality {
        Quality::Bitrate(megabits * 1_000_000)
    }

    #[test]
    fn an_item_plays_at_the_highest_entry_not_above_the_preferred_quality() {
        // The menu: Original (5.6), 720p at 4, 480p at 1. 5.6 is not above 20: the original.
        let small = source(1920, 1080, 5_600_000);
        assert_eq!(
            snap_quality(mbit(20), Some(&small), false),
            Quality::Maximum
        );
        // The menu: Original (26), 1080p at 20, 720p at 4, 480p at 1.
        let large = source(1920, 1080, 26_000_000);
        assert_eq!(snap_quality(mbit(10), Some(&large), false), mbit(4));
        assert_eq!(snap_quality(mbit(20), Some(&large), false), mbit(20));
        assert_eq!(
            snap_quality(mbit(26), Some(&large), false),
            Quality::Maximum
        );
        // With every step listed, 10 is an entry of its own.
        assert_eq!(snap_quality(mbit(10), Some(&large), true), mbit(10));
        assert_eq!(snap_quality(mbit(9), Some(&large), true), mbit(8));
    }

    #[test]
    fn maximum_stays_maximum_whatever_the_original() {
        let huge = source(3840, 2160, 120_000_000);
        assert_eq!(
            snap_quality(Quality::Maximum, Some(&huge), false),
            Quality::Maximum
        );
        assert_eq!(snap_quality(Quality::Maximum, None, true), Quality::Maximum);
    }

    #[test]
    fn without_the_source_s_bitrate_the_preferred_quality_is_used_as_is() {
        assert_eq!(snap_quality(mbit(10), None, false), mbit(10));
        let mut unknown = source(1920, 1080, 1);
        unknown.bitrate = None;
        assert_eq!(snap_quality(mbit(10), Some(&unknown), false), mbit(10));
    }

    #[test]
    fn a_preferred_quality_below_every_entry_is_used_as_is() {
        // The common menu's lowest is 1 Mbit/s; 420 kbit/s is a step of the full list only.
        let large = source(1920, 1080, 26_000_000);
        assert_eq!(
            snap_quality(Quality::Bitrate(420_000), Some(&large), false),
            Quality::Bitrate(420_000)
        );
    }
}
