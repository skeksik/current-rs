//! What a detail page says about an item's media: a thin summary of its default version (ADR 0018). Media sources and the full
//! stream types are ADR 0019's; they map to these.

use crate::playback::{MediaSource, MediaStream, StreamKind};

/// The default version of an item's media.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaSummary {
    /// How many versions the item has.
    pub source_count: u32,
    /// The container format, as the server names it.
    pub container: Option<String>,
    pub video: Option<StreamSummary>,
    pub audio: Vec<StreamSummary>,
    pub subtitles: Vec<StreamSummary>,
}

/// One stream of the default version, as a detail page lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamSummary {
    /// The server's display title for the stream.
    pub label: String,
    pub language: Option<String>,
    pub codec: Option<String>,
}

impl MediaSummary {
    /// The summary of an item's default version, `source`, for an item with `source_count` versions (ADR 0019: derived from the
    /// first media source, so the details screen needs no wire type of its own).
    pub fn from_source(source: &MediaSource, source_count: u32) -> Self {
        Self::from_streams(source_count, source.container.clone(), &source.streams)
    }

    /// The summary of one version's `streams`, in the server's order: its first video stream, and every audio and subtitle stream.
    pub fn from_streams(
        source_count: u32,
        container: Option<String>,
        streams: &[MediaStream],
    ) -> Self {
        let of = |kind: StreamKind| {
            streams
                .iter()
                .filter(move |stream| stream.kind == kind)
                .map(StreamSummary::from)
        };
        Self {
            source_count,
            container,
            video: of(StreamKind::VIDEO).next(),
            audio: of(StreamKind::AUDIO).collect(),
            subtitles: of(StreamKind::SUBTITLE).collect(),
        }
    }
}

impl From<&MediaStream> for StreamSummary {
    fn from(stream: &MediaStream) -> Self {
        Self {
            label: stream.display_title.clone().unwrap_or_default(),
            language: stream.language.clone(),
            codec: stream.codec.as_ref().map(|codec| codec.as_str().to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::Codec;

    fn stream(index: u32, kind: StreamKind, label: &str) -> MediaStream {
        MediaStream {
            index,
            kind,
            codec: Some(Codec::AAC),
            profile: None,
            level: None,
            language: Some("eng".into()),
            title: None,
            display_title: Some(label.into()),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
            is_external: false,
            bitrate: None,
            video: None,
            audio: None,
            subtitle_delivery: None,
        }
    }

    #[test]
    fn the_summary_comes_from_the_source() {
        let source = MediaSource {
            id: "s1".into(),
            name: None,
            container: Some("mkv".into()),
            size: None,
            bitrate: None,
            duration: None,
            streams: vec![
                stream(0, StreamKind::VIDEO, "1080p"),
                stream(1, StreamKind::AUDIO, "English"),
                stream(2, StreamKind::AUDIO, "Commentary"),
                stream(3, StreamKind::SUBTITLE, "English - SRT"),
                stream(4, StreamKind::new("SomeFutureKind"), "other"),
            ],
            default_audio: Some(1),
            default_subtitle: None,
            has_segments: false,
            is_remote: false,
        };
        let summary = MediaSummary::from_source(&source, 2);
        assert_eq!(summary.source_count, 2);
        assert_eq!(summary.container.as_deref(), Some("mkv"));
        assert_eq!(summary.video.map(|v| v.label), Some("1080p".to_owned()));
        assert_eq!(summary.audio.len(), 2);
        assert_eq!(summary.subtitles[0].label, "English - SRT");
        assert_eq!(summary.audio[0].codec.as_deref(), Some("aac"));
        assert_eq!(summary.audio[0].language.as_deref(), Some("eng"));
    }
}
