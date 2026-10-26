//! Playback: what a server says about an item's media (sources and their streams), how it will be played (the route and why), the
//! user's track choices, media segments, the server's transcoding facts and the user's playback preferences (ADR 0019).
//!
//! No URL, path or token is in any of these types: paths stay in the service crate, and stream URLs exist only inside the engine
//! handoff (ADR 0019, "Media sources and streams").

use std::time::Duration;

use crate::identity::ItemRef;
use crate::open::open_value;

open_value! {
    /// What a stream inside a media source carries. Open: each service crate maps its server's stream types to these and passes any
    /// other type through.
    StreamKind {
        VIDEO = "video",
        AUDIO = "audio",
        SUBTITLE = "subtitle",
        /// Cover art stored as a stream.
        EMBEDDED_IMAGE = "embedded_image",
        DATA = "data",
        LYRIC = "lyric",
    }
}

open_value! {
    /// A codec, by the server's name (for Jellyfin, FFmpeg's codec name as its probe reports it; subtitle codecs it renames, for
    /// example `PGSSUB`). Open: any codec passes through unchanged. Subtitle formats an engine renders are
    /// [`SubtitleFormat`](crate::SubtitleFormat)s.
    Codec {
        H264 = "h264",
        HEVC = "hevc",
        AV1 = "av1",
        VP9 = "vp9",
        VP8 = "vp8",
        MPEG2 = "mpeg2video",
        VC1 = "vc1",
        AAC = "aac",
        AC3 = "ac3",
        EAC3 = "eac3",
        TRUEHD = "truehd",
        DTS = "dts",
        FLAC = "flac",
        OPUS = "opus",
        VORBIS = "vorbis",
        MP3 = "mp3",
        ALAC = "alac",
        PCM_S16LE = "pcm_s16le",
        PCM_S24LE = "pcm_s24le",
    }
}

open_value! {
    /// A video's dynamic range, coarse, by Jellyfin's name (`VideoRange`). Open.
    VideoRange {
        SDR = "SDR",
        HDR = "HDR",
    }
}

open_value! {
    /// A video's dynamic range format in detail, by Jellyfin's name (`VideoRangeType`): which HDR or Dolby Vision signalling it
    /// carries. Open.
    VideoRangeType {
        SDR = "SDR",
        HDR10 = "HDR10",
        HDR10_PLUS = "HDR10Plus",
        HLG = "HLG",
        DOVI = "DOVI",
        DOVI_WITH_HDR10 = "DOVIWithHDR10",
        DOVI_WITH_HLG = "DOVIWithHLG",
        DOVI_WITH_SDR = "DOVIWithSDR",
        DOVI_WITH_EL = "DOVIWithEL",
        DOVI_WITH_HDR10_PLUS = "DOVIWithHDR10Plus",
        DOVI_WITH_EL_HDR10_PLUS = "DOVIWithELHDR10Plus",
        DOVI_INVALID = "DOVIInvalid",
    }
}

open_value! {
    /// How a server would deliver a subtitle stream to this client. Open.
    SubtitleDelivery {
        /// Burned into the video by a transcode.
        ENCODE = "encode",
        /// Inside the file or stream the engine plays.
        EMBED = "embed",
        /// A separate file the core fetches (ADR 0019, "External subtitles").
        EXTERNAL = "external",
        /// A subtitle playlist inside an HLS manifest.
        HLS = "hls",
        /// Not delivered.
        DROP = "drop",
    }
}

/// One version of an item as a server describes it: its container, size, bitrate and streams, and which audio and subtitle stream
/// the server picked for the user. `id` is the server's id for the version; no path or URL.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSource {
    pub id: String,
    pub name: Option<String>,
    /// The server's container name (for example `mkv`, or a list such as `mov,mp4,m4a`), for display.
    pub container: Option<String>,
    /// Bytes.
    pub size: Option<u64>,
    /// Bits per second, all streams together.
    pub bitrate: Option<u32>,
    pub duration: Option<Duration>,
    pub streams: Vec<MediaStream>,
    /// The audio stream the server picked for the user (their preferences or a remembered choice), by server index.
    pub default_audio: Option<u32>,
    /// The subtitle stream the server picked for the user, by server index; `None` for none.
    pub default_subtitle: Option<u32>,
    /// Whether the server has media segments (intro, outro, ...) for the item.
    pub has_segments: bool,
    /// Whether the media lives somewhere other than the server (the server proxies it).
    pub is_remote: bool,
}

impl MediaSource {
    /// The streams of one kind, in the server's order.
    pub fn streams_of<'a>(
        &'a self,
        kind: &'a StreamKind,
    ) -> impl Iterator<Item = &'a MediaStream> + 'a {
        self.streams
            .iter()
            .filter(move |stream| &stream.kind == kind)
    }

    /// The stream with the server's index `index`.
    pub fn stream(&self, index: u32) -> Option<&MediaStream> {
        self.streams.iter().find(|stream| stream.index == index)
    }

    /// The first video stream.
    pub fn video(&self) -> Option<&MediaStream> {
        self.streams_of(&StreamKind::VIDEO).next()
    }
}

/// One stream of a media source. `index` is the server's stream index, the one the server and progress reports use.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaStream {
    pub index: u32,
    pub kind: StreamKind,
    pub codec: Option<Codec>,
    pub profile: Option<String>,
    pub level: Option<f64>,
    /// The language as the server has it (usually ISO 639-2, for example `eng`).
    pub language: Option<String>,
    pub title: Option<String>,
    /// The server's label for the stream, for track menus.
    pub display_title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    /// A separate file next to the media (an external subtitle or audio track).
    pub is_external: bool,
    pub video: Option<VideoFacts>,
    pub audio: Option<AudioFacts>,
    /// How the server would deliver this subtitle stream; `None` for other kinds.
    pub subtitle_delivery: Option<SubtitleDelivery>,
}

/// What a video stream is.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFacts {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    /// Frames per second.
    pub frame_rate: Option<f32>,
    pub range: Option<VideoRange>,
    pub range_type: Option<VideoRangeType>,
    /// The Dolby Vision profile, when the stream has one.
    pub dv_profile: Option<u32>,
}

/// What an audio stream is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFacts {
    pub channels: Option<u32>,
    /// For example `5.1(side)`.
    pub layout: Option<String>,
    /// Hz.
    pub sample_rate: Option<u32>,
}

/// What happens to one stream on a server stream: copied as it is, or converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamAction {
    Copy,
    Transcode,
}

/// How a server stream is delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamProtocol {
    /// An HLS playlist with segments.
    Hls,
    /// One progressive HTTP response.
    Progressive,
}

/// How a media source reaches the engine. Closed: Current defines the routes (ADR 0019, "Route planner").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The engine reads the file as it is.
    DirectPlay,
    /// The server makes a stream for the engine. With both streams copied it is a remux; with only the video copied, what the UI
    /// calls "direct stream"; otherwise a transcode.
    ServerStream {
        protocol: StreamProtocol,
        /// The stream's container (for example `mp4` for fMP4 segments, `ts`).
        container: Option<String>,
        video: StreamAction,
        audio: StreamAction,
    },
}

/// What the UI calls a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouteLabel {
    DirectPlay,
    Remux,
    DirectStream,
    Transcode,
}

impl Route {
    pub fn label(&self) -> RouteLabel {
        match self {
            Self::DirectPlay => RouteLabel::DirectPlay,
            Self::ServerStream { video, audio, .. } => match (video, audio) {
                (StreamAction::Copy, StreamAction::Copy) => RouteLabel::Remux,
                (StreamAction::Copy, StreamAction::Transcode) => RouteLabel::DirectStream,
                (StreamAction::Transcode, _) => RouteLabel::Transcode,
            },
        }
    }

    pub fn is_direct_play(&self) -> bool {
        *self == Self::DirectPlay
    }
}

open_value! {
    /// Why a route is not direct play: the server's reasons, mapped by the service crate (unknown ones pass through under the
    /// server's name), plus Current's own.
    RouteReason {
        CONTAINER_NOT_SUPPORTED = "container_not_supported",
        VIDEO_CODEC_NOT_SUPPORTED = "video_codec_not_supported",
        AUDIO_CODEC_NOT_SUPPORTED = "audio_codec_not_supported",
        SUBTITLE_CODEC_NOT_SUPPORTED = "subtitle_codec_not_supported",
        AUDIO_IS_EXTERNAL = "audio_is_external",
        SECONDARY_AUDIO_NOT_SUPPORTED = "secondary_audio_not_supported",
        STREAM_COUNT_EXCEEDS_LIMIT = "stream_count_exceeds_limit",
        VIDEO_PROFILE_NOT_SUPPORTED = "video_profile_not_supported",
        VIDEO_RANGE_TYPE_NOT_SUPPORTED = "video_range_type_not_supported",
        VIDEO_CODEC_TAG_NOT_SUPPORTED = "video_codec_tag_not_supported",
        VIDEO_LEVEL_NOT_SUPPORTED = "video_level_not_supported",
        VIDEO_RESOLUTION_NOT_SUPPORTED = "video_resolution_not_supported",
        VIDEO_BIT_DEPTH_NOT_SUPPORTED = "video_bit_depth_not_supported",
        VIDEO_FRAMERATE_NOT_SUPPORTED = "video_framerate_not_supported",
        VIDEO_ROTATION_NOT_SUPPORTED = "video_rotation_not_supported",
        REF_FRAMES_NOT_SUPPORTED = "ref_frames_not_supported",
        ANAMORPHIC_VIDEO_NOT_SUPPORTED = "anamorphic_video_not_supported",
        INTERLACED_VIDEO_NOT_SUPPORTED = "interlaced_video_not_supported",
        AUDIO_CHANNELS_NOT_SUPPORTED = "audio_channels_not_supported",
        AUDIO_PROFILE_NOT_SUPPORTED = "audio_profile_not_supported",
        AUDIO_SAMPLE_RATE_NOT_SUPPORTED = "audio_sample_rate_not_supported",
        AUDIO_BIT_DEPTH_NOT_SUPPORTED = "audio_bit_depth_not_supported",
        CONTAINER_BITRATE_EXCEEDS_LIMIT = "container_bitrate_exceeds_limit",
        VIDEO_BITRATE_NOT_SUPPORTED = "video_bitrate_not_supported",
        AUDIO_BITRATE_NOT_SUPPORTED = "audio_bitrate_not_supported",
        UNKNOWN_VIDEO_STREAM_INFO = "unknown_video_stream_info",
        UNKNOWN_AUDIO_STREAM_INFO = "unknown_audio_stream_info",
        DIRECT_PLAY_ERROR = "direct_play_error",
        /// Current's: the user's quality choice is below the source's bitrate.
        QUALITY_LIMIT = "quality_limit",
        /// Current's: the engine could not play the file directly, so the plan was made again without direct play.
        FALLBACK_AFTER_ENGINE_ERROR = "fallback_after_engine_error",
        /// Current's: the user asked for a transcode.
        USER_CHOSE_TRANSCODE = "user_chose_transcode",
    }
}

/// The subtitle the user wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SubtitleChoice {
    /// Whatever the server picks for the user.
    #[default]
    Default,
    /// No subtitle.
    Off,
    /// The stream with this server index.
    Stream(u32),
}

/// The tracks the user wants, by server stream index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TrackChoice {
    /// `None`: whatever the server picks for the user.
    pub audio: Option<u32>,
    pub subtitle: SubtitleChoice,
}

/// How an item will be played: the source, the route and why, where it starts and with which tracks. No URL: the stream's address
/// stays with the engine handoff.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackPlan {
    pub item: ItemRef,
    pub source: MediaSource,
    /// The server's id for this playback, reported with every progress report.
    pub play_session_id: String,
    pub route: Route,
    /// Empty for direct play.
    pub reasons: Vec<RouteReason>,
    pub start: Duration,
    pub tracks: TrackChoice,
}

open_value! {
    /// What a media segment is. Open: unknown kinds pass through under the server's name.
    SegmentKind {
        INTRO = "intro",
        OUTRO = "outro",
        RECAP = "recap",
        PREVIEW = "preview",
        COMMERCIAL = "commercial",
    }
}

/// A stretch of an item with a meaning (an intro, the credits, ...), from the server's segment providers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: Duration,
    pub end: Duration,
}

impl Segment {
    /// Whether `position` lies inside the segment (start included, end excluded).
    pub fn contains(&self, position: Duration) -> bool {
        self.start <= position && position < self.end
    }
}

open_value! {
    /// Why the engine failed. Open: the engine passes its own kinds through.
    EngineErrorKind {
        /// The file or stream could not be opened or read.
        LOADING_FAILED = "loading_failed",
        /// The engine does not support the format.
        UNSUPPORTED_FORMAT = "unsupported_format",
        /// A decoder failed.
        DECODER_FAILED = "decoder_failed",
        /// An HLS or DASH stream that needs gateway headers reached the engine without the core's same-origin check of its playlists
        /// (ADR 0019, "Playlists and headers"): refused rather than sending the headers to whatever host a playlist names.
        GATEWAY_PLAYLIST_UNCHECKED = "gateway_playlist_unchecked",
        /// The engine stopped working (libmpv quit on its own): the player must be opened again.
        ENGINE_GONE = "engine_gone",
    }
}

/// Why playback cannot start or go on. Flat, with no server text and no URL (ADR 0019, "Errors").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackProblem {
    /// The server offers neither direct play nor a stream for the source.
    NotPlayable {
        reasons: Vec<RouteReason>,
    },
    Engine {
        kind: EngineErrorKind,
    },
    Network,
    /// A gateway in front of the server wants the user to sign in to it again (ADR 0017).
    GatewaySignInNeeded,
    /// The server ended the session: sign in again.
    SignedOut,
    Server {
        status: u16,
    },
    NotFound,
    /// The server's HLS playlist, fetched through the loopback relay, names a URI off the server's address or outside the item's
    /// streams, or could not be read, so the relay refused it (ADR 0020, "HLS playlists"). Holds no address.
    PlaylistElsewhere,
    /// Current could not start its local stream connection, the loopback relay on 127.0.0.1 that streams on an address with a
    /// gateway go through (ADR 0020). No fallback hands the player the gateway's credentials.
    LocalRelay,
}

/// What the server reports about its transcode of this playback (a session's transcoding information). Before start the route is
/// predicted from the reasons; these facts correct it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscodingFacts {
    pub video_direct: bool,
    pub audio_direct: bool,
    pub video_codec: Option<Codec>,
    pub audio_codec: Option<Codec>,
    pub container: Option<String>,
    /// Bits per second.
    pub bitrate: Option<u32>,
    pub frame_rate: Option<f32>,
    /// Percent of the item transcoded so far.
    pub completion: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub audio_channels: Option<u32>,
    /// The server's name for its hardware acceleration (for example `nvenc`), or `None` for software.
    pub hardware_acceleration: Option<String>,
    pub reasons: Vec<RouteReason>,
}

open_value! {
    /// When the server shows subtitles for the user. Open.
    SubtitleMode {
        /// The server's default rules (forced or default subtitles, by language).
        DEFAULT = "default",
        ALWAYS = "always",
        ONLY_FORCED = "only_forced",
        NONE = "none",
        /// Only when the audio is in another language than the user's.
        SMART = "smart",
    }
}

/// The user's playback preferences as the server keeps them. The server applies the language and remembering preferences itself
/// (the source's default audio and subtitle); Current reads them to explain and for next-episode autoplay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackPreferences {
    pub audio_language: Option<String>,
    pub play_default_audio_track: bool,
    pub subtitle_language: Option<String>,
    pub subtitle_mode: SubtitleMode,
    pub next_episode_autoplay: bool,
    pub remember_audio: bool,
    pub remember_subtitles: bool,
}

/// Where a player is (ADR 0019, "Playback controller"). Position, duration, tracks, route and the next episode ride along in the
/// player's state, not here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlayerState {
    /// Nothing loaded.
    #[default]
    Idle,
    /// Asking the server how to play (planning).
    Preparing,
    /// The engine opens the stream.
    Loading,
    Playing,
    Paused,
    /// Waiting for data.
    Buffering,
    Seeking,
    /// The file played to its end.
    Ended,
    Failed {
        problem: PlaybackProblem,
    },
}

impl PlayerState {
    /// Whether a file is open in the engine and playback goes on (playing, paused, buffering or seeking).
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Playing | Self::Paused | Self::Buffering | Self::Seeking
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(index: u32, kind: StreamKind) -> MediaStream {
        MediaStream {
            index,
            kind,
            codec: None,
            profile: None,
            level: None,
            language: None,
            title: None,
            display_title: None,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
            is_external: false,
            video: None,
            audio: None,
            subtitle_delivery: None,
        }
    }

    #[test]
    fn route_labels_follow_what_is_copied() {
        let stream = |video, audio| Route::ServerStream {
            protocol: StreamProtocol::Hls,
            container: Some("mp4".into()),
            video,
            audio,
        };
        assert_eq!(Route::DirectPlay.label(), RouteLabel::DirectPlay);
        assert_eq!(
            stream(StreamAction::Copy, StreamAction::Copy).label(),
            RouteLabel::Remux
        );
        assert_eq!(
            stream(StreamAction::Copy, StreamAction::Transcode).label(),
            RouteLabel::DirectStream
        );
        assert_eq!(
            stream(StreamAction::Transcode, StreamAction::Copy).label(),
            RouteLabel::Transcode
        );
    }

    #[test]
    fn streams_are_found_by_server_index_and_kind() {
        let source = MediaSource {
            id: "s1".into(),
            name: None,
            container: Some("mkv".into()),
            size: None,
            bitrate: None,
            duration: None,
            streams: vec![
                stream(0, StreamKind::VIDEO),
                stream(1, StreamKind::AUDIO),
                stream(3, StreamKind::new("SomeFutureKind")),
                stream(2, StreamKind::SUBTITLE),
            ],
            default_audio: Some(1),
            default_subtitle: None,
            has_segments: false,
            is_remote: false,
        };
        assert_eq!(source.video().map(|s| s.index), Some(0));
        assert_eq!(
            source.stream(2).map(|s| s.kind.clone()),
            Some(StreamKind::SUBTITLE)
        );
        assert_eq!(source.streams_of(&StreamKind::AUDIO).count(), 1);
        assert!(source.stream(9).is_none());
    }

    #[test]
    fn active_states_are_the_ones_with_a_file_going() {
        assert!(PlayerState::Playing.is_active());
        assert!(PlayerState::Seeking.is_active());
        assert!(!PlayerState::Preparing.is_active());
        assert!(
            !PlayerState::Failed {
                problem: PlaybackProblem::Network
            }
            .is_active()
        );
        assert_eq!(PlayerState::default(), PlayerState::Idle);
    }

    #[test]
    fn a_segment_holds_its_start_but_not_its_end() {
        let segment = Segment {
            kind: SegmentKind::INTRO,
            start: Duration::from_secs(10),
            end: Duration::from_secs(70),
        };
        assert!(!segment.contains(Duration::from_secs(9)));
        assert!(segment.contains(Duration::from_secs(10)));
        assert!(segment.contains(Duration::from_secs(69)));
        assert!(!segment.contains(Duration::from_secs(70)));
    }
}
