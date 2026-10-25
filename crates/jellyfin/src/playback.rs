//! Playback against Jellyfin 12.2 (ADR 0019): PlaybackInfo with the generated device profile, the route and its reasons read from
//! the answer, the stream URL for the engine, playstate reports, media segments, the session's transcoding information and the
//! user's playback preferences.
//!
//! Server behaviour relied on, from Jellyfin `v12.2` (commit ca0f16eb, read 2026-10-25; quotes in `docs/notes/playback.md`):
//! - The answer is either direct play (`SupportsDirectPlay`; the client builds the static stream URL) or a `TranscodingUrl`: direct
//!   stream is switched off for every client (`MediaInfoHelper.SetDeviceSpecificData`), so a remux is a transcode that copies.
//! - The reasons are only in the `TranscodingUrl` query (`TranscodeReasons=` with the flag names joined by `,`,
//!   `StreamInfo.ToUrl`); `MediaSourceInfo.TranscodeReasons` is `[JsonIgnore]`.
//! - `TranscodingUrl` is relative (`StreamInfo.ToUrl(null, token, ...)`: `/videos/{id}/master.m3u8?...` or
//!   `/videos/{id}/stream.{container}?...`) and carries `ApiKey=<the caller's token>`; the server appends
//!   `&allowVideoStreamCopy=false` / `&allowAudioStreamCopy=false` when copying is not allowed.
//! - The token is read from the `ApiKey` query value on every 12.2 server (`AuthorizationContext.cs`).

use std::time::Duration;

use current_model::{
    AudioFacts, Codec, EngineCapabilities, ItemId, MediaSource, MediaStream, PlaybackPreferences,
    Route, RouteReason, Segment, SegmentKind, StreamAction, StreamKind, StreamProtocol,
    SubtitleChoice, SubtitleDelivery, SubtitleMode, TrackChoice, TranscodingFacts, VideoFacts,
    VideoRange, VideoRangeType,
};
use current_net::{NetError, Request, Url};

use crate::auth;
use crate::client::{AuthError, JellyfinClient};
use crate::playback_wire as wire;
use crate::profile;

/// The "Maximum" quality (ADR 0019, choice 10): the largest `MaxStreamingBitrate` the server takes (`int?` in `PlaybackInfoDto`), so
/// the user's `RemoteClientBitrateLimit` is the only cap. A transcode's target stays at the source's own bitrate and at most 400
/// Mbit/s whatever the client asks (`EncodingHelper.GetVideoBitrateParamValue`: "Don't allow bitrate increases unless upscaling",
/// "Cap the max target bitrate to 400 Mbps"). The caller passes this unless the user picked a lower quality.
pub const MAXIMUM_STREAMING_BITRATE: u32 = i32::MAX as u32;

/// A stop report waits at most this long: it is best effort, also when the app exits (ADR 0019, "Progress reporting").
pub const STOP_REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// What a PlaybackInfo request asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackRequest {
    /// The version the user picked; `None` for the server's first.
    pub media_source: Option<String>,
    pub start: Duration,
    pub tracks: TrackChoice,
    /// Bits per second; always sent ([`MAXIMUM_STREAMING_BITRATE`] for "Maximum"). Never left to the server, whose default is 8
    /// Mbit/s.
    pub max_streaming_bitrate: u32,
    /// False only after a fallback (ADR 0019, choice 9).
    pub direct_play: bool,
}

/// The server's answer to a PlaybackInfo request.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackAnswer {
    /// The server's id for this playback, carried by every report.
    pub play_session_id: String,
    /// The sources in the server's order: the requested or the item's own first.
    pub sources: Vec<SourceAnswer>,
}

impl PlaybackAnswer {
    /// The source with `id`, else the first; with its stream, or [`PlaybackError::NotPlayable`] when the server offers neither
    /// direct play nor a stream for it.
    pub fn playable(
        &self,
        id: Option<&str>,
    ) -> Result<(&MediaSource, &PlayableStream), PlaybackError> {
        let source = match id {
            Some(id) => self.sources.iter().find(|answer| answer.source.id == id),
            None => self.sources.first(),
        };
        match source {
            Some(SourceAnswer {
                source,
                stream: Some(stream),
            }) => Ok((source, stream)),
            _ => Err(PlaybackError::NotPlayable {
                reasons: Vec::new(),
            }),
        }
    }
}

/// One source of the answer and how it can be played; `stream` is `None` when the server offers no route for it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceAnswer {
    pub source: MediaSource,
    pub stream: Option<PlayableStream>,
}

/// A route the server offers for a source. Before start, whether a server stream copies the video and audio is predicted from the
/// reasons and codecs in its URL; the session's [`TranscodingFacts`] tell after start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayableStream {
    pub route: Route,
    /// Empty for direct play.
    pub reasons: Vec<RouteReason>,
    pub url: StreamUrl,
}

/// The URL the engine plays, with the token in it as `ApiKey` (ADR 0019, choice 3). Its `Debug` shows nothing of it, it has no
/// `Display`, and it never goes through the request log: the client builds it without sending it.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamUrl(Url);

impl StreamUrl {
    /// The URL with its token. Only for the engine handoff (`current-playback` passing it to the engine, `current-mpv` to libmpv;
    /// ADR 0019 tasks 2 and 3): never logged, shown, stored or passed across `current-ffi`.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for StreamUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamUrl(<redacted>)")
    }
}

/// Why a PlaybackInfo request found nothing to play. Neither Display nor Debug holds a URL or a token.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlaybackError {
    /// The server offers neither direct play nor a stream (`NoCompatibleStream`, no source, or no route for the source).
    #[error("the server offers no way to play this item")]
    NotPlayable { reasons: Vec<RouteReason> },
    /// `NotAllowed`: the user's policy does not let them play it.
    #[error("the server does not let this user play the item")]
    NotAllowed,
    /// `RateLimitExceeded`: the server's streaming limit is reached.
    #[error("the server's streaming limit is reached")]
    RateLimited,
    #[error(transparent)]
    Auth(#[from] AuthError),
}

/// A playstate report for `Sessions/Playing` and `/Progress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressReport {
    pub item: ItemId,
    pub media_source: String,
    pub play_session_id: String,
    pub position: Duration,
    /// The audio stream playing, by server index.
    pub audio: Option<u32>,
    /// The subtitle shown, by server index; `None` for none (sent as `-1`, which the server remembers as the user's choice).
    pub subtitle: Option<u32>,
    pub paused: bool,
    pub muted: bool,
    /// 0 to 100.
    pub volume: Option<u8>,
    pub can_seek: bool,
    /// Gives the play method: `DirectPlay` or `Transcode`.
    pub route: Route,
}

/// A report for `Sessions/Playing/Stopped`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopReport {
    pub item: ItemId,
    pub media_source: String,
    pub play_session_id: String,
    pub position: Duration,
    /// Playback ended with an error.
    pub failed: bool,
}

impl JellyfinClient {
    /// Asks the server how to play `item` (`POST /Items/{item}/PlaybackInfo`) with the device profile for `capabilities`.
    pub async fn playback_info(
        &self,
        item: &ItemId,
        request: &PlaybackRequest,
        capabilities: &EngineCapabilities,
    ) -> Result<PlaybackAnswer, PlaybackError> {
        let bitrate = i32::try_from(request.max_streaming_bitrate).unwrap_or(i32::MAX);
        let profile = profile::device_profile(capabilities, bitrate);
        let body = wire::PlaybackInfoDto {
            max_streaming_bitrate: bitrate,
            start_time_ticks: Some(ticks(request.start)),
            audio_stream_index: request.tracks.audio.map(index_to_wire),
            subtitle_stream_index: match request.tracks.subtitle {
                SubtitleChoice::Default => None,
                SubtitleChoice::Off => Some(-1),
                SubtitleChoice::Stream(index) => Some(index_to_wire(index)),
            },
            media_source_id: request.media_source.as_deref(),
            device_profile: &profile,
            enable_direct_play: request.direct_play,
            enable_transcoding: true,
            allow_video_stream_copy: true,
            allow_audio_stream_copy: true,
        };
        let path = format!("/Items/{}/PlaybackInfo", item.as_str());
        let request = Request::post(path).json(&body).map_err(AuthError::from)?;
        let answer: wire::PlaybackInfoResponse = self
            .send_authenticated(request)
            .await?
            .json()
            .map_err(AuthError::from)?;
        match answer.error_code.as_deref() {
            None => {}
            Some("NotAllowed") => return Err(PlaybackError::NotAllowed),
            Some("RateLimitExceeded") => return Err(PlaybackError::RateLimited),
            Some(_) => {
                return Err(PlaybackError::NotPlayable {
                    reasons: Vec::new(),
                });
            }
        }
        if answer.media_sources.is_empty() {
            return Err(PlaybackError::NotPlayable {
                reasons: Vec::new(),
            });
        }
        let play_session_id = answer.play_session_id.unwrap_or_default();
        let base = self.stream_base()?;
        let token = auth::read(&self.token).ok_or(AuthError::NotSignedIn)?;
        let sources = answer
            .media_sources
            .into_iter()
            .map(|source| {
                let tag = source.e_tag.clone();
                let direct = source.supports_direct_play;
                let transcoding = (
                    source.transcoding_url.clone(),
                    source.transcoding_sub_protocol.clone(),
                    source.transcoding_container.clone(),
                );
                let source = map_source(source);
                let stream = if direct {
                    Some(PlayableStream {
                        route: Route::DirectPlay,
                        reasons: Vec::new(),
                        url: static_url(
                            &base,
                            item,
                            &source.id,
                            &play_session_id,
                            tag.as_deref(),
                            token.expose(),
                        ),
                    })
                } else {
                    match transcoding {
                        (Some(url), protocol, container) => server_stream(
                            &base,
                            &url,
                            protocol.as_deref(),
                            container,
                            &source,
                            token.expose(),
                        ),
                        (None, ..) => None,
                    }
                };
                SourceAnswer { source, stream }
            })
            .collect();
        Ok(PlaybackAnswer {
            play_session_id,
            sources,
        })
    }

    /// Reports that playback started (`POST /Sessions/Playing`).
    pub async fn report_start(&self, report: &ProgressReport) -> Result<(), AuthError> {
        self.report("/Sessions/Playing", &progress_body(report), None)
            .await
    }

    /// Reports the position and state (`POST /Sessions/Playing/Progress`). It also keeps the server's transcode alive.
    pub async fn report_progress(&self, report: &ProgressReport) -> Result<(), AuthError> {
        self.report("/Sessions/Playing/Progress", &progress_body(report), None)
            .await
    }

    /// Reports that playback stopped (`POST /Sessions/Playing/Stopped`); the server ends the play session's transcodes and decides
    /// from the position whether the item was played. Waits at most [`STOP_REPORT_TIMEOUT`].
    pub async fn report_stopped(&self, report: &StopReport) -> Result<(), AuthError> {
        let body = wire::PlaybackStopInfo {
            item_id: report.item.as_str(),
            media_source_id: &report.media_source,
            play_session_id: &report.play_session_id,
            position_ticks: ticks(report.position),
            failed: report.failed,
        };
        self.report(
            "/Sessions/Playing/Stopped",
            &body,
            Some(STOP_REPORT_TIMEOUT),
        )
        .await
    }

    async fn report(
        &self,
        path: &str,
        body: &impl serde::Serialize,
        timeout: Option<Duration>,
    ) -> Result<(), AuthError> {
        let mut request = Request::post(path).json(body)?;
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        self.send_authenticated(request).await.map(|_| ())
    }

    /// The item's media segments (`GET /MediaSegments/{item}`), in the server's order. Only worth asking when the source
    /// `has_segments`: segments come from server plugins only.
    pub async fn media_segments(&self, item: &ItemId) -> Result<Vec<Segment>, AuthError> {
        let path = format!("/MediaSegments/{}", item.as_str());
        let segments: wire::MediaSegments =
            self.send_authenticated(Request::get(path)).await?.json()?;
        Ok(segments
            .items
            .into_iter()
            .filter(|segment| segment.end_ticks > segment.start_ticks && segment.start_ticks >= 0)
            .map(|segment| Segment {
                kind: segment_kind(segment.r#type.as_deref().unwrap_or("Unknown")),
                start: duration(segment.start_ticks),
                end: duration(segment.end_ticks),
            })
            .collect())
    }

    /// What the server reports about its transcode for this device's session (`GET /Sessions?deviceId=...`), if one runs.
    pub async fn transcoding_facts(&self) -> Result<Option<TranscodingFacts>, AuthError> {
        let request = Request::get("/Sessions").query("deviceId", self.device_id.clone());
        let sessions: Vec<wire::SessionInfo> = self.send_authenticated(request).await?.json()?;
        Ok(sessions
            .into_iter()
            .filter(|session| session.device_id.as_deref() == Some(self.device_id.as_str()))
            .find_map(|session| session.transcoding_info)
            .map(map_transcoding))
    }

    /// The signed-in user's playback preferences (`GET /Users/Me`, its `Configuration`).
    pub async fn playback_preferences(&self) -> Result<PlaybackPreferences, AuthError> {
        let user: wire::UserWithConfiguration = self
            .send_authenticated(Request::get("/Users/Me"))
            .await?
            .json()?;
        let configuration = user.configuration;
        let flag = |read: fn(&wire::UserConfiguration) -> Option<bool>| {
            // The server's constructor sets these four to true.
            configuration.as_ref().and_then(read).unwrap_or(true)
        };
        Ok(PlaybackPreferences {
            audio_language: configuration
                .as_ref()
                .and_then(|c| non_empty(c.audio_language_preference.clone())),
            play_default_audio_track: flag(|c| c.play_default_audio_track),
            subtitle_language: configuration
                .as_ref()
                .and_then(|c| non_empty(c.subtitle_language_preference.clone())),
            subtitle_mode: configuration
                .as_ref()
                .and_then(|c| c.subtitle_mode.as_deref())
                .map_or(SubtitleMode::DEFAULT, subtitle_mode),
            next_episode_autoplay: flag(|c| c.enable_next_episode_auto_play),
            remember_audio: flag(|c| c.remember_audio_selections),
            remember_subtitles: flag(|c| c.remember_subtitle_selections),
        })
    }

    /// The base of stream URLs: the address the client sends its token to now, so the token's transport rule (ADR 0012) holds.
    fn stream_base(&self) -> Result<Url, AuthError> {
        self.http
            .current_address()
            .map(|selected| selected.url().clone())
            .ok_or(AuthError::Network(NetError::Unreachable))
    }
}

fn progress_body(report: &ProgressReport) -> wire::PlaybackProgressInfo<'_> {
    wire::PlaybackProgressInfo {
        item_id: report.item.as_str(),
        media_source_id: &report.media_source,
        play_session_id: &report.play_session_id,
        position_ticks: ticks(report.position),
        audio_stream_index: report.audio.map(index_to_wire),
        subtitle_stream_index: Some(report.subtitle.map_or(-1, index_to_wire)),
        is_paused: report.paused,
        is_muted: report.muted,
        volume_level: report.volume.map(|volume| i32::from(volume.min(100))),
        can_seek: report.can_seek,
        play_method: if report.route.is_direct_play() {
            "DirectPlay"
        } else {
            "Transcode"
        },
    }
}

/// `/Videos/{item}/stream?static=true&mediaSourceId=..&playSessionId=..&tag=..&ApiKey=..` on `base` (`VideosController.GetVideoStream`).
/// Never the source's own path, even for a remote source: the server proxies those.
fn static_url(
    base: &Url,
    item: &ItemId,
    media_source: &str,
    play_session_id: &str,
    tag: Option<&str>,
    token: &str,
) -> StreamUrl {
    let mut url = base.clone();
    if let Ok(mut segments) = url.path_segments_mut() {
        segments
            .pop_if_empty()
            .extend(["Videos", item.as_str(), "stream"]);
    }
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("static", "true")
            .append_pair("mediaSourceId", media_source)
            .append_pair("playSessionId", play_session_id);
        if let Some(tag) = tag {
            query.append_pair("tag", tag);
        }
        query.append_pair("ApiKey", token);
    }
    StreamUrl(url)
}

/// The server stream for a source from its `TranscodingUrl`, or `None` when the URL is not a path on the server (an absolute URL
/// would lead elsewhere, and the token must not).
fn server_stream(
    base: &Url,
    transcoding_url: &str,
    sub_protocol: Option<&str>,
    container: Option<String>,
    source: &MediaSource,
    token: &str,
) -> Option<PlayableStream> {
    if !transcoding_url.starts_with('/') || transcoding_url.starts_with("//") {
        return None;
    }
    let (path, query) = transcoding_url
        .split_once('?')
        .unwrap_or((transcoding_url, ""));
    let pairs: Vec<(String, String)> = url_query_pairs(query);
    let mut url = base.clone();
    if let Ok(mut segments) = url.path_segments_mut() {
        segments
            .pop_if_empty()
            .extend(path.split('/').filter(|segment| !segment.is_empty()));
    }
    url.set_query(if query.is_empty() { None } else { Some(query) });
    if !pairs
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("ApiKey"))
    {
        url.query_pairs_mut().append_pair("ApiKey", token);
    }
    let file = path.rsplit('/').next().unwrap_or_default();
    let protocol = match sub_protocol {
        Some(protocol) if protocol.eq_ignore_ascii_case("hls") => StreamProtocol::Hls,
        Some(_) => StreamProtocol::Progressive,
        None if file.ends_with(".m3u8") => StreamProtocol::Hls,
        None => StreamProtocol::Progressive,
    };
    let container = container.filter(|c| !c.is_empty()).or_else(|| {
        file.strip_prefix("stream.")
            .filter(|ext| !ext.is_empty())
            .map(str::to_owned)
    });
    let (route, reasons) = predict(protocol, container, &pairs, source);
    Some(PlayableStream {
        route,
        reasons,
        url: StreamUrl(url),
    })
}

fn url_query_pairs(query: &str) -> Vec<(String, String)> {
    current_net::Url::parse("http://127.0.0.1/")
        .map(|mut url| {
            url.set_query(Some(query));
            url.query_pairs().into_owned().collect()
        })
        .unwrap_or_default()
}

/// Reasons that force the video to be converted.
const VIDEO_REASONS: &[RouteReason] = &[
    RouteReason::VIDEO_CODEC_NOT_SUPPORTED,
    RouteReason::VIDEO_PROFILE_NOT_SUPPORTED,
    RouteReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED,
    RouteReason::VIDEO_CODEC_TAG_NOT_SUPPORTED,
    RouteReason::VIDEO_LEVEL_NOT_SUPPORTED,
    RouteReason::VIDEO_RESOLUTION_NOT_SUPPORTED,
    RouteReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
    RouteReason::VIDEO_FRAMERATE_NOT_SUPPORTED,
    RouteReason::VIDEO_ROTATION_NOT_SUPPORTED,
    RouteReason::REF_FRAMES_NOT_SUPPORTED,
    RouteReason::ANAMORPHIC_VIDEO_NOT_SUPPORTED,
    RouteReason::INTERLACED_VIDEO_NOT_SUPPORTED,
    RouteReason::VIDEO_BITRATE_NOT_SUPPORTED,
    RouteReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
    RouteReason::UNKNOWN_VIDEO_STREAM_INFO,
];

/// Reasons that force the audio to be converted.
const AUDIO_REASONS: &[RouteReason] = &[
    RouteReason::AUDIO_CODEC_NOT_SUPPORTED,
    RouteReason::AUDIO_CHANNELS_NOT_SUPPORTED,
    RouteReason::AUDIO_PROFILE_NOT_SUPPORTED,
    RouteReason::AUDIO_SAMPLE_RATE_NOT_SUPPORTED,
    RouteReason::AUDIO_BIT_DEPTH_NOT_SUPPORTED,
    RouteReason::AUDIO_BITRATE_NOT_SUPPORTED,
    RouteReason::UNKNOWN_AUDIO_STREAM_INFO,
];

/// The route a `TranscodingUrl` predicts: its reasons, and whether video and audio are copied. A stream is predicted converted
/// when a reason names it, when copying it is not allowed (`allowVideoStreamCopy=false`, `allowAudioStreamCopy=false`), when its
/// codec is not among the URL's output codecs, or (video) when a subtitle is burned in (`SubtitleMethod=Encode`).
fn predict(
    protocol: StreamProtocol,
    container: Option<String>,
    pairs: &[(String, String)],
    source: &MediaSource,
) -> (Route, Vec<RouteReason>) {
    let value = |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    };
    let reasons: Vec<RouteReason> = value("TranscodeReasons")
        .map(|names| {
            names
                .split(',')
                .filter(|name| !name.is_empty())
                .map(route_reason)
                .collect()
        })
        .unwrap_or_default();
    let listed = |key: &str, codec: Option<&Codec>| {
        let Some(codec) = codec else {
            return false;
        };
        value(key).is_some_and(|names| {
            names
                .split(',')
                .any(|name| codec_from_server(name) == *codec)
        })
    };
    let refused = |key: &str| value(key).is_some_and(|v| v.eq_ignore_ascii_case("false"));
    let video_codec = source.video().and_then(|stream| stream.codec.as_ref());
    let audio_index = value("AudioStreamIndex")
        .and_then(|index| index.parse::<u32>().ok())
        .or(source.default_audio);
    let audio_stream = audio_index
        .and_then(|index| source.stream(index))
        .or_else(|| source.streams_of(&StreamKind::AUDIO).next());
    let audio_codec = audio_stream.and_then(|stream| stream.codec.as_ref());
    let video = if reasons.iter().any(|reason| VIDEO_REASONS.contains(reason))
        || refused("allowVideoStreamCopy")
        || value("SubtitleMethod").is_some_and(|method| method.eq_ignore_ascii_case("Encode"))
        || !listed("VideoCodec", video_codec)
    {
        StreamAction::Transcode
    } else {
        StreamAction::Copy
    };
    let audio = if reasons.iter().any(|reason| AUDIO_REASONS.contains(reason))
        || refused("allowAudioStreamCopy")
        || !listed("AudioCodec", audio_codec)
    {
        StreamAction::Transcode
    } else {
        StreamAction::Copy
    };
    (
        Route::ServerStream {
            protocol,
            container,
            video,
            audio,
        },
        reasons,
    )
}

/// A `TranscodeReason` name as the model's constant (`ContainerNotSupported` to `container_not_supported`); a name the model does
/// not know passes through unchanged.
fn route_reason(name: &str) -> RouteReason {
    let mut snake = String::with_capacity(name.len() + 8);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        } else {
            snake.push(c);
        }
    }
    let known = RouteReason::new(snake);
    if known.is_known() {
        known
    } else {
        RouteReason::new(name)
    }
}

/// A codec name from the server as the model's (FFmpeg's) name: the server renames some subtitle codecs
/// (`ProbeResultNormalizer.NormalizeSubtitleCodec`), and external subtitle files carry their extension. Others pass through.
fn codec_from_server(name: &str) -> Codec {
    let renamed = [
        ("PGSSUB", Codec::PGS),
        ("DVDSUB", Codec::DVD_SUBTITLE),
        ("DVBSUB", Codec::DVB_SUBTITLE),
        ("srt", Codec::SUBRIP),
        ("vtt", Codec::WEBVTT),
    ];
    renamed
        .into_iter()
        .find(|(server, _)| server.eq_ignore_ascii_case(name))
        .map_or_else(|| Codec::new(name), |(_, codec)| codec)
}

fn map_source(source: wire::MediaSourceInfo) -> MediaSource {
    MediaSource {
        id: source.id.unwrap_or_default(),
        name: non_empty(source.name),
        container: non_empty(source.container),
        size: source.size.and_then(|size| u64::try_from(size).ok()),
        bitrate: source.bitrate.and_then(|rate| u32::try_from(rate).ok()),
        duration: source.run_time_ticks.filter(|t| *t > 0).map(duration),
        streams: source
            .media_streams
            .into_iter()
            .filter_map(map_stream)
            .collect(),
        default_audio: source.default_audio_stream_index.and_then(index_from_wire),
        default_subtitle: source
            .default_subtitle_stream_index
            .and_then(index_from_wire),
        has_segments: source.has_segments,
        is_remote: source.is_remote,
    }
}

fn map_stream(stream: wire::MediaStream) -> Option<MediaStream> {
    let index = index_from_wire(stream.index)?;
    let kind = match stream.r#type.as_deref() {
        Some("Video") => StreamKind::VIDEO,
        Some("Audio") => StreamKind::AUDIO,
        Some("Subtitle") => StreamKind::SUBTITLE,
        Some("EmbeddedImage") => StreamKind::EMBEDDED_IMAGE,
        Some("Data") => StreamKind::DATA,
        Some("Lyric") => StreamKind::LYRIC,
        Some(other) => StreamKind::new(other),
        None => StreamKind::new(""),
    };
    let positive =
        |value: Option<i32>| value.and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    let video = (kind == StreamKind::VIDEO).then(|| VideoFacts {
        width: positive(stream.width),
        height: positive(stream.height),
        bit_depth: positive(stream.bit_depth),
        frame_rate: stream
            .real_frame_rate
            .or(stream.average_frame_rate)
            .filter(|rate| *rate > 0.0),
        range: stream.video_range.as_deref().and_then(video_range),
        range_type: stream
            .video_range_type
            .as_deref()
            .and_then(video_range_type),
        dv_profile: stream.dv_profile.and_then(|v| u32::try_from(v).ok()),
    });
    let audio = (kind == StreamKind::AUDIO).then(|| AudioFacts {
        channels: positive(stream.channels),
        layout: non_empty(stream.channel_layout),
        sample_rate: positive(stream.sample_rate),
    });
    let subtitle_delivery = if kind == StreamKind::SUBTITLE {
        stream.delivery_method.as_deref().map(subtitle_delivery)
    } else {
        None
    };
    Some(MediaStream {
        index,
        codec: non_empty(stream.codec).map(|codec| codec_from_server(&codec)),
        profile: non_empty(stream.profile),
        level: stream.level.filter(|level| *level > 0.0),
        language: non_empty(stream.language),
        title: non_empty(stream.title),
        display_title: non_empty(stream.display_title),
        is_default: stream.is_default,
        is_forced: stream.is_forced,
        is_hearing_impaired: stream.is_hearing_impaired,
        is_external: stream.is_external,
        kind,
        video,
        audio,
        subtitle_delivery,
    })
}

/// `VideoRange`: `Unknown`, `SDR`, `HDR`.
fn video_range(name: &str) -> Option<VideoRange> {
    match name {
        "Unknown" | "" => None,
        "SDR" => Some(VideoRange::SDR),
        "HDR" => Some(VideoRange::HDR),
        other => Some(VideoRange::new(other)),
    }
}

/// `VideoRangeType` (`Jellyfin.Data.Enums`).
fn video_range_type(name: &str) -> Option<VideoRangeType> {
    Some(match name {
        "Unknown" | "" => return None,
        "SDR" => VideoRangeType::SDR,
        "HDR10" => VideoRangeType::HDR10,
        "HDR10Plus" => VideoRangeType::HDR10_PLUS,
        "HLG" => VideoRangeType::HLG,
        "DOVI" => VideoRangeType::DOVI,
        "DOVIWithHDR10" => VideoRangeType::DOVI_WITH_HDR10,
        "DOVIWithHLG" => VideoRangeType::DOVI_WITH_HLG,
        "DOVIWithSDR" => VideoRangeType::DOVI_WITH_SDR,
        "DOVIWithEL" => VideoRangeType::DOVI_WITH_EL,
        "DOVIWithHDR10Plus" => VideoRangeType::DOVI_WITH_HDR10_PLUS,
        "DOVIWithELHDR10Plus" => VideoRangeType::DOVI_WITH_EL_HDR10_PLUS,
        "DOVIInvalid" => VideoRangeType::DOVI_INVALID,
        other => VideoRangeType::new(other),
    })
}

/// `SubtitleDeliveryMethod`: `Encode`, `Embed`, `External`, `Hls`, `Drop`.
fn subtitle_delivery(name: &str) -> SubtitleDelivery {
    match name {
        "Encode" => SubtitleDelivery::ENCODE,
        "Embed" => SubtitleDelivery::EMBED,
        "External" => SubtitleDelivery::EXTERNAL,
        "Hls" => SubtitleDelivery::HLS,
        "Drop" => SubtitleDelivery::DROP,
        other => SubtitleDelivery::new(other),
    }
}

/// `MediaSegmentType`: `Unknown`, `Commercial`, `Preview`, `Recap`, `Outro`, `Intro`; anything else under the server's name.
fn segment_kind(name: &str) -> SegmentKind {
    match name {
        "Intro" => SegmentKind::INTRO,
        "Outro" => SegmentKind::OUTRO,
        "Recap" => SegmentKind::RECAP,
        "Preview" => SegmentKind::PREVIEW,
        "Commercial" => SegmentKind::COMMERCIAL,
        other => SegmentKind::new(other),
    }
}

/// `SubtitlePlaybackMode`: `Default`, `Always`, `OnlyForced`, `None`, `Smart`.
fn subtitle_mode(name: &str) -> SubtitleMode {
    match name {
        "Default" => SubtitleMode::DEFAULT,
        "Always" => SubtitleMode::ALWAYS,
        "OnlyForced" => SubtitleMode::ONLY_FORCED,
        "None" => SubtitleMode::NONE,
        "Smart" => SubtitleMode::SMART,
        other => SubtitleMode::new(other),
    }
}

fn map_transcoding(info: wire::TranscodingInfo) -> TranscodingFacts {
    let positive =
        |value: Option<i32>| value.and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    TranscodingFacts {
        video_direct: info.is_video_direct,
        audio_direct: info.is_audio_direct,
        video_codec: non_empty(info.video_codec).map(|codec| codec_from_server(&codec)),
        audio_codec: non_empty(info.audio_codec).map(|codec| codec_from_server(&codec)),
        container: non_empty(info.container),
        bitrate: info.bitrate.and_then(|rate| u32::try_from(rate).ok()),
        frame_rate: info.framerate,
        completion: info.completion_percentage,
        width: positive(info.width),
        height: positive(info.height),
        audio_channels: positive(info.audio_channels),
        hardware_acceleration: non_empty(info.hardware_acceleration_type)
            .filter(|kind| !kind.eq_ignore_ascii_case("none")),
        reasons: info
            .transcode_reasons
            .iter()
            .map(|name| route_reason(name))
            .collect(),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// Jellyfin's ticks are 100 ns.
fn ticks(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX)
}

fn duration(ticks: i64) -> Duration {
    Duration::from_nanos(u64::try_from(ticks).unwrap_or(0).saturating_mul(100))
}

fn index_to_wire(index: u32) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// A stream index from the server; negative means none.
fn index_from_wire(index: i32) -> Option<u32> {
    u32::try_from(index).ok()
}

#[cfg(test)]
#[path = "playback_tests.rs"]
mod tests;
