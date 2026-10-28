//! The Jellyfin device profile, generated from what the engine can play (ADR 0019, "Engine capabilities and the device profile",
//! choices 1 and 2): one direct-play profile per container, HLS with fMP4 segments first and HLS with MPEG-TS segments second,
//! subtitle profiles for every format the engine renders, codec profiles only for the engine's own codec limits, and
//! `MaxStreamingBitrate` always set. [`EngineCapabilities`] already speaks Jellyfin's names, so they go into the profile unchanged.
//!
//! Server behaviour relied on, from Jellyfin `v12.2` (commit ca0f16eb, read 2026-10-25):
//! - `ContainerHelper.ContainsContainer` compares comma-separated lists without case, and an empty list matches everything ("Empty
//!   profiles always support all containers/codecs"). A profile is therefore never sent with an empty codec list.
//! - Subtitle profiles match a stream when `profile.Format` equals the stream's codec without case (`StreamBuilder.GetSubtitleProfile`).
//! - `ConditionProcessor` splits an `EqualsAny` value on `|`.
//! - HLS audio the server accepts (`StreamBuilder`): MPEG-TS `aac, ac3, eac3, mp3`; fMP4 also `alac, flac, opus, dts, truehd`. Video
//!   `h264, hevc, vp9, av1`.

use current_model::{Codec, CodecSupport, Container, EngineCapabilities, SubtitleFormat};

use crate::playback_wire::{
    CodecProfile, DeviceProfile, DirectPlayProfile, ProfileCondition, SubtitleProfile,
    TranscodingProfile,
};

/// The profile's name, shown in the server's logs.
const PROFILE_NAME: &str = "Current";

/// The HLS fMP4 profile's codecs, in the server's order of preference: HEVC keeps 4K transcodes at a reasonable bitrate (choice 2).
const FMP4_VIDEO: &[Codec] = &[Codec::HEVC, Codec::H264];
const FMP4_AUDIO: &[Codec] = &[
    Codec::AAC,
    Codec::AC3,
    Codec::EAC3,
    Codec::OPUS,
    Codec::FLAC,
];
/// The HLS MPEG-TS profile: the server's fallback for sources marked `UseMostCompatibleTranscodingProfile`.
const TS_VIDEO: &[Codec] = &[Codec::H264];
const TS_AUDIO: &[Codec] = &[Codec::AAC, Codec::AC3, Codec::EAC3, Codec::MP3];

/// Text subtitle formats the engine can take as a separate file, and PGS (choice 1: "`External` for text formats and PGS files").
const EXTERNAL_SUBTITLES: &[SubtitleFormat] = &[
    SubtitleFormat::SUBRIP,
    SubtitleFormat::SRT,
    SubtitleFormat::ASS,
    SubtitleFormat::SSA,
    SubtitleFormat::WEBVTT,
    SubtitleFormat::VTT,
    SubtitleFormat::PGSSUB,
];

/// The device profile for `capabilities`, with `max_streaming_bitrate` in bits per second.
pub(crate) fn device_profile(
    capabilities: &EngineCapabilities,
    max_streaming_bitrate: i32,
) -> DeviceProfile {
    let limited = |supports: &[CodecSupport], kind| {
        supports
            .iter()
            .filter(|support| support.has_limits())
            .filter_map(move |support| codec_profile(support, kind))
            .collect::<Vec<_>>()
    };
    let mut codec_profiles = limited(&capabilities.video, Kind::Video);
    codec_profiles.extend(limited(&capabilities.audio, Kind::Audio));
    DeviceProfile {
        name: PROFILE_NAME,
        max_streaming_bitrate,
        direct_play_profiles: direct_play_profiles(capabilities),
        transcoding_profiles: transcoding_profiles(capabilities),
        container_profiles: Vec::new(),
        codec_profiles,
        subtitle_profiles: subtitle_profiles(capabilities),
    }
}

fn direct_play_profiles(capabilities: &EngineCapabilities) -> Vec<DirectPlayProfile> {
    let video = join(
        capabilities
            .video
            .iter()
            .map(|support| support.codec.as_str()),
    );
    let audio = join(
        capabilities
            .audio
            .iter()
            .map(|support| support.codec.as_str()),
    );
    if video.is_empty() || audio.is_empty() {
        return Vec::new();
    }
    capabilities
        .containers
        .iter()
        .map(|container| DirectPlayProfile {
            container: container.as_str().to_owned(),
            r#type: "Video",
            video_codec: video.clone(),
            audio_codec: audio.clone(),
        })
        .collect()
}

fn transcoding_profiles(capabilities: &EngineCapabilities) -> Vec<TranscodingProfile> {
    let profile = |container, video: &[Codec], audio: &[Codec]| {
        let video = join(
            video
                .iter()
                .filter(|codec| capabilities.decodes_video(codec))
                .map(Codec::as_str),
        );
        let audio = join(
            audio
                .iter()
                .filter(|codec| capabilities.decodes_audio(codec))
                .map(Codec::as_str),
        );
        (!video.is_empty() && !audio.is_empty()).then_some(TranscodingProfile {
            container,
            r#type: "Video",
            video_codec: video,
            audio_codec: audio,
            protocol: "hls",
            context: "Streaming",
        })
    };
    let demuxes = |container: &Container| capabilities.containers.contains(container);
    [
        demuxes(&Container::MP4)
            .then(|| profile("mp4", FMP4_VIDEO, FMP4_AUDIO))
            .flatten(),
        demuxes(&Container::TS)
            .then(|| profile("ts", TS_VIDEO, TS_AUDIO))
            .flatten(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The device profile for `capabilities` as PlaybackInfo's body carries it (`DeviceProfile`), with `max_streaming_bitrate` in bits
/// per second: for checks that a change to how the capabilities are made leaves the profile the server gets unchanged (ADR 0025).
pub fn device_profile_json(
    capabilities: &EngineCapabilities,
    max_streaming_bitrate: i32,
) -> String {
    // The profile is plain data with string keys, so serializing it cannot fail; an empty string would fail any comparison.
    serde_json::to_string(&device_profile(capabilities, max_streaming_bitrate)).unwrap_or_default()
}

/// What the server converts a stream to when this device cannot play it as it is (ADR 0021, the "Supported formats" page): the codecs
/// of the transcoding profiles [`device_profile`] sends for `capabilities`, fMP4 first, each once. Empty when no transcoding profile
/// is sent (the engine decodes none of the targets).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscodeTargets {
    pub video: Vec<Codec>,
    pub audio: Vec<Codec>,
}

/// The transcode targets for `capabilities`, read from the profiles themselves so they cannot drift from what the server is sent.
pub fn transcode_targets(capabilities: &EngineCapabilities) -> TranscodeTargets {
    let mut targets = TranscodeTargets::default();
    let add = |list: &mut Vec<Codec>, names: &str| {
        for name in names.split(',').filter(|name| !name.is_empty()) {
            let codec = Codec::new(name);
            if !list.contains(&codec) {
                list.push(codec);
            }
        }
    };
    for profile in transcoding_profiles(capabilities) {
        add(&mut targets.video, &profile.video_codec);
        add(&mut targets.audio, &profile.audio_codec);
    }
    targets
}

/// The subtitle formats the device profile offers as a separate file (`External`) for `capabilities`, in the profile's order (ADR 0021):
/// what the server can convert a text subtitle the engine does not render into.
pub fn external_subtitle_formats(capabilities: &EngineCapabilities) -> Vec<SubtitleFormat> {
    subtitle_profiles(capabilities)
        .into_iter()
        .filter(|profile| profile.method == "External")
        .map(|profile| SubtitleFormat::new(profile.format))
        .collect()
}

#[derive(Clone, Copy)]
enum Kind {
    Video,
    Audio,
}

/// The engine's limits for one codec as conditions the server must meet to send it as it is; `None` when no limit applies to the
/// kind (a level or a range on an audio codec).
fn codec_profile(support: &CodecSupport, kind: Kind) -> Option<CodecProfile> {
    let (r#type, bit_depth, profile) = match kind {
        Kind::Video => ("Video", "VideoBitDepth", "VideoProfile"),
        // Audio inside a video (`CodecType.VideoAudio`).
        Kind::Audio => ("VideoAudio", "AudioBitDepth", "AudioProfile"),
    };
    let mut conditions = Vec::new();
    if let Some(depth) = support.max_bit_depth {
        conditions.push(condition("LessThanEqual", bit_depth, depth.to_string()));
    }
    if let Some(profiles) = &support.profiles {
        conditions.push(condition("EqualsAny", profile, profiles.join("|")));
    }
    if let Kind::Video = kind {
        if let Some(level) = support.level {
            conditions.push(condition("LessThanEqual", "VideoLevel", level.to_string()));
        }
        if let Some(ranges) = &support.ranges {
            let ranges: Vec<&str> = ranges.iter().map(|range| range.as_str()).collect();
            conditions.push(condition("EqualsAny", "VideoRangeType", ranges.join("|")));
        }
    }
    (!conditions.is_empty()).then(|| CodecProfile {
        r#type,
        codec: support.codec.as_str().to_owned(),
        conditions,
    })
}

fn condition(condition: &'static str, property: &'static str, value: String) -> ProfileCondition {
    ProfileCondition {
        condition,
        property,
        value,
        // A condition the server cannot check (an unknown bit depth) fails only when required; unknown values pass.
        is_required: false,
    }
}

/// `Embed` for every format the engine renders inside the stream, and `External` for text formats and PGS when the engine takes
/// separate subtitle files.
fn subtitle_profiles(capabilities: &EngineCapabilities) -> Vec<SubtitleProfile> {
    let profile = |format: &SubtitleFormat, method| SubtitleProfile {
        format: format.as_str().to_owned(),
        method,
    };
    let embed = capabilities
        .subtitles
        .iter()
        .map(|format| profile(format, "Embed"));
    let external = capabilities
        .subtitles
        .iter()
        .filter(|format| capabilities.external_subtitles && EXTERNAL_SUBTITLES.contains(format))
        .map(|format| profile(format, "External"));
    embed.chain(external).collect()
}

/// Names joined with `,`, each once.
fn join<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for name in names {
        if !seen.contains(&name) {
            seen.push(name);
        }
    }
    seen.join(",")
}

#[cfg(test)]
mod tests {
    use current_model::VideoRangeType;

    use super::*;

    /// The lists `current-mpv` is expected to report for our Windows build (ADR 0019, "Engine capabilities";
    /// `build/mpv-windows/check.py`), in Jellyfin's names.
    fn mpv_windows() -> EngineCapabilities {
        EngineCapabilities {
            containers: vec![
                Container::MKV,
                Container::WEBM,
                Container::MP4,
                Container::MOV,
                Container::TS,
                Container::AVI,
                Container::OGG,
                Container::FLAC,
            ],
            video: [
                Codec::H264,
                Codec::HEVC,
                Codec::AV1,
                Codec::VP9,
                Codec::VP8,
                Codec::MPEG2,
                Codec::VC1,
            ]
            .into_iter()
            .map(CodecSupport::any)
            .collect(),
            audio: [
                Codec::AAC,
                Codec::AC3,
                Codec::EAC3,
                Codec::TRUEHD,
                Codec::DTS,
                Codec::FLAC,
                Codec::OPUS,
                Codec::VORBIS,
                Codec::MP3,
                Codec::PCM_S16LE,
                Codec::PCM_S24LE,
            ]
            .into_iter()
            .map(CodecSupport::any)
            .collect(),
            subtitles: vec![
                SubtitleFormat::ASS,
                SubtitleFormat::SUBRIP,
                SubtitleFormat::SRT,
                SubtitleFormat::WEBVTT,
                SubtitleFormat::VTT,
                SubtitleFormat::PGSSUB,
                SubtitleFormat::DVDSUB,
                SubtitleFormat::DVBSUB,
                SubtitleFormat::MOV_TEXT,
            ],
            external_subtitles: true,
        }
    }

    /// Checked by hand against `DeviceProfile.cs` and its siblings in Jellyfin v12.2: property names in PascalCase, enums as their
    /// names (`DlnaProfileType.Video`, `MediaStreamProtocol.hls`, `EncodingContext.Streaming`, `SubtitleDeliveryMethod.Embed` and
    /// `External`), comma-separated lists, the server's spellings for containers, codecs and subtitle formats.
    const SAMPLE: &str = r#"{
      "Name": "Current",
      "MaxStreamingBitrate": 2147483647,
      "DirectPlayProfiles": [
        {"Container": "mkv", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "webm", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "mp4", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "mov", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "ts", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "avi", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "ogg", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "flac", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"}
      ],
      "TranscodingProfiles": [
        {"Container": "mp4", "Type": "Video", "VideoCodec": "hevc,h264", "AudioCodec": "aac,ac3,eac3,opus,flac", "Protocol": "hls", "Context": "Streaming"},
        {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac,ac3,eac3,mp3", "Protocol": "hls", "Context": "Streaming"}
      ],
      "ContainerProfiles": [],
      "CodecProfiles": [],
      "SubtitleProfiles": [
        {"Format": "ass", "Method": "Embed"},
        {"Format": "subrip", "Method": "Embed"},
        {"Format": "srt", "Method": "Embed"},
        {"Format": "webvtt", "Method": "Embed"},
        {"Format": "vtt", "Method": "Embed"},
        {"Format": "pgssub", "Method": "Embed"},
        {"Format": "dvdsub", "Method": "Embed"},
        {"Format": "dvbsub", "Method": "Embed"},
        {"Format": "mov_text", "Method": "Embed"},
        {"Format": "ass", "Method": "External"},
        {"Format": "subrip", "Method": "External"},
        {"Format": "srt", "Method": "External"},
        {"Format": "webvtt", "Method": "External"},
        {"Format": "vtt", "Method": "External"},
        {"Format": "pgssub", "Method": "External"}
      ]
    }"#;

    #[test]
    fn the_profile_for_our_windows_build_matches_the_checked_sample() {
        let profile = device_profile(&mpv_windows(), i32::MAX);
        let actual = serde_json::to_value(&profile).unwrap();
        let expected: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_bitrate_is_always_sent() {
        let profile = device_profile(&mpv_windows(), 4_000_000);
        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(json["MaxStreamingBitrate"], 4_000_000);
    }

    #[test]
    fn transcoding_lists_only_what_the_engine_decodes() {
        let mut capabilities = mpv_windows();
        capabilities
            .video
            .retain(|support| support.codec == Codec::H264);
        capabilities.audio = vec![CodecSupport::any(Codec::AAC)];
        let json = serde_json::to_value(device_profile(&capabilities, i32::MAX)).unwrap();
        let profiles = json["TranscodingProfiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0]["VideoCodec"], "h264");
        assert_eq!(profiles[0]["AudioCodec"], "aac");
    }

    #[test]
    fn transcode_targets_are_the_profiles_codecs() {
        let targets = transcode_targets(&mpv_windows());
        assert_eq!(targets.video, vec![Codec::HEVC, Codec::H264]);
        assert_eq!(
            targets.audio,
            vec![
                Codec::AAC,
                Codec::AC3,
                Codec::EAC3,
                Codec::OPUS,
                Codec::FLAC,
                Codec::MP3
            ]
        );
        let mut h264_only = mpv_windows();
        h264_only
            .video
            .retain(|support| support.codec == Codec::H264);
        assert_eq!(transcode_targets(&h264_only).video, vec![Codec::H264]);
        assert_eq!(
            transcode_targets(&EngineCapabilities::default()),
            TranscodeTargets::default()
        );
    }

    #[test]
    fn external_subtitle_formats_are_the_external_profiles() {
        let mut capabilities = mpv_windows();
        capabilities.subtitles = vec![SubtitleFormat::SRT, SubtitleFormat::DVBSUB];
        assert_eq!(
            external_subtitle_formats(&capabilities),
            vec![SubtitleFormat::SRT]
        );
        capabilities.external_subtitles = false;
        assert!(external_subtitle_formats(&capabilities).is_empty());
    }

    #[test]
    fn no_profile_carries_an_empty_list() {
        let capabilities = EngineCapabilities {
            containers: vec![Container::MKV, Container::MP4],
            video: vec![CodecSupport::any(Codec::AV1)],
            audio: Vec::new(),
            subtitles: Vec::new(),
            external_subtitles: false,
        };
        let json = serde_json::to_value(device_profile(&capabilities, i32::MAX)).unwrap();
        assert_eq!(json["DirectPlayProfiles"], serde_json::json!([]));
        assert_eq!(json["TranscodingProfiles"], serde_json::json!([]));
        assert_eq!(json["SubtitleProfiles"], serde_json::json!([]));
    }

    #[test]
    fn codec_limits_become_codec_profiles() {
        let mut capabilities = mpv_windows();
        capabilities.video = vec![CodecSupport {
            codec: Codec::HEVC,
            max_bit_depth: Some(10),
            profiles: Some(vec!["main".into(), "main 10".into()]),
            level: Some(153),
            ranges: Some(vec![VideoRangeType::SDR, VideoRangeType::HDR10]),
        }];
        capabilities.audio = vec![
            CodecSupport {
                max_bit_depth: Some(24),
                // A level or a range means nothing for audio and is left out.
                level: Some(1),
                ..CodecSupport::any(Codec::FLAC)
            },
            CodecSupport {
                level: Some(1),
                ..CodecSupport::any(Codec::AAC)
            },
        ];
        let json = serde_json::to_value(device_profile(&capabilities, i32::MAX)).unwrap();
        assert_eq!(
            json["CodecProfiles"],
            serde_json::json!([
                {
                    "Type": "Video",
                    "Codec": "hevc",
                    "Conditions": [
                        {"Condition": "LessThanEqual", "Property": "VideoBitDepth", "Value": "10", "IsRequired": false},
                        {"Condition": "EqualsAny", "Property": "VideoProfile", "Value": "main|main 10", "IsRequired": false},
                        {"Condition": "LessThanEqual", "Property": "VideoLevel", "Value": "153", "IsRequired": false},
                        {"Condition": "EqualsAny", "Property": "VideoRangeType", "Value": "SDR|HDR10", "IsRequired": false}
                    ]
                },
                {
                    "Type": "VideoAudio",
                    "Codec": "flac",
                    "Conditions": [
                        {"Condition": "LessThanEqual", "Property": "AudioBitDepth", "Value": "24", "IsRequired": false}
                    ]
                }
            ])
        );
    }
}
