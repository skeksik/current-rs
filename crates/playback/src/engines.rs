//! The engines a player has (ADR 0025, "How the core decides"): up to two, the native one and the app engine, each with what it
//! plays, and whether the platform has a remux on the device and what it reads. The device profile sent to the server is their
//! union, so the server offers direct play whenever any local route can play the file.
//!
//! Capabilities stay logic in the core: the composition root makes them from the facts each engine reports (for libmpv its decoder
//! and demuxer lists, for a platform engine the facts its UI reports, ADR 0025 "How a platform engine plugs into the core").

use current_model::{CodecSupport, Container, EngineCapabilities, VideoRangeType};

/// What one engine plays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineProfile {
    pub capabilities: EngineCapabilities,
    /// Whether it plays HLS (a server stream's playlist).
    pub hls: bool,
}

impl EngineProfile {
    pub fn new(capabilities: EngineCapabilities, hls: bool) -> Self {
        Self { capabilities, hls }
    }
}

/// The remux on the device (ADR 0025, route 2): what it reads, for the native engine, which then takes the streams it copies. None on
/// Windows (departure 1); Apple's (MKV to fMP4 HLS) comes with M3. Only its containers are checked: which streams it may copy (ADR
/// 0025, "Engines": "containers it demuxes, streams it may copy") is left to the Apple remux spike (M3-2) and its ADR.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRemux {
    /// Containers it reads.
    pub containers: Vec<Container>,
}

/// The platform's own rules for the native route, from ADR 0025's departures. All off unless the composition root sets them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PlatformRules {
    /// Departure 2 (Windows): a Dolby Vision source plays on the app engine first. Its exception, native on a display in Dolby
    /// Vision mode with the extension installed, was not testable in W1 and has no display fact yet, so it is not here (ADR 0025,
    /// departure 2).
    pub dolby_vision_on_app_engine: bool,
    /// Departure 8 (Windows): a PQ source (HDR10, HDR10+, Dolby Vision; not HLG) on an SDR display plays on the app engine first.
    pub pq_on_sdr_on_app_engine: bool,
    /// The subtitle formats that need rendering (picture formats, styled ASS) which the native route still shows, by the server's
    /// names: an exception to choice 1, which sends them to the app engine, made per platform by a departure (Windows: PGS,
    /// departure 3). Empty elsewhere. The native engine must also list the format in its capabilities.
    pub native_rendered_subtitles: &'static [&'static str],
    /// The subtitle formats the native engine takes as a file (an external subtitle, or one the server delivers beside its stream),
    /// by the server's names; a subtitle file in any other format plays on the app engine. When set, this list, not the engine's
    /// capabilities (its formats inside a file), decides for files. Empty: no rule (any format the native engine lists). Windows: the
    /// text formats `TimedTextSource` reads (W5), not PGS, which only arrives inside the file.
    pub native_external_subtitles: &'static [&'static str],
}

impl PlatformRules {
    /// Windows' rules (ADR 0025, departures 2, 3 and 8).
    pub const WINDOWS: Self = Self {
        dolby_vision_on_app_engine: true,
        pq_on_sdr_on_app_engine: true,
        native_rendered_subtitles: &["pgssub"],
        native_external_subtitles: &["subrip", "srt", "webvtt", "vtt"],
    };
}

/// The engines of a player. The app engine is always there today; the web, which has none (departure 7), comes later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Engines {
    pub native: Option<EngineProfile>,
    pub app: EngineProfile,
    /// Only with a native engine.
    pub remux: Option<DeviceRemux>,
    pub rules: PlatformRules,
}

impl Engines {
    /// A player with the app engine only: today's (ADR 0019), where the planner takes the server's answer as it is.
    pub fn app_only(capabilities: EngineCapabilities) -> Self {
        Self {
            native: None,
            app: EngineProfile::new(capabilities, true),
            remux: None,
            rules: PlatformRules::default(),
        }
    }

    /// What the device profile sends: the union of the native engine, the remux's containers with the native engine's codecs, and
    /// the app engine (ADR 0025). With the app engine only, exactly its capabilities.
    ///
    /// The server's direct-play profiles pair every container with every codec, so the union can offer direct play for a file no
    /// single route plays (a container only the native engine reads with a codec only the app engine decodes); the per-file check
    /// then finds no local route and the plan is asked again without direct play.
    pub fn device_profile(&self) -> EngineCapabilities {
        let mut union = self.app.capabilities.clone();
        if let Some(native) = &self.native {
            merge(&mut union, &native.capabilities);
            if let Some(remux) = &self.remux {
                for container in &remux.containers {
                    if !union.containers.contains(container) {
                        union.containers.push(container.clone());
                    }
                }
            }
        }
        union
    }
}

/// Adds `other` to `into`: what either plays, each codec once with the looser of its two limits.
fn merge(into: &mut EngineCapabilities, other: &EngineCapabilities) {
    for container in &other.containers {
        if !into.containers.contains(container) {
            into.containers.push(container.clone());
        }
    }
    merge_codecs(&mut into.video, &other.video);
    merge_codecs(&mut into.audio, &other.audio);
    for format in &other.subtitles {
        if !into.subtitles.contains(format) {
            into.subtitles.push(format.clone());
        }
    }
    into.external_subtitles |= other.external_subtitles;
}

fn merge_codecs(into: &mut Vec<CodecSupport>, other: &[CodecSupport]) {
    for support in other {
        match into.iter_mut().find(|known| known.codec == support.codec) {
            Some(known) => *known = looser(known, support),
            None => into.push(support.clone()),
        }
    }
}

/// The limits either engine accepts: no limit when one has none.
fn looser(a: &CodecSupport, b: &CodecSupport) -> CodecSupport {
    fn both<T>(a: &Option<T>, b: &Option<T>, pick: impl Fn(&T, &T) -> T) -> Option<T> {
        match (a, b) {
            (Some(a), Some(b)) => Some(pick(a, b)),
            _ => None,
        }
    }
    fn joined<T: Clone + PartialEq>(a: &[T], b: &[T]) -> Vec<T> {
        let mut all = a.to_vec();
        all.extend(b.iter().filter(|value| !a.contains(value)).cloned());
        all
    }
    CodecSupport {
        codec: a.codec.clone(),
        max_bit_depth: both(&a.max_bit_depth, &b.max_bit_depth, |a, b| *a.max(b)),
        profiles: both(&a.profiles, &b.profiles, |a, b| joined(a, b)),
        level: both(&a.level, &b.level, |a, b| *a.max(b)),
        ranges: both(&a.ranges, &b.ranges, |a: &Vec<VideoRangeType>, b| {
            joined(a, b)
        }),
        max_channels: both(&a.max_channels, &b.max_channels, |a, b| *a.max(b)),
    }
}

/// The display the player's window is on, as far as departure 8 needs it: the UI reports it (on Windows from `AdvancedColorKind`,
/// W4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DisplayKind {
    /// Not reported yet. Departure 8 then leaves the rule unchanged (native); the Windows UI always reports the display (W4).
    #[default]
    Unknown,
    Sdr,
    Hdr,
}

/// Settings → Playback → "Player" (ADR 0025, choice 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EnginePreference {
    /// The route order of ADR 0025.
    #[default]
    Automatic,
    /// The app engine, then the server.
    AlwaysAppEngine,
}

#[cfg(test)]
mod tests {
    use super::*;
    use current_model::{Codec, SubtitleFormat};

    fn caps(
        containers: &[Container],
        video: Vec<CodecSupport>,
        audio: &[Codec],
        subtitles: &[SubtitleFormat],
    ) -> EngineCapabilities {
        EngineCapabilities {
            containers: containers.to_vec(),
            video,
            audio: audio.iter().cloned().map(CodecSupport::any).collect(),
            subtitles: subtitles.to_vec(),
            external_subtitles: false,
        }
    }

    #[test]
    fn with_the_app_engine_only_the_profile_is_its_capabilities() {
        let app = caps(
            &[Container::MKV, Container::MP4],
            vec![CodecSupport::any(Codec::HEVC)],
            &[Codec::TRUEHD],
            &[SubtitleFormat::ASS],
        );
        let engines = Engines::app_only(app.clone());
        assert_eq!(engines.device_profile(), app);
        // A remux without a native engine adds nothing.
        let with_remux = Engines {
            remux: Some(DeviceRemux {
                containers: vec![Container::AVI],
            }),
            ..engines
        };
        assert_eq!(with_remux.device_profile(), app);
    }

    #[test]
    fn the_profile_is_the_union_with_the_looser_limits() {
        let native_h264 = CodecSupport {
            max_bit_depth: Some(8),
            profiles: Some(vec!["high".into()]),
            level: Some(51),
            ranges: Some(vec![VideoRangeType::SDR]),
            ..CodecSupport::any(Codec::H264)
        };
        let app_h264 = CodecSupport {
            max_bit_depth: Some(10),
            profiles: Some(vec!["high 10".into()]),
            level: Some(52),
            ranges: None,
            ..CodecSupport::any(Codec::H264)
        };
        let mut native = caps(
            &[Container::MP4, Container::TS],
            vec![native_h264, CodecSupport::any(Codec::AV1)],
            &[Codec::AAC, Codec::EAC3],
            &[SubtitleFormat::SUBRIP, SubtitleFormat::PGSSUB],
        );
        // Opus in stereo only natively; any channels on the app engine.
        native.audio.push(CodecSupport {
            max_channels: Some(2),
            ..CodecSupport::any(Codec::OPUS)
        });
        let app = caps(
            &[Container::MKV, Container::MP4],
            vec![app_h264],
            &[Codec::AAC, Codec::TRUEHD, Codec::OPUS],
            &[SubtitleFormat::ASS, SubtitleFormat::PGSSUB],
        );
        let engines = Engines {
            native: Some(EngineProfile::new(native, true)),
            app: EngineProfile::new(app, true),
            remux: Some(DeviceRemux {
                containers: vec![Container::MKV, Container::AVI],
            }),
            rules: PlatformRules::WINDOWS,
        };
        let union = engines.device_profile();
        assert_eq!(
            union.containers,
            vec![
                Container::MKV,
                Container::MP4,
                Container::TS,
                Container::AVI
            ]
        );
        assert_eq!(
            union.video,
            vec![
                CodecSupport {
                    codec: Codec::H264,
                    max_bit_depth: Some(10),
                    profiles: Some(vec!["high 10".into(), "high".into()]),
                    level: Some(52),
                    ranges: None,
                    max_channels: None,
                },
                CodecSupport::any(Codec::AV1),
            ]
        );
        assert_eq!(
            union.audio,
            [Codec::AAC, Codec::TRUEHD, Codec::OPUS, Codec::EAC3].map(CodecSupport::any)
        );
        assert_eq!(
            union.subtitles,
            vec![
                SubtitleFormat::ASS,
                SubtitleFormat::PGSSUB,
                SubtitleFormat::SUBRIP
            ]
        );
    }
}
