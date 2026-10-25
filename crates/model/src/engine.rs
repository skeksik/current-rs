//! What a playback engine can play (ADR 0019, "Engine capabilities and the device profile"). The engine reports it (for libmpv, read
//! from its decoder and demuxer lists at start), and a service crate turns it into what its server needs to choose a route (for
//! Jellyfin, the device profile). It lives here rather than in `current-playback` so that a service crate can take it without
//! depending on the playback logic.
//!
//! The names are Jellyfin's (what its probe reports and its device profile takes), the one server vocabulary playback has today;
//! an engine maps its own names onto them, and unknown names pass through.
//!
//! Not to be confused with [`Capabilities`](crate::Capabilities), which is what the user's servers offer.

use crate::open::open_value;
use crate::playback::{Codec, VideoRangeType};

open_value! {
    /// A container format, by Jellyfin's name (`ProbeResultNormalizer.NormalizeFormat` renames FFmpeg's `matroska` to `mkv` and
    /// `mpegts` to `ts`). Open.
    Container {
        MKV = "mkv",
        WEBM = "webm",
        MP4 = "mp4",
        MOV = "mov",
        TS = "ts",
        AVI = "avi",
        OGG = "ogg",
        FLAC = "flac",
    }
}

open_value! {
    /// A subtitle format, by Jellyfin's name. Jellyfin reports embedded SubRip as `subrip` and external files as `srt` (and WebVTT as
    /// `webvtt` and `vtt`), so an engine that renders a format lists both spellings. Compared without case by the server. Open.
    SubtitleFormat {
        ASS = "ass",
        SSA = "ssa",
        SUBRIP = "subrip",
        SRT = "srt",
        WEBVTT = "webvtt",
        VTT = "vtt",
        MOV_TEXT = "mov_text",
        PGSSUB = "pgssub",
        DVDSUB = "dvdsub",
        DVBSUB = "dvbsub",
    }
}

/// A codec the engine decodes, with its limits where it has them. `None` means no limit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodecSupport {
    pub codec: Codec,
    pub max_bit_depth: Option<u8>,
    /// The profiles it decodes, by the server's names (for example `main`, `main 10`).
    pub profiles: Option<Vec<String>>,
    /// The highest level, as the server counts it (for example 51 for H.264 level 5.1, 153 for HEVC level 5.1). Video only.
    pub level: Option<u32>,
    /// The dynamic range types it shows. Video only. `None` for all: an engine that cannot show a range tone-maps it (ADR 0019,
    /// choice 1).
    pub ranges: Option<Vec<VideoRangeType>>,
}

impl CodecSupport {
    /// The codec without limits.
    pub fn any(codec: Codec) -> Self {
        Self {
            codec,
            max_bit_depth: None,
            profiles: None,
            level: None,
            ranges: None,
        }
    }

    pub fn has_limits(&self) -> bool {
        self.max_bit_depth.is_some()
            || self.profiles.is_some()
            || self.level.is_some()
            || self.ranges.is_some()
    }
}

/// What an engine can play.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EngineCapabilities {
    /// Containers it demuxes.
    pub containers: Vec<Container>,
    /// Video codecs it decodes.
    pub video: Vec<CodecSupport>,
    /// Audio codecs it decodes.
    pub audio: Vec<CodecSupport>,
    /// Subtitle formats it renders.
    pub subtitles: Vec<SubtitleFormat>,
    /// Whether it can take a subtitle as a separate file next to the stream.
    pub external_subtitles: bool,
}

impl EngineCapabilities {
    pub fn decodes_video(&self, codec: &Codec) -> bool {
        self.video.iter().any(|support| &support.codec == codec)
    }

    pub fn decodes_audio(&self, codec: &Codec) -> bool {
        self.audio.iter().any(|support| &support.codec == codec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_checks_the_lists() {
        let capabilities = EngineCapabilities {
            containers: vec![Container::MKV],
            video: vec![CodecSupport::any(Codec::HEVC)],
            audio: vec![CodecSupport::any(Codec::AAC)],
            subtitles: vec![SubtitleFormat::ASS],
            external_subtitles: true,
        };
        assert!(capabilities.decodes_video(&Codec::HEVC));
        assert!(!capabilities.decodes_video(&Codec::AAC));
        assert!(capabilities.decodes_audio(&Codec::AAC));
        assert!(!CodecSupport::any(Codec::HEVC).has_limits());
        let limited = CodecSupport {
            ranges: Some(vec![VideoRangeType::SDR]),
            ..CodecSupport::any(Codec::HEVC)
        };
        assert!(limited.has_limits());
    }
}
