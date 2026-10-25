//! What the linked libmpv can play, read once after `mpv_initialize` from `decoder-list` and `demuxer-lavf-list` (ADR 0019
//! "Capabilities"), so a different build reports what it really has.

use std::collections::BTreeSet;

use current_model::{Codec, CodecSupport, Container, EngineCapabilities, SubtitleFormat};

use crate::node::Node;

/// Video codecs, by mpv's `decoder-list` name (FFmpeg's codec name), and Jellyfin's name for each. They are the same today; the table
/// keeps the two vocabularies apart.
const VIDEO: [(&str, Codec); 7] = [
    ("h264", Codec::H264),
    ("hevc", Codec::HEVC),
    ("av1", Codec::AV1),
    ("vp9", Codec::VP9),
    ("vp8", Codec::VP8),
    ("mpeg2video", Codec::MPEG2),
    ("vc1", Codec::VC1),
];

/// Audio codecs, the same way.
const AUDIO: [(&str, Codec); 12] = [
    ("aac", Codec::AAC),
    ("ac3", Codec::AC3),
    ("eac3", Codec::EAC3),
    ("truehd", Codec::TRUEHD),
    ("dts", Codec::DTS),
    ("flac", Codec::FLAC),
    ("opus", Codec::OPUS),
    ("vorbis", Codec::VORBIS),
    ("mp3", Codec::MP3),
    ("alac", Codec::ALAC),
    ("pcm_s16le", Codec::PCM_S16LE),
    ("pcm_s24le", Codec::PCM_S24LE),
];

/// Containers, by one FFmpeg demuxer name (`demuxer-lavf-list` names a demuxer by all its formats, `matroska,webm`), and Jellyfin's
/// name, which renames `matroska` to `mkv` and `mpegts` to `ts` (`ProbeResultNormalizer.NormalizeFormat`, see `Container`).
const CONTAINERS: [(&str, Container); 8] = [
    ("matroska", Container::MKV),
    ("webm", Container::WEBM),
    ("mp4", Container::MP4),
    ("mov", Container::MOV),
    ("mpegts", Container::TS),
    ("avi", Container::AVI),
    ("ogg", Container::OGG),
    ("flac", Container::FLAC),
];

/// Subtitle formats the engine renders. mpv lists no subtitle decoders (`decoder-list` is filled from `vd_lavc.c` and `ad_lavc.c`), so
/// this is the list our Windows build is checked for: `check.py`'s `REQUIRED_SUBTITLE_DECODERS` (ass, subrip, webvtt,
/// hdmv_pgs_subtitle, dvd_subtitle, dvb_subtitle, mov_text), looked up in libavcodec. Both spellings where Jellyfin has two (embedded
/// `subrip`, external `srt`; `webvtt`, `vtt`); `ssa` is rendered by the same `ass` decoder. A system libmpv elsewhere may lack one (not
/// verified).
const SUBTITLES: [SubtitleFormat; 10] = [
    SubtitleFormat::ASS,
    SubtitleFormat::SSA,
    SubtitleFormat::SUBRIP,
    SubtitleFormat::SRT,
    SubtitleFormat::WEBVTT,
    SubtitleFormat::VTT,
    SubtitleFormat::MOV_TEXT,
    SubtitleFormat::PGSSUB,
    SubtitleFormat::DVDSUB,
    SubtitleFormat::DVBSUB,
];

/// mpv's own names: `decoder-list`'s `codec` entries (audio and video decoders; mpv fills it from `vd_lavc.c` and `ad_lavc.c`, so no
/// subtitle decoders) and `demuxer-lavf-list`'s FFmpeg demuxer names (`matroska,webm`, `mov,mp4,m4a,3gp,3g2,mj2`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildCapabilities {
    pub decoders: BTreeSet<String>,
    pub demuxers: BTreeSet<String>,
}

impl BuildCapabilities {
    /// From the two property values. Entries of an unexpected shape are skipped.
    pub(crate) fn from_nodes(decoder_list: &Node, demuxer_list: &Node) -> BuildCapabilities {
        let decoders = decoder_list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.get("codec").and_then(Node::as_str))
            .map(str::to_owned)
            .collect();
        let demuxers = demuxer_list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Node::as_str)
            .map(str::to_owned)
            .collect();
        BuildCapabilities { decoders, demuxers }
    }

    pub fn has_decoder(&self, codec: &str) -> bool {
        self.decoders.contains(codec)
    }

    pub fn has_demuxer(&self, name: &str) -> bool {
        self.demuxers.contains(name)
    }

    /// Whether some demuxer handles `format` (one of the comma-separated names a demuxer lists).
    fn demuxes(&self, format: &str) -> bool {
        self.demuxers
            .iter()
            .any(|d| d.split(',').any(|f| f == format))
    }

    /// What the engine can play, in Jellyfin's names (ADR 0019, "Engine capabilities and the device profile"). Only the codecs and
    /// containers in the tables above are listed, and only when this build has them; no limits, since mpv tone-maps every range
    /// (choice 1). External subtitles: yes, the core fetches them and hands them over in memory (ADR 0019, "External subtitles").
    pub fn engine_capabilities(&self) -> EngineCapabilities {
        let codecs = |table: &[(&str, Codec)]| {
            table
                .iter()
                .filter(|(name, _)| self.has_decoder(name))
                .map(|(_, codec)| CodecSupport::any(codec.clone()))
                .collect()
        };
        EngineCapabilities {
            containers: CONTAINERS
                .iter()
                .filter(|(name, _)| self.demuxes(name))
                .map(|(_, container)| container.clone())
                .collect(),
            video: codecs(&VIDEO),
            audio: codecs(&AUDIO),
            subtitles: SUBTITLES.to_vec(),
            external_subtitles: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder(codec: &str, driver: &str) -> Node {
        Node::Map(vec![
            ("codec".into(), Node::String(codec.into())),
            ("driver".into(), Node::String(driver.into())),
            ("description".into(), Node::String(String::new())),
        ])
    }

    #[test]
    fn reads_codecs_and_demuxers() {
        let decoders = Node::Array(vec![
            decoder("h264", "h264"),
            decoder("hevc", "hevc"),
            decoder("av1", "libdav1d"),
            decoder("av1", "av1"),
            Node::String("junk".into()),
        ]);
        let demuxers = Node::Array(vec![
            Node::String("matroska,webm".into()),
            Node::String("hls".into()),
            Node::Int(1),
        ]);
        let caps = BuildCapabilities::from_nodes(&decoders, &demuxers);
        assert_eq!(
            caps.decoders.iter().map(String::as_str).collect::<Vec<_>>(),
            ["av1", "h264", "hevc"]
        );
        assert!(caps.has_demuxer("matroska,webm"));
        assert!(caps.has_demuxer("hls"));
        assert!(!caps.has_decoder("vc1"));
        assert_eq!(
            BuildCapabilities::from_nodes(&Node::None, &Node::None),
            BuildCapabilities::default()
        );
    }

    #[test]
    fn maps_onto_jellyfin_names() {
        let decoders = Node::Array(
            ["h264", "hevc", "dts", "pcm_s16le", "cinepak"]
                .iter()
                .map(|c| decoder(c, c))
                .collect(),
        );
        let demuxers = Node::Array(
            ["matroska,webm", "mov,mp4,m4a,3gp,3g2,mj2", "mpegts", "srt"]
                .iter()
                .map(|d| Node::String((*d).into()))
                .collect(),
        );
        let caps = BuildCapabilities::from_nodes(&decoders, &demuxers).engine_capabilities();
        assert_eq!(
            caps.containers,
            vec![
                Container::MKV,
                Container::WEBM,
                Container::MP4,
                Container::MOV,
                Container::TS
            ]
        );
        assert_eq!(
            caps.video,
            vec![
                CodecSupport::any(Codec::H264),
                CodecSupport::any(Codec::HEVC)
            ]
        );
        assert_eq!(
            caps.audio,
            vec![
                CodecSupport::any(Codec::DTS),
                CodecSupport::any(Codec::PCM_S16LE)
            ]
        );
        for both in [
            SubtitleFormat::SUBRIP,
            SubtitleFormat::SRT,
            SubtitleFormat::WEBVTT,
            SubtitleFormat::VTT,
        ] {
            assert!(caps.subtitles.contains(&both), "{both:?}");
        }
        assert!(caps.external_subtitles);
        assert!(
            caps.video
                .iter()
                .chain(&caps.audio)
                .all(|c| !c.has_limits())
        );
    }
}
