//! The quality limit (ADR 0019, choice 10): a preferred quality in Settings, "Maximum" by default and the same for local and internet
//! addresses, and a per-playback menu whose pick changes only the item playing (owner, 2026-10-27). Each item plays at the highest
//! entry of its own menu that is not above the preferred quality ([`snap_quality`]). No automatic bandwidth test.
//!
//! The menu names each choice by the resolution the Jellyfin server would send this file at and its bitrate (owner, 2026-10-26),
//! reproducing the server's own rules: the audio share it sets aside, the bitrate it encodes at, the codec and frame-rate factors and
//! low-bitrate boost it scales that by, its `ResolutionNormalizer` width table and `MediaStream.GetResolutionText` names
//! ([`quality_options`]; ADR 0019, choice 10, "Names"). By default the menu offers the common resolutions, each at a bitrate the
//! server's table gives that name for H.264 at 30 fps; Settings can list every step instead.

use current_model::{Codec, MediaSource, MediaStream, StreamKind};

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

/// The jellyfin/jellyfin source the rules below reproduce: v12.2 (commit ca0f16eb), the server baseline; `master` (02c2951e,
/// 2026-10-26) is the same in every file quoted. Read 2026-10-27; the quotes are in docs/notes/playback.md ("How the server names a
/// step").
///
/// The widest picture the Jellyfin server sends for a reference bitrate: the first of `ResolutionNormalizer`'s configurations (maximum
/// width, maximum bitrate) whose bitrate is not below it; none above its last, where it sets no width
/// (`MediaBrowser.Model/Dlna/ResolutionNormalizer.cs`, `GetResolutionConfiguration`). "All bitrate here are in the scale of SDR h264
/// bitrate at 30fps": [`reference_bitrate`] brings a stream's to that scale.
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

fn width_cap(bitrate: i64) -> Option<u32> {
    WIDTH_CAPS
        .iter()
        .find(|(_, max)| bitrate <= i64::from(*max))
        .map(|(width, _)| *width)
}

/// .NET's `Convert.ToInt32` of a double: to the nearest integer, ties to even.
fn to_int(value: f64) -> i64 {
    // The values are bitrates well inside i64; `as` saturates on anything else.
    value.round_ties_even() as i64
}

/// What the server would convert a file to, for naming the steps of its quality menu ([`quality_options`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct QualityTarget<'a> {
    /// The audio track playing (the server's index); `None` for the one the server picks (the source's default, else its first
    /// default-flagged audio track, else its first; `MediaSourceInfo.GetDefaultAudioStream`).
    pub audio: Option<u32>,
    /// The video codec the server encodes to, as its session reports it once it transcodes; `None` for H.264. The device profile asks
    /// for HEVC first, but the server moves HEVC to the end unless its "Allow encoding in HEVC format" is on, which it is not by
    /// default (`MediaBrowser.Controller/MediaEncoding/EncodingHelper.cs`, `ShiftVideoCodecsIfNeeded`;
    /// `MediaBrowser.Model/Configuration/EncodingOptions.cs`, "AllowHevcEncoding = false;"). So the names can change once playback has
    /// started and the server has said what it encodes.
    pub video_codec: Option<&'a Codec>,
    /// The audio codecs the device profile's transcoding profiles list, fMP4's first (`current_jellyfin::TranscodeTargets::audio`).
    pub audio_codecs: &'a [Codec],
}

/// The audio bitrate the server sets aside from a stream of at most `max` bits per second for `track` (`StreamBuilder.GetAudioBitrate`,
/// `MediaBrowser.Model/Dlna/StreamBuilder.cs`). Once a limit applies the audio is never copied ("directAudioStreamSatisfied &&
/// !playlistItem.TranscodeReasons.HasFlag(TranscodeReason.ContainerBitrateExceedsLimit)"). Current sends no audio channel limit, so
/// the server's downmix and other-codec branches never apply: the track's own bitrate, else the default for its codec when the profile
/// lists it, else for the profile's first (`StreamInfo.TargetAudioCodec`); a mono track under 64 kbit/s at most that ("Seeing webm
/// encoding failures when source has 1 audio channel and 22k bitrate"); and never above the share the total allows
/// (`GetMaxAudioBitrateForTotalBitrate`). No track: 192 kbit/s.
fn audio_bitrate(max: i64, track: Option<&MediaStream>, audio_codecs: &[Codec]) -> i64 {
    let bitrate = match track {
        None => 192_000,
        Some(track) => {
            let codec = track
                .codec
                .as_ref()
                .filter(|codec| audio_codecs.iter().any(|listed| same_codec(listed, codec)))
                .or_else(|| audio_codecs.first());
            let bitrate = track
                .bitrate
                .map_or_else(|| default_audio_bitrate(codec), i64::from);
            let mono = track.audio.as_ref().and_then(|audio| audio.channels) == Some(1);
            if mono && track.bitrate.unwrap_or(0) < 64_000 {
                bitrate.min(64_000)
            } else {
                bitrate
            }
        }
    };
    bitrate.min(max_audio_bitrate(max))
}

/// `StreamBuilder.GetDefaultAudioBitrate` without a channel count (none is asked for, so the server counts 0): 128 kbit/s for AAC,
/// MP3, AC-3 and E-AC-3 ("if ((audioChannels ?? 0) < 2) return 128000;"), 768 kbit/s for FLAC and ALAC, else 192 kbit/s.
fn default_audio_bitrate(codec: Option<&Codec>) -> i64 {
    let is = |names: &[&str]| {
        codec.is_some_and(|codec| {
            names
                .iter()
                .any(|name| codec.as_str().eq_ignore_ascii_case(name))
        })
    };
    if is(&["aac", "mp3", "ac3", "eac3"]) {
        128_000
    } else if is(&["flac", "alac"]) {
        768_000
    } else {
        192_000
    }
}

/// `StreamBuilder.GetMaxAudioBitrateForTotalBitrate`: the most audio a total bitrate leaves room for.
fn max_audio_bitrate(total: i64) -> i64 {
    const SHARES: [(i64, i64); 8] = [
        (640_000, 128_000),
        (2_000_000, 384_000),
        (3_000_000, 448_000),
        (4_000_000, 640_000),
        (5_000_000, 768_000),
        (10_000_000, 1_536_000),
        (15_000_000, 2_304_000),
        (20_000_000, 3_584_000),
    ];
    SHARES
        .iter()
        .find(|(limit, _)| total <= *limit)
        .map_or(7_168_000, |(_, share)| *share)
}

fn same_codec(a: &Codec, b: &Codec) -> bool {
    a.as_str().eq_ignore_ascii_case(b.as_str())
}

/// `EncodingHelper.GetVideoBitrateScaleFactor`: "hevc & vp9 - 40% more efficient than h.264", "av1 - 50% more efficient".
fn codec_factor(codec: Option<&Codec>) -> f64 {
    match codec
        .map(|codec| codec.as_str().to_ascii_lowercase())
        .as_deref()
    {
        Some("h265" | "hevc" | "vp9") => 0.6,
        Some("av1") => 0.5,
        _ => 1.0,
    }
}

/// `EncodingHelper.ScaleBitrate`: `bitrate` in `input`'s terms brought to `output`'s, never lower ("Don't scale the real bitrate lower
/// than the requested bitrate"), with its low-bitrate boost (at least ×4 up to 500 kbit/s, ×3 up to 1, ×2.5 up to 2, ×2 up to
/// 3 Mbit/s) and none from 30 Mbit/s ("Don't scale beyond 30Mbps").
fn scale_bitrate(bitrate: i64, input: Option<&Codec>, output: Option<&Codec>) -> i64 {
    let mut factor = (codec_factor(output) / codec_factor(input)).max(1.0);
    if bitrate <= 500_000 {
        factor = factor.max(4.0);
    } else if bitrate <= 1_000_000 {
        factor = factor.max(3.0);
    } else if bitrate <= 2_000_000 {
        factor = factor.max(2.5);
    } else if bitrate <= 3_000_000 {
        factor = factor.max(2.0);
    } else if bitrate >= 30_000_000 {
        factor = 1.0;
    }
    #[allow(clippy::cast_precision_loss)] // Bitrates are far below 2^52.
    to_int(factor * bitrate as f64)
}

/// The video bitrate the server encodes at for a request of `requested` (`EncodingHelper.GetVideoBitrateParamValue`): at most the
/// source's own video bitrate, raised for small sources (`GetMinBitrate`: "these values were chosen from testing to improve low
/// bitrate streams", ×2.5 up to 2 Mbit/s and ×2 up to 3), compensated for a source above 8 bits (`GetBitDepthCompensatedBitrate`)
/// and scaled for the codecs ([`scale_bitrate`]), and never above the request ("If a max bitrate was requested, don't let the scaled
/// bitrate exceed it").
fn video_bitrate(requested: i64, video: Option<&MediaStream>, output: Option<&Codec>) -> i64 {
    let Some(video) = video else {
        return requested;
    };
    let mut bitrate = requested;
    if let Some(source) = video.bitrate.map(i64::from) {
        #[allow(clippy::cast_precision_loss)] // Bitrates are far below 2^52.
        let raised = if source <= 2_000_000 {
            to_int(source as f64 * 2.5)
        } else if source <= 3_000_000 {
            source * 2
        } else {
            source
        };
        bitrate = raised.min(requested);
        // `GetBitDepthCompensatedBitrate(bitrate, videoStream, 8)`: unchanged for a source of at most 8 bits.
        let bit_depth = video.video.as_ref().and_then(|facts| facts.bit_depth);
        if bit_depth.is_none_or(|depth| depth > 8) {
            let factor = match source {
                ..=4_000_000 => 1.5,
                4_000_001..=8_000_000 => 1.2,
                8_000_001..=12_000_000 => 1.1,
                _ => 1.05,
            };
            #[allow(clippy::cast_precision_loss)] // Bitrates are far below 2^52.
            {
                bitrate = to_int(bitrate as f64 * factor);
            }
        }
    }
    scale_bitrate(bitrate, video.codec.as_ref(), output).min(requested)
}

/// `ResolutionNormalizer.Normalize`'s reference bitrate: an H.264-equivalent bitrate brought to 30 frames per second ("Our reference
/// bitrate is based on SDR h264 at 30fps"): ×30/fps up to 30 fps, ×1/√(fps/30) above, in single precision as the server computes it.
/// Without a frame rate, 30. The server passes no HDR flag ("We are not doing HDR transcoding for now"), so HDR changes nothing.
fn reference_bitrate(h264_equivalent: i64, fps: Option<f32>) -> i64 {
    let fps = fps.unwrap_or(30.0);
    let scale = if fps <= 30.0 {
        30.0 / fps
    } else {
        1.0 / (fps / 30.0).sqrt()
    };
    #[allow(clippy::cast_precision_loss)]
    // Bitrates are far below 2^24 × 2^8; the server rounds in f32 too.
    let reference = h264_equivalent as f32 * scale;
    to_int(f64::from(reference))
}

/// The picture the server sends for a source of `size` with a maximum width of `cap` (`EncodingHelper.GetSwScaleFilter`, a maximum
/// width only: "scale=trunc(min(max(iw\,ih*a)\,{maxWidth})/2)*2:trunc(ow/a/2)*2"): never wider than the source, the height in
/// proportion, both even. Square pixels are assumed (Current does not know a stream's pixel aspect).
fn scaled(size: (u32, u32), cap: u32) -> (u32, u32) {
    let (width, height) = size;
    let scaled_width = width.min(cap) / 2 * 2;
    let scaled_height =
        u32::try_from(u64::from(height) * u64::from(scaled_width) / u64::from(width) / 2 * 2)
            .unwrap_or(height);
    (scaled_width, scaled_height)
}

/// The picture the Jellyfin server sends `source` at for a `MaxStreamingBitrate` of `max`, with `target`'s audio track and output
/// codec (`Jellyfin.Api/Helpers/StreamingHelpers.cs`, `GetStreamingState`): the video's share of the step ([`audio_bitrate`]; "Make
/// sure the video bitrate is lower than bitrate settings but at least 64k"), the bitrate it encodes at ([`video_bitrate`]), then the
/// width cap for its H.264-equivalent at 30 fps ([`scale_bitrate`] to H.264, [`reference_bitrate`]). It keeps the source's size when
/// the video's share is not below the source's video bitrate ("Don't downscale the resolution if … the requested video bitrate is
/// greater than source video bitrate"), and never upscales ([`scaled`]). Without the source's size, the width cap at 16:9.
fn sent_picture(source: &MediaSource, target: &QualityTarget<'_>, max: i64) -> Option<(u32, u32)> {
    let video = source.video();
    let size = video
        .and_then(|stream| stream.video.as_ref())
        .and_then(|facts| facts.width.zip(facts.height))
        .filter(|(width, height)| *width > 0 && *height > 0);
    let track = target
        .audio
        .or(source.default_audio)
        .and_then(|index| source.stream(index))
        .filter(|stream| stream.kind == StreamKind::AUDIO)
        .or_else(|| {
            source
                .streams_of(&StreamKind::AUDIO)
                .find(|stream| stream.is_default)
        })
        .or_else(|| source.streams_of(&StreamKind::AUDIO).next());
    let requested = (max - audio_bitrate(max, track, target.audio_codecs)).max(64_000);
    let source_video = video.and_then(|stream| stream.bitrate).map(i64::from);
    if let Some(size) = size
        && source_video.is_some_and(|source_video| requested >= source_video)
    {
        return Some(size);
    }
    let h264 = Codec::H264;
    let output = target.video_codec.unwrap_or(&h264);
    let bitrate = video_bitrate(requested, video, Some(output));
    let fps = video
        .and_then(|stream| stream.video.as_ref())
        .and_then(|facts| facts.reference_frame_rate)
        .filter(|fps| *fps > 0.0);
    let cap = width_cap(reference_bitrate(
        scale_bitrate(bitrate, Some(output), Some(&h264)),
        fps,
    ));
    match (size, cap) {
        (Some(size), Some(cap)) => Some(scaled(size, cap)),
        (Some(size), None) => Some(size),
        (None, Some(cap)) => Some((cap, cap * 9 / 16)),
        (None, None) => None,
    }
}

/// A picture's name as the Jellyfin server gives it (`MediaBrowser.Model/Entities/MediaStream.cs`, `GetResolutionText`, progressive):
/// by its width, with its height no more than the size's; `None` for one larger than 8K.
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
/// ([`Quality::limits`]), so it is left out (owner, 2026-10-26); without a known bitrate every step is listed. Each step is named by
/// the picture the server would send this file at with `target`'s audio track and output codec (ADR 0019, choice 10, "Names").
/// Without a source (Settings' list), each step by its own bitrate's width cap at 16:9: H.264 at 30 fps with no audio share.
pub fn quality_options(
    source: Option<&MediaSource>,
    target: &QualityTarget<'_>,
    all: bool,
) -> Vec<QualityOption> {
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
                let picture = match source {
                    Some(source) => sent_picture(source, target, i64::from(bitrate)),
                    None => width_cap(i64::from(bitrate)).map(|cap| (cap, cap * 9 / 16)),
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
    // The names play no part: only the entries' bitrates.
    quality_options(source, &QualityTarget::default(), all)
        .iter()
        .find(|option| option.bitrate.is_some_and(|bitrate| bitrate <= limit))
        .map_or(preferred, |option| option.quality)
}

/// Settings' "Preferred quality" list (owner, 2026-10-27): the quality menu without a file ([`quality_options`] with no source, `all`
/// as there), so "Maximum" (Original) and every step named by its width cap. The preferred quality is always one of them
/// ([`listed_quality`]).
pub fn preferred_quality_options(all: bool) -> Vec<QualityOption> {
    quality_options(None, &QualityTarget::default(), all)
}

/// The preferred quality once Settings lists the steps `all` says (owner, 2026-10-27): itself when the list has it, else the nearest
/// listed step not above it, so the items never play above what the user chose; below every step, the lowest. "Show all quality
/// options" switched off with a step only it lists as the preferred one moves the setting to this.
pub fn listed_quality(preferred: Quality, all: bool) -> Quality {
    let Quality::Bitrate(bitrate) = preferred else {
        return Quality::Maximum;
    };
    let steps: &[Quality] = if all {
        &Quality::ALL_STEPS
    } else {
        &Quality::STEPS
    };
    if steps.contains(&preferred) {
        return preferred;
    }
    steps
        .iter()
        .copied()
        .find(|step| matches!(step, Quality::Bitrate(step) if *step <= bitrate))
        .or_else(|| steps.last().copied())
        .unwrap_or(preferred)
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
                reference_frame_rate: None,
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
        let options = quality_options(
            Some(&source(1920, 1080, 5_200_000)),
            &QualityTarget::default(),
            false,
        );
        assert_eq!(options[0].quality, Quality::Maximum);
        assert_eq!(
            names(&options),
            [
                (Some("1080p"), Some(5_200_000)),
                (Some("720p"), Some(4_000_000)),
                // 192 kbit/s for audio (no track), then ×3 for a stream under 1 Mbit/s: 2.4, so 960 wide.
                (Some("540p"), Some(1_000_000)),
            ]
        );
    }

    #[test]
    fn the_preferred_quality_list_is_original_then_the_steps() {
        let options = preferred_quality_options(false);
        assert_eq!(options[0].quality, Quality::Maximum);
        assert_eq!(options[0].bitrate, None);
        assert_eq!(
            options[1..]
                .iter()
                .map(|option| option.quality)
                .collect::<Vec<_>>(),
            Quality::STEPS
        );
    }

    #[test]
    fn a_preferred_step_the_common_list_lacks_moves_to_the_nearest_below() {
        // "Show all" off with 10 Mbit/s preferred: 4 (20 would be above it).
        assert_eq!(listed_quality(mbit(10), false), mbit(4));
        assert_eq!(listed_quality(mbit(60), false), mbit(40));
        assert_eq!(listed_quality(Quality::Bitrate(500_000), false), mbit(1));
        // Listed steps and Original stay.
        assert_eq!(listed_quality(mbit(20), false), mbit(20));
        assert_eq!(listed_quality(Quality::Maximum, false), Quality::Maximum);
        assert_eq!(listed_quality(mbit(10), true), mbit(10));
    }

    #[test]
    fn a_step_at_the_original_s_bitrate_is_not_offered() {
        // An original at exactly 4 Mbit/s: the 4 Mbit/s step would send the same, so only the steps below it follow "Original".
        let options = quality_options(
            Some(&source(1280, 720, 4_000_000)),
            &QualityTarget::default(),
            false,
        );
        assert_eq!(
            names(&options),
            [
                (Some("720p"), Some(4_000_000)),
                (Some("540p"), Some(1_000_000))
            ]
        );
        assert_eq!(options[0].quality, Quality::Maximum);
    }

    #[test]
    fn the_common_steps_are_named_as_the_server_sends_them() {
        let options = quality_options(
            Some(&source(3840, 2160, 60_000_000)),
            &QualityTarget::default(),
            false,
        );
        assert_eq!(
            names(&options),
            [
                (Some("4K"), Some(60_000_000)),
                (Some("4K"), Some(40_000_000)),
                // 20 Mbit/s: 2560 wide, which the server calls 1080p.
                (Some("1080p"), Some(20_000_000)),
                (Some("720p"), Some(4_000_000)),
                (Some("540p"), Some(1_000_000)),
            ]
        );
    }

    #[test]
    fn every_step_below_the_original_when_all_are_asked_for() {
        let options = quality_options(
            Some(&source(1920, 1080, 12_000_000)),
            &QualityTarget::default(),
            true,
        );
        assert_eq!(
            names(&options),
            [
                (Some("1080p"), Some(12_000_000)),
                (Some("1080p"), Some(10_000_000)),
                (Some("1080p"), Some(8_000_000)),
                (Some("720p"), Some(6_000_000)),
                (Some("720p"), Some(4_000_000)),
                // The low-bitrate boost: 2.808 Mbit/s of video ×2 and 1.308 ×2.5 are both 1280 wide.
                (Some("720p"), Some(3_000_000)),
                (Some("720p"), Some(1_500_000)),
                // 0.528 (720 kbit/s less 192 for audio) ×3 and 0.292 (420 less 128, the most audio 420 leaves room for) ×4: 960.
                (Some("540p"), Some(1_000_000)),
                (Some("540p"), Some(720_000)),
                (Some("540p"), Some(420_000)),
            ]
        );
    }

    #[test]
    fn a_step_never_names_more_than_the_original() {
        // A 1080p remux at 60 Mbit/s: the 40 Mbit/s step lowers the bitrate but not the picture.
        let options = quality_options(
            Some(&source(1920, 1080, 60_000_000)),
            &QualityTarget::default(),
            false,
        );
        assert_eq!(options[1].resolution, Some("1080p"));
        assert_eq!(options[1].bitrate, Some(40_000_000));
    }

    #[test]
    fn without_the_original_every_step_is_named_by_its_width_cap() {
        let options = quality_options(None, &QualityTarget::default(), false);
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

    /// The audio codecs Current's device profile lists for the Windows build (fMP4's, then MPEG-TS's MP3).
    const AUDIO_TARGETS: [Codec; 6] = [
        Codec::AAC,
        Codec::AC3,
        Codec::EAC3,
        Codec::OPUS,
        Codec::FLAC,
        Codec::MP3,
    ];

    fn h264_out() -> QualityTarget<'static> {
        QualityTarget {
            audio: None,
            video_codec: None,
            audio_codecs: &AUDIO_TARGETS,
        }
    }

    /// A file: its video stream (index 0) and its audio tracks (index 1 on, the first the default), and its total bitrate.
    struct File {
        size: Option<(u32, u32)>,
        fps: Option<f32>,
        codec: Codec,
        bit_depth: Option<u32>,
        video_bitrate: Option<u32>,
        audio: Vec<(Codec, u32, Option<u32>)>,
        bitrate: u32,
    }

    impl File {
        fn new(width: u32, height: u32, fps: f32, bitrate: u32) -> Self {
            Self {
                size: Some((width, height)),
                fps: Some(fps),
                codec: Codec::H264,
                bit_depth: Some(8),
                video_bitrate: None,
                audio: Vec::new(),
                bitrate,
            }
        }

        fn video(mut self, codec: Codec, bit_depth: u32, bitrate: u32) -> Self {
            self.codec = codec;
            self.bit_depth = Some(bit_depth);
            self.video_bitrate = Some(bitrate);
            self
        }

        fn audio(mut self, codec: Codec, channels: u32, bitrate: Option<u32>) -> Self {
            self.audio.push((codec, channels, bitrate));
            self
        }

        fn source(&self) -> MediaSource {
            use current_model::{AudioFacts, VideoFacts};
            let mut streams = vec![MediaStream {
                codec: Some(self.codec.clone()),
                bitrate: self.video_bitrate,
                video: Some(VideoFacts {
                    width: self.size.map(|size| size.0),
                    height: self.size.map(|size| size.1),
                    bit_depth: self.bit_depth,
                    frame_rate: self.fps,
                    reference_frame_rate: self.fps,
                    range: None,
                    range_type: None,
                    dv_profile: None,
                }),
                ..crate::test_support::stream(0, StreamKind::VIDEO)
            }];
            for (index, (codec, channels, bitrate)) in (1..).zip(&self.audio) {
                streams.push(MediaStream {
                    codec: Some(codec.clone()),
                    bitrate: *bitrate,
                    is_default: index == 1,
                    audio: Some(AudioFacts {
                        channels: Some(*channels),
                        layout: None,
                        sample_rate: None,
                    }),
                    ..crate::test_support::stream(index, StreamKind::AUDIO)
                });
            }
            MediaSource {
                streams,
                default_audio: (!self.audio.is_empty()).then_some(1),
                ..source(1, 1, self.bitrate)
            }
        }

        /// The names of the menu's steps (not "Original"), with `target`, the common steps or `all`.
        fn steps(
            &self,
            target: &QualityTarget<'_>,
            all: bool,
        ) -> Vec<(Option<&'static str>, Option<u32>)> {
            names(&quality_options(Some(&self.source()), target, all)[1..])
        }
    }

    fn track(codec: Codec, channels: u32, bitrate: Option<u32>) -> MediaStream {
        let file = File::new(1920, 1080, 24.0, 1).audio(codec, channels, bitrate);
        file.source().streams.remove(1)
    }

    #[test]
    fn the_audio_share_is_the_track_s_bitrate_within_what_the_step_leaves_room_for() {
        let ac3 = track(Codec::AC3, 6, Some(640_000));
        assert_eq!(
            audio_bitrate(40_000_000, Some(&ac3), &AUDIO_TARGETS),
            640_000
        );
        assert_eq!(
            audio_bitrate(4_000_000, Some(&ac3), &AUDIO_TARGETS),
            640_000
        );
        assert_eq!(
            audio_bitrate(3_000_000, Some(&ac3), &AUDIO_TARGETS),
            448_000
        );
        assert_eq!(
            audio_bitrate(1_000_000, Some(&ac3), &AUDIO_TARGETS),
            384_000
        );
        assert_eq!(audio_bitrate(420_000, Some(&ac3), &AUDIO_TARGETS), 128_000);
        // A lossless track is not copied either: its bitrate, capped by the step.
        let truehd = track(Codec::TRUEHD, 8, Some(4_500_000));
        assert_eq!(
            audio_bitrate(40_000_000, Some(&truehd), &AUDIO_TARGETS),
            4_500_000
        );
        assert_eq!(
            audio_bitrate(20_000_000, Some(&truehd), &AUDIO_TARGETS),
            3_584_000
        );
        assert_eq!(
            audio_bitrate(15_000_000, Some(&truehd), &AUDIO_TARGETS),
            2_304_000
        );
        assert_eq!(
            audio_bitrate(6_000_000, Some(&truehd), &AUDIO_TARGETS),
            1_536_000
        );
        assert_eq!(
            audio_bitrate(5_000_000, Some(&truehd), &AUDIO_TARGETS),
            768_000
        );
        // No track: 192 kbit/s.
        assert_eq!(audio_bitrate(4_000_000, None, &AUDIO_TARGETS), 192_000);
        assert_eq!(audio_bitrate(420_000, None, &AUDIO_TARGETS), 128_000);
    }

    #[test]
    fn without_the_track_s_bitrate_its_codec_s_default_whatever_its_channels() {
        let default = |codec, channels| {
            audio_bitrate(
                40_000_000,
                Some(&track(codec, channels, None)),
                &AUDIO_TARGETS,
            )
        };
        // Listed: its own codec's default; the server counts no channels, as none is asked for.
        assert_eq!(default(Codec::AC3, 6), 128_000);
        assert_eq!(default(Codec::AAC, 2), 128_000);
        assert_eq!(default(Codec::FLAC, 2), 768_000);
        assert_eq!(default(Codec::OPUS, 6), 192_000);
        // Not listed: the profile's first, AAC.
        assert_eq!(default(Codec::TRUEHD, 8), 128_000);
        assert_eq!(default(Codec::DTS, 6), 128_000);
        assert_eq!(default(Codec::ALAC, 2), 128_000);
        // No transcode target at all: 192.
        let truehd = track(Codec::TRUEHD, 8, None);
        assert_eq!(audio_bitrate(40_000_000, Some(&truehd), &[]), 192_000);
    }

    #[test]
    fn a_mono_track_under_64_kbits_is_held_to_64() {
        let low = track(Codec::AAC, 1, Some(22_000));
        assert_eq!(audio_bitrate(4_000_000, Some(&low), &AUDIO_TARGETS), 22_000);
        // Unknown counts as under: the default of 128 is held to 64.
        let unknown = track(Codec::AAC, 1, None);
        assert_eq!(
            audio_bitrate(4_000_000, Some(&unknown), &AUDIO_TARGETS),
            64_000
        );
        let stereo = track(Codec::AAC, 2, None);
        assert_eq!(
            audio_bitrate(4_000_000, Some(&stereo), &AUDIO_TARGETS),
            128_000
        );
    }

    #[test]
    fn the_video_keeps_at_least_64_kbits() {
        // 150 kbit/s less 128 for audio is 22: 64, ×4 is 256 kbit/s, so 416 wide.
        let file = File::new(1920, 1080, 30.0, 10_000_000).audio(Codec::AAC, 2, Some(128_000));
        assert_eq!(
            sent_picture(&file.source(), &h264_out(), 150_000),
            Some((416, 234))
        );
    }

    #[test]
    fn scale_bitrate_brings_a_codec_s_bitrate_to_another_s_with_its_low_bitrate_boost() {
        let (h264, hevc, av1) = (Some(&Codec::H264), Some(&Codec::HEVC), Some(&Codec::AV1));
        assert_eq!(scale_bitrate(4_000_000, h264, h264), 4_000_000);
        assert_eq!(scale_bitrate(4_000_000, hevc, h264), 6_666_667);
        assert_eq!(scale_bitrate(4_000_000, Some(&Codec::VP9), h264), 6_666_667);
        assert_eq!(scale_bitrate(4_000_000, av1, h264), 8_000_000);
        assert_eq!(scale_bitrate(4_000_000, av1, hevc), 4_800_000);
        // Never lower than asked.
        assert_eq!(scale_bitrate(4_000_000, h264, hevc), 4_000_000);
        // The boost, whatever the codecs, unless they ask for more.
        assert_eq!(scale_bitrate(3_000_000, h264, h264), 6_000_000);
        assert_eq!(scale_bitrate(2_000_000, h264, h264), 5_000_000);
        assert_eq!(scale_bitrate(2_000_000, av1, h264), 5_000_000);
        assert_eq!(scale_bitrate(1_000_000, h264, h264), 3_000_000);
        assert_eq!(scale_bitrate(500_000, h264, h264), 2_000_000);
        // None from 30 Mbit/s.
        assert_eq!(scale_bitrate(30_000_000, hevc, h264), 30_000_000);
        assert_eq!(scale_bitrate(29_999_999, hevc, h264), 49_999_998);
    }

    #[test]
    fn the_reference_bitrate_is_at_30_frames_per_second() {
        assert_eq!(reference_bitrate(4_000_000, Some(30.0)), 4_000_000);
        assert_eq!(reference_bitrate(4_000_000, None), 4_000_000);
        assert_eq!(reference_bitrate(4_000_000, Some(24.0)), 5_000_000);
        assert_eq!(reference_bitrate(4_000_000, Some(25.0)), 4_800_000);
        assert_eq!(reference_bitrate(4_000_000, Some(60.0)), 2_828_427);
        assert_eq!(reference_bitrate(4_000_000, Some(120.0)), 2_000_000);
    }

    #[test]
    fn the_video_bitrate_is_the_request_unless_a_small_source_s_is_below_it() {
        let video = |bitrate, bit_depth| {
            let file = File::new(1920, 1080, 30.0, 1).video(Codec::H264, bit_depth, bitrate);
            file.source().streams.remove(0)
        };
        let h264 = Some(&Codec::H264);
        assert_eq!(
            video_bitrate(3_360_000, Some(&video(10_000_000, 8)), h264),
            3_360_000
        );
        // 100 kbit/s raised ×2.5 to 250, ×4 for the codecs' low-bitrate boost: 1 Mbit/s, below the 1.308 asked for.
        assert_eq!(
            video_bitrate(1_308_000, Some(&video(100_000, 8)), h264),
            1_000_000
        );
        // A 10-bit source's is raised ×1.5 more: 1.5 Mbit/s, held to the request.
        assert_eq!(
            video_bitrate(1_308_000, Some(&video(100_000, 10)), h264),
            1_308_000
        );
        assert_eq!(video_bitrate(1_308_000, None, h264), 1_308_000);
    }

    #[test]
    fn a_small_source_of_unknown_size_is_named_by_its_own_video_bitrate() {
        // 1.5 Mbit/s less 192 for audio is 1.308, ×2.5 is 3.27: 1280 wide. The source's own 100 kbit/s gives 1 Mbit/s, ×3: 960.
        let mut file = File::new(1, 1, 30.0, 2_000_000).video(Codec::H264, 8, 100_000);
        file.size = None;
        let source = file.source();
        assert_eq!(
            sent_picture(&source, &h264_out(), 1_500_000),
            Some((960, 540))
        );
        file.video_bitrate = None;
        assert_eq!(
            sent_picture(&file.source(), &h264_out(), 1_500_000),
            Some((1280, 720))
        );
    }

    #[test]
    fn the_source_s_size_stays_when_the_video_s_share_is_not_below_its_own() {
        // 4 Mbit/s less 640 for audio is 3.36, above the source's 3 Mbit/s of video: no downscale.
        let file = File::new(1920, 1080, 24.0, 5_200_000)
            .video(Codec::H264, 8, 3_000_000)
            .audio(Codec::FLAC, 2, Some(1_500_000));
        assert_eq!(
            sent_picture(&file.source(), &h264_out(), 4_000_000),
            Some((1920, 1080))
        );
        assert_eq!(
            file.steps(&h264_out(), false)[0],
            (Some("1080p"), Some(4_000_000))
        );
    }

    #[test]
    fn a_picture_is_never_wider_than_the_source_and_keeps_its_aspect() {
        // A 2.39:1 film at 1280 wide: 1280 × 535.6, made even.
        let scope = File::new(1920, 804, 24.0, 10_000_000).audio(Codec::AC3, 6, Some(640_000));
        assert_eq!(
            sent_picture(&scope.source(), &h264_out(), 4_000_000),
            Some((1280, 536))
        );
        assert_eq!(
            scope.steps(&h264_out(), false)[0],
            (Some("720p"), Some(4_000_000))
        );
        // A PAL DVD at 4 Mbit/s would be 1280 wide: it stays 720 × 576.
        let dvd = File::new(720, 576, 25.0, 8_000_000)
            .video(Codec::MPEG2, 8, 7_500_000)
            .audio(Codec::AC3, 6, Some(448_000));
        assert_eq!(
            sent_picture(&dvd.source(), &h264_out(), 4_000_000),
            Some((720, 576))
        );
        assert_eq!(
            dvd.steps(&h264_out(), false)[0],
            (Some("576p"), Some(4_000_000))
        );
    }

    #[test]
    fn hevc_output_counts_for_more_than_h264() {
        let film = File::new(1920, 1080, 23.976, 10_640_000)
            .video(Codec::H264, 8, 10_000_000)
            .audio(Codec::AC3, 6, Some(640_000));
        let hevc = QualityTarget {
            video_codec: Some(&Codec::HEVC),
            ..h264_out()
        };
        // 3.36 Mbit/s of HEVC is 5.6 of H.264, ×1.25 for 24 fps: 7.007, so 1920 wide.
        assert_eq!(
            film.steps(&hevc, false),
            [
                (Some("1080p"), Some(4_000_000)),
                // At 1 Mbit/s the low-bitrate boost (×3) is above HEVC's ×1.67, so the codec changes nothing there.
                (Some("540p"), Some(1_000_000))
            ]
        );
        assert_eq!(
            film.steps(&h264_out(), false),
            [
                (Some("720p"), Some(4_000_000)),
                (Some("540p"), Some(1_000_000))
            ]
        );
    }

    #[test]
    fn the_source_s_codec_plays_no_part_when_the_server_encodes_h264() {
        let film = |codec| {
            File::new(1920, 1080, 23.976, 10_640_000)
                .video(codec, 10, 10_000_000)
                .audio(Codec::AC3, 6, Some(640_000))
        };
        let h264 = film(Codec::H264).steps(&h264_out(), true);
        assert_eq!(film(Codec::HEVC).steps(&h264_out(), true), h264);
        assert_eq!(film(Codec::AV1).steps(&h264_out(), true), h264);
    }

    #[test]
    fn the_chosen_audio_track_s_share_counts() {
        // A 4K film at 24 fps with TrueHD 7.1 (the default) and AC-3 5.1. At 6 Mbit/s TrueHD takes 1.536, AC-3 0.64: 4.464 ×1.25
        // is 5.59 (1280 wide), 5.36 ×1.25 is 6.71 (1920 wide).
        let film = File::new(3840, 2160, 24.0, 60_000_000)
            .video(Codec::HEVC, 10, 55_000_000)
            .audio(Codec::TRUEHD, 8, Some(4_500_000))
            .audio(Codec::AC3, 6, Some(640_000));
        let six = |target: &QualityTarget<'_>| {
            film.steps(target, true)
                .into_iter()
                .find(|(_, bitrate)| *bitrate == Some(6_000_000))
        };
        assert_eq!(six(&h264_out()), Some((Some("720p"), Some(6_000_000))));
        let ac3 = QualityTarget {
            audio: Some(2),
            ..h264_out()
        };
        assert_eq!(six(&ac3), Some((Some("1080p"), Some(6_000_000))));
    }

    #[test]
    fn a_film_at_24_fps_is_sent_larger_at_1_mbits() {
        // 1 Mbit/s less 384 for AC-3 is 616 kbit/s, ×3 and ×1.25 for 24 fps: 2.31, so 960 wide (H.264 at 30 fps alone said 768).
        let film = File::new(1920, 1080, 23.976, 10_640_000)
            .video(Codec::H264, 8, 10_000_000)
            .audio(Codec::AC3, 6, Some(640_000));
        assert_eq!(
            film.steps(&h264_out(), false)[1],
            (Some("540p"), Some(1_000_000))
        );
    }

    #[test]
    fn a_4k_file_at_60_fps_is_sent_at_1080p_at_40_mbits() {
        // 40 Mbit/s less 640 for E-AC-3 is 39.36, ×0.707 for 60 fps: 27.8, so 2560 wide, which the server calls 1080p.
        let file = File::new(3840, 2160, 59.94, 46_000_000)
            .video(Codec::HEVC, 10, 45_000_000)
            .audio(Codec::EAC3, 6, Some(640_000));
        assert_eq!(
            file.steps(&h264_out(), false),
            [
                (Some("1080p"), Some(40_000_000)),
                (Some("1080p"), Some(20_000_000)),
                (Some("540p"), Some(4_000_000)),
                (Some("540p"), Some(1_000_000)),
            ]
        );
    }
}
