//! Jellyfin's wire types for playback: PlaybackInfo with the device profile, playstate reports, media segments, the session's
//! transcoding information and the user's playback configuration. Private: the public API speaks `current-model`.
//!
//! Shapes from Jellyfin `v12.2` (commit ca0f16eb, read 2026-10-25): `Jellyfin.Api/Models/MediaInfoDtos/PlaybackInfoDto.cs`,
//! `MediaBrowser.Model/Dlna/` (`DeviceProfile.cs`, `DirectPlayProfile.cs`, `TranscodingProfile.cs`, `CodecProfile.cs`,
//! `ProfileCondition.cs`, `SubtitleProfile.cs`), `MediaBrowser.Model/MediaInfo/PlaybackInfoResponse.cs`,
//! `MediaBrowser.Model/Dto/MediaSourceInfo.cs`, `MediaBrowser.Model/Entities/MediaStream.cs`, `MediaBrowser.Model/Session/`
//! (`PlaybackProgressInfo.cs`, `PlaybackStopInfo.cs`, `TranscodingInfo.cs`), `MediaBrowser.Model/Dto/SessionInfoDto.cs`,
//! `MediaBrowser.Model/MediaSegments/MediaSegmentDto.cs` and `MediaBrowser.Model/Configuration/UserConfiguration.cs`. Enums are
//! strings on the wire (`JsonDefaults` adds `JsonStringEnumConverter`), flag enums arrays of names (`JsonFlagEnumConverter`).
//!
//! Fields that hold a path or a URL with a token (`Path`, `DeliveryUrl`, `TranscodingUrl` aside, which the client reads and keeps
//! inside a [`StreamUrl`](crate::StreamUrl)) are not read at all.

use serde::{Deserialize, Serialize};

/// `POST /Items/{itemId}/PlaybackInfo` body (`PlaybackInfoDto`). Absent flags default to true on the server; Current sends them all.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaybackInfoDto<'a> {
    pub max_streaming_bitrate: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_id: Option<&'a str>,
    pub device_profile: &'a DeviceProfile,
    pub enable_direct_play: bool,
    pub enable_transcoding: bool,
    pub allow_video_stream_copy: bool,
    pub allow_audio_stream_copy: bool,
}

/// `DeviceProfile`. `MaxStreamingBitrate` is always set: the server's default is 8 Mbit/s.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct DeviceProfile {
    pub name: &'static str,
    pub max_streaming_bitrate: i32,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub container_profiles: Vec<()>,
    pub codec_profiles: Vec<CodecProfile>,
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

/// `DirectPlayProfile`. An empty codec list means every codec to the server (`ContainerHelper.ContainsContainer`: "Empty profiles
/// always support all containers/codecs"), so the builder never sends one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct DirectPlayProfile {
    pub container: String,
    pub r#type: &'static str,
    pub video_codec: String,
    pub audio_codec: String,
}

/// `TranscodingProfile`, the fields Current sets; the rest keep the server's defaults.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TranscodingProfile {
    pub container: &'static str,
    pub r#type: &'static str,
    pub video_codec: String,
    pub audio_codec: String,
    pub protocol: &'static str,
    pub context: &'static str,
}

/// `CodecProfile`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CodecProfile {
    pub r#type: &'static str,
    pub codec: String,
    pub conditions: Vec<ProfileCondition>,
}

/// `ProfileCondition`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ProfileCondition {
    pub condition: &'static str,
    pub property: &'static str,
    pub value: String,
    pub is_required: bool,
}

/// `SubtitleProfile`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SubtitleProfile {
    pub format: &'static str,
    pub method: &'static str,
}

/// `PlaybackInfoResponse`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaybackInfoResponse {
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
    pub play_session_id: Option<String>,
    /// `PlaybackErrorCode`: `NotAllowed`, `NoCompatibleStream`, `RateLimitExceeded`.
    pub error_code: Option<String>,
}

/// `MediaSourceInfo`, the fields Current reads. `Path`, `EncoderPath` and `RequiredHttpHeaders` are left out on purpose. Also read
/// inside `BaseItemDto` by browsing, whose media summary is derived from it (ADR 0018, ADR 0019).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaSourceInfo {
    pub id: Option<String>,
    pub container: Option<String>,
    pub size: Option<i64>,
    pub name: Option<String>,
    pub is_remote: bool,
    #[serde(rename = "ETag")]
    pub e_tag: Option<String>,
    pub run_time_ticks: Option<i64>,
    pub supports_direct_play: bool,
    pub media_streams: Vec<MediaStream>,
    pub bitrate: Option<i64>,
    pub transcoding_url: Option<String>,
    pub transcoding_sub_protocol: Option<String>,
    pub transcoding_container: Option<String>,
    pub default_audio_stream_index: Option<i32>,
    pub default_subtitle_stream_index: Option<i32>,
    pub has_segments: bool,
}

/// `MediaStream`, the fields Current reads. `Path` and `DeliveryUrl` (which carries `ApiKey`) are left out on purpose.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MediaStream {
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub display_title: Option<String>,
    pub profile: Option<String>,
    pub level: Option<f64>,
    pub r#type: Option<String>,
    /// A stream without an index is left out: the server's index is how reports name it.
    pub index: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub is_external: bool,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub bit_depth: Option<i32>,
    pub real_frame_rate: Option<f32>,
    pub average_frame_rate: Option<f32>,
    pub video_range: Option<String>,
    pub video_range_type: Option<String>,
    pub dv_profile: Option<i32>,
    pub channels: Option<i32>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<i32>,
    pub delivery_method: Option<String>,
}

/// `POST /Sessions/Playing` and `/Sessions/Playing/Progress` body (`PlaybackStartInfo`, `PlaybackProgressInfo`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaybackProgressInfo<'a> {
    pub item_id: &'a str,
    pub media_source_id: &'a str,
    pub play_session_id: &'a str,
    pub position_ticks: i64,
    pub audio_stream_index: Option<i32>,
    /// `-1` for no subtitle: the server remembers it as the user's choice (`SessionManager.UpdatePlaybackSettings`).
    pub subtitle_stream_index: Option<i32>,
    pub is_paused: bool,
    pub is_muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<i32>,
    pub can_seek: bool,
    /// `DirectPlay` or `Transcode` (`PlayMethod`).
    pub play_method: &'static str,
}

/// `POST /Sessions/Playing/Stopped` body (`PlaybackStopInfo`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaybackStopInfo<'a> {
    pub item_id: &'a str,
    pub media_source_id: &'a str,
    pub play_session_id: &'a str,
    pub position_ticks: i64,
    pub failed: bool,
}

/// `QueryResult<MediaSegmentDto>`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MediaSegments {
    #[serde(default)]
    pub items: Vec<MediaSegmentDto>,
}

/// `MediaSegmentDto`. `Type` is `MediaSegmentType`: `Unknown`, `Commercial`, `Preview`, `Recap`, `Outro`, `Intro`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MediaSegmentDto {
    pub r#type: Option<String>,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

/// `SessionInfoDto`, the fields Current reads.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SessionInfo {
    pub device_id: Option<String>,
    pub transcoding_info: Option<TranscodingInfo>,
}

/// `TranscodingInfo`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TranscodingInfo {
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub container: Option<String>,
    #[serde(default)]
    pub is_video_direct: bool,
    #[serde(default)]
    pub is_audio_direct: bool,
    pub bitrate: Option<i64>,
    pub framerate: Option<f32>,
    pub completion_percentage: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub audio_channels: Option<i32>,
    pub hardware_acceleration_type: Option<String>,
    #[serde(default)]
    pub transcode_reasons: Vec<String>,
}

/// `UserDto` with its `Configuration`, for the playback preferences.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct UserWithConfiguration {
    pub configuration: Option<UserConfiguration>,
}

/// `UserConfiguration`, the playback fields. The server's constructor sets `PlayDefaultAudioTrack`, `RememberAudioSelections`,
/// `RememberSubtitleSelections` and `EnableNextEpisodeAutoPlay` to true, so a missing one reads as true.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct UserConfiguration {
    pub audio_language_preference: Option<String>,
    pub play_default_audio_track: Option<bool>,
    pub subtitle_language_preference: Option<String>,
    /// `SubtitlePlaybackMode`: `Default`, `Always`, `OnlyForced`, `None`, `Smart`.
    pub subtitle_mode: Option<String>,
    pub remember_audio_selections: Option<bool>,
    pub remember_subtitle_selections: Option<bool>,
    pub enable_next_episode_auto_play: Option<bool>,
}
