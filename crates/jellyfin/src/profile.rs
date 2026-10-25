//! The Jellyfin device profile, generated from what the engine can play (ADR 0019, "Engine capabilities and the device profile",
//! choices 1 and 2): one direct-play profile per container, HLS with fMP4 segments first and HLS with MPEG-TS segments second,
//! subtitle profiles for every format the engine renders, codec profiles only for the engine's own codec limits, and
//! `MaxStreamingBitrate` always set.
//!
//! Server behaviour relied on, from Jellyfin `v12.2` (commit ca0f16eb, read 2026-10-25):
//! - `ContainerHelper.ContainsContainer` compares comma-separated lists without case, and an empty list matches everything ("Empty
//!   profiles always support all containers/codecs"). A profile is therefore never sent with an empty codec list.
//! - The server probes containers under FFmpeg's names but renames `matroska` to `mkv` and `mpegts` to `ts`
//!   (`ProbeResultNormalizer.NormalizeFormat`), and subtitle codecs `hdmv_pgs_subtitle`, `dvd_subtitle`, `dvb_subtitle` to `PGSSUB`,
//!   `DVDSUB`, `DVBSUB` (`NormalizeSubtitleCodec`); external `.srt` and `.vtt` files carry their extension as codec.
//! - HLS audio the server accepts (`StreamBuilder`): MPEG-TS `aac, ac3, eac3, mp3`; fMP4 also `alac, flac, opus, dts, truehd`. Video
//!   `h264, hevc, vp9, av1`.

use current_model::{Codec, Container, EngineCapabilities, VideoCodecSupport};

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
const EXTERNAL_SUBTITLES: &[Codec] = &[
    Codec::SUBRIP,
    Codec::ASS,
    Codec::SSA,
    Codec::WEBVTT,
    Codec::PGS,
];

/// The device profile for `capabilities`, with `max_streaming_bitrate` in bits per second.
pub(crate) fn device_profile(
    capabilities: &EngineCapabilities,
    max_streaming_bitrate: i32,
) -> DeviceProfile {
    DeviceProfile {
        name: PROFILE_NAME,
        max_streaming_bitrate,
        direct_play_profiles: direct_play_profiles(capabilities),
        transcoding_profiles: transcoding_profiles(capabilities),
        container_profiles: Vec::new(),
        codec_profiles: capabilities
            .video_codecs
            .iter()
            .filter(|support| support.has_limits())
            .map(codec_profile)
            .collect(),
        subtitle_profiles: subtitle_profiles(capabilities),
    }
}

fn direct_play_profiles(capabilities: &EngineCapabilities) -> Vec<DirectPlayProfile> {
    let video = join(
        capabilities
            .video_codecs
            .iter()
            .flat_map(|support| codec_names(&support.codec)),
    );
    let audio = join(capabilities.audio_codecs.iter().flat_map(codec_names));
    if video.is_empty() || audio.is_empty() {
        return Vec::new();
    }
    capabilities
        .containers
        .iter()
        .map(|container| DirectPlayProfile {
            container: container_names(container),
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
                .flat_map(codec_names),
        );
        let audio = join(
            audio
                .iter()
                .filter(|codec| capabilities.decodes_audio(codec))
                .flat_map(codec_names),
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
    let demuxes_mp4 = capabilities.containers.contains(&Container::MP4);
    let demuxes_ts = capabilities.containers.contains(&Container::MPEG_TS);
    [
        demuxes_mp4
            .then(|| profile("mp4", FMP4_VIDEO, FMP4_AUDIO))
            .flatten(),
        demuxes_ts
            .then(|| profile("ts", TS_VIDEO, TS_AUDIO))
            .flatten(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The engine's limits for one video codec as conditions the server must meet to send it as it is.
fn codec_profile(support: &VideoCodecSupport) -> CodecProfile {
    let mut conditions = Vec::new();
    if let Some(depth) = support.max_bit_depth {
        conditions.push(condition(
            "LessThanEqual",
            "VideoBitDepth",
            depth.to_string(),
        ));
    }
    if let Some(level) = support.max_level {
        conditions.push(condition("LessThanEqual", "VideoLevel", level.to_string()));
    }
    if !support.profiles.is_empty() {
        // `ConditionProcessor` splits an `EqualsAny` value on `|`.
        conditions.push(condition(
            "EqualsAny",
            "VideoProfile",
            support.profiles.join("|"),
        ));
    }
    CodecProfile {
        r#type: "Video",
        codec: join(codec_names(&support.codec)),
        conditions,
    }
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
    let embed = capabilities.subtitle_formats.iter().flat_map(|codec| {
        subtitle_names(codec).iter().map(|format| SubtitleProfile {
            format,
            method: "Embed",
        })
    });
    let external = capabilities
        .subtitle_formats
        .iter()
        .filter(|codec| capabilities.external_subtitles && EXTERNAL_SUBTITLES.contains(codec))
        .flat_map(|codec| {
            subtitle_names(codec).iter().map(|format| SubtitleProfile {
                format,
                method: "External",
            })
        });
    embed.chain(external).collect()
}

/// The server's names for a container.
fn container_names(container: &Container) -> String {
    let names: &[&str] = match container.as_str() {
        "matroska" => &["mkv"],
        "webm" => &["webm"],
        "mp4" => &["mp4", "m4v", "mov"],
        "mpegts" => &["ts", "mpegts", "m2ts"],
        other => return other.to_owned(),
    };
    names.join(",")
}

/// The server's names for an audio or video codec: FFmpeg's, as the model's.
fn codec_names(codec: &Codec) -> Vec<String> {
    vec![codec.as_str().to_owned()]
}

/// The server's names for a subtitle format: what its probe and its external-file scan report. Formats Current does not know get
/// no profile: the server could not match an unknown name to a stream anyway.
fn subtitle_names(codec: &Codec) -> &'static [&'static str] {
    match codec.as_str() {
        "subrip" => &["subrip", "srt"],
        "ass" => &["ass"],
        "ssa" => &["ssa"],
        "webvtt" => &["webvtt", "vtt"],
        "mov_text" => &["mov_text"],
        "hdmv_pgs_subtitle" => &["pgssub"],
        "dvd_subtitle" => &["dvdsub"],
        "dvb_subtitle" => &["dvbsub"],
        _ => &[],
    }
}

fn join(names: impl IntoIterator<Item = String>) -> String {
    let mut seen: Vec<String> = Vec::new();
    for name in names {
        if !seen.contains(&name) {
            seen.push(name);
        }
    }
    seen.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lists `current-mpv` reports for our Windows build (ADR 0019, "Engine capabilities"; `build/mpv-windows/check.py`).
    pub(crate) fn mpv_windows() -> EngineCapabilities {
        EngineCapabilities {
            containers: vec![
                Container::MATROSKA,
                Container::WEBM,
                Container::MP4,
                Container::MPEG_TS,
                Container::AVI,
                Container::OGG,
                Container::FLAC,
            ],
            video_codecs: [
                Codec::H264,
                Codec::HEVC,
                Codec::AV1,
                Codec::VP9,
                Codec::VP8,
                Codec::MPEG2,
                Codec::VC1,
            ]
            .into_iter()
            .map(VideoCodecSupport::any)
            .collect(),
            audio_codecs: vec![
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
            ],
            subtitle_formats: vec![
                Codec::ASS,
                Codec::SUBRIP,
                Codec::WEBVTT,
                Codec::PGS,
                Codec::DVD_SUBTITLE,
                Codec::DVB_SUBTITLE,
                Codec::MOV_TEXT,
            ],
            external_subtitles: true,
        }
    }

    /// Checked by hand against `DeviceProfile.cs` and its siblings in Jellyfin v12.2: property names in PascalCase, enums as their
    /// names (`DlnaProfileType.Video`, `MediaStreamProtocol.hls`, `EncodingContext.Streaming`, `SubtitleDeliveryMethod.Embed` and
    /// `External`), comma-separated lists, server spellings for containers and subtitle formats.
    const SAMPLE: &str = r#"{
      "Name": "Current",
      "MaxStreamingBitrate": 2147483647,
      "DirectPlayProfiles": [
        {"Container": "mkv", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "webm", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "mp4,m4v,mov", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
        {"Container": "ts,mpegts,m2ts", "Type": "Video", "VideoCodec": "h264,hevc,av1,vp9,vp8,mpeg2video,vc1", "AudioCodec": "aac,ac3,eac3,truehd,dts,flac,opus,vorbis,mp3,pcm_s16le,pcm_s24le"},
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
            .video_codecs
            .retain(|support| support.codec == Codec::H264);
        capabilities.audio_codecs = vec![Codec::AAC];
        let json = serde_json::to_value(device_profile(&capabilities, i32::MAX)).unwrap();
        let profiles = json["TranscodingProfiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0]["VideoCodec"], "h264");
        assert_eq!(profiles[0]["AudioCodec"], "aac");
    }

    #[test]
    fn no_profile_carries_an_empty_list() {
        let capabilities = EngineCapabilities {
            containers: vec![Container::MATROSKA, Container::MP4],
            video_codecs: vec![VideoCodecSupport::any(Codec::AV1)],
            audio_codecs: Vec::new(),
            subtitle_formats: vec![Codec::new("SomeFutureFormat")],
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
        capabilities.video_codecs = vec![VideoCodecSupport {
            codec: Codec::HEVC,
            max_bit_depth: Some(10),
            max_level: Some(153),
            profiles: vec!["main".into(), "main 10".into()],
        }];
        let json = serde_json::to_value(device_profile(&capabilities, i32::MAX)).unwrap();
        assert_eq!(
            json["CodecProfiles"],
            serde_json::json!([{
                "Type": "Video",
                "Codec": "hevc",
                "Conditions": [
                    {"Condition": "LessThanEqual", "Property": "VideoBitDepth", "Value": "10", "IsRequired": false},
                    {"Condition": "LessThanEqual", "Property": "VideoLevel", "Value": "153", "IsRequired": false},
                    {"Condition": "EqualsAny", "Property": "VideoProfile", "Value": "main|main 10", "IsRequired": false}
                ]
            }])
        );
    }
}
