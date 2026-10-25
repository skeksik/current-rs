//! What a playback engine can play (ADR 0019, "Engine capabilities and the device profile"). The engine reports it (for libmpv, read
//! from its decoder and demuxer lists at start), and a service crate turns it into what its server needs to choose a route (for
//! Jellyfin, the device profile). It lives here rather than in `current-playback` so that a service crate can take it without
//! depending on the playback logic.
//!
//! Not to be confused with [`Capabilities`](crate::Capabilities), which is what the user's servers offer.

use crate::open::open_value;
use crate::playback::Codec;

open_value! {
    /// A container format, by Current's name. Open: an engine passes containers it knows under its own name, and a service crate
    /// translates the known ones to its server's spelling.
    Container {
        /// Matroska (`.mkv`).
        MATROSKA = "matroska",
        WEBM = "webm",
        /// The MP4 / QuickTime family (`.mp4`, `.m4v`, `.mov`).
        MP4 = "mp4",
        /// MPEG transport stream (`.ts`, `.m2ts`).
        MPEG_TS = "mpegts",
        AVI = "avi",
        OGG = "ogg",
        FLAC = "flac",
    }
}

/// A video codec the engine decodes, with its limits where it has them. No limit means anything the codec allows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VideoCodecSupport {
    pub codec: Codec,
    pub max_bit_depth: Option<u32>,
    /// The highest level, as the server counts it (for example 51 for H.264 level 5.1, 153 for HEVC level 5.1).
    pub max_level: Option<u32>,
    /// The profiles it decodes (for example `main`, `main 10`); empty for all.
    pub profiles: Vec<String>,
}

impl VideoCodecSupport {
    /// The codec without limits.
    pub fn any(codec: Codec) -> Self {
        Self {
            codec,
            max_bit_depth: None,
            max_level: None,
            profiles: Vec::new(),
        }
    }

    pub fn has_limits(&self) -> bool {
        self.max_bit_depth.is_some() || self.max_level.is_some() || !self.profiles.is_empty()
    }
}

/// What an engine can play. No dynamic range is excluded: an engine that cannot show a range tone-maps it (ADR 0019, choice 1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EngineCapabilities {
    /// Containers it demuxes.
    pub containers: Vec<Container>,
    pub video_codecs: Vec<VideoCodecSupport>,
    pub audio_codecs: Vec<Codec>,
    /// Subtitle formats it renders.
    pub subtitle_formats: Vec<Codec>,
    /// Whether it can take a subtitle as a separate file next to the stream.
    pub external_subtitles: bool,
}

impl EngineCapabilities {
    pub fn decodes_video(&self, codec: &Codec) -> bool {
        self.video_codecs
            .iter()
            .any(|support| &support.codec == codec)
    }

    pub fn decodes_audio(&self, codec: &Codec) -> bool {
        self.audio_codecs.contains(codec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_checks_the_lists() {
        let capabilities = EngineCapabilities {
            containers: vec![Container::MATROSKA],
            video_codecs: vec![VideoCodecSupport::any(Codec::HEVC)],
            audio_codecs: vec![Codec::AAC],
            subtitle_formats: vec![Codec::ASS],
            external_subtitles: true,
        };
        assert!(capabilities.decodes_video(&Codec::HEVC));
        assert!(!capabilities.decodes_video(&Codec::AAC));
        assert!(capabilities.decodes_audio(&Codec::AAC));
        assert!(!VideoCodecSupport::any(Codec::HEVC).has_limits());
    }
}
