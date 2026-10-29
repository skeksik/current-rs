//! What a platform's own player plays, from the facts its UI reports (ADR 0025, "Capabilities stay logic in the core"; ADR 0021's
//! way): the core asks the UI which decoders to look for, the UI answers how many it found of each, and the tables here turn the
//! answers into [`EngineCapabilities`] with what the device check found (W1, `docs/notes/windows-native-player.md`).
//!
//! Windows: the UI runs `CodecQuery.FindAllAsync(kind, CodecCategory.Decoder, subtype)` per subtype (W1 §3: it works from the
//! unpackaged app and lists the Store extensions; none is returned when an extension is missing). Subtypes are named by their
//! `Windows.Media.Core.CodecSubtypes` property (Microsoft Learn, "CodecSubtypes Class", ms.date 2017-02-08, read 2026-10-29); the UI
//! reads the property's value.

use current_model::{Codec, CodecSupport, Container, EngineCapabilities};
use current_playback::{EngineProfile, PlatformRules};

/// Which list a subtype is looked for in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodecKind {
    Video,
    Audio,
}

/// A decoder the UI is asked to look for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodecQuery {
    pub kind: CodecKind,
    /// A `CodecSubtypes` property name, for example `VideoFormatHevc`.
    pub subtype: String,
}

/// The UI's answer for one subtype: how many decoders `CodecQuery` returned. A subtype the core did not ask for is ignored.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodecAnswer {
    pub subtype: String,
    pub decoders: u32,
}

/// One row: the codec, the subtype that says Windows decodes it, and W1's limits.
struct Row {
    kind: CodecKind,
    subtype: &'static str,
    support: fn() -> CodecSupport,
}

/// Windows' decoders Current takes natively, with what W1 found (each row cites `docs/notes/windows-native-player.md`). TrueHD, DTS,
/// multichannel PCM and Opus beyond stereo are left out on purpose: W1 §5 found them failing natively even where a subtype exists.
const WINDOWS: &[Row] = &[
    // §2: H.264 plays; H.264 High 10 reports `FullySupported` and then fails with `DecodingError`: 8 bit only.
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatH264",
        support: || CodecSupport {
            max_bit_depth: Some(8),
            ..CodecSupport::any(Codec::H264)
        },
    },
    // §2, §3: HEVC Main and Main 10 play with the HEVC Video Extensions; without them `CodecQuery` returns none for HEVC.
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatHevc",
        support: || CodecSupport::any(Codec::HEVC),
    },
    // §2, §3: AV1 10 bit plays with the AV1 Video Extension; none without it.
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatAv1",
        support: || CodecSupport::any(Codec::AV1),
    },
    // §2, §3: VP9 (and VP9 Profile 2 HDR10+ in WebM) plays through the VP9 Video Extensions, listed as `VP80`, `VP90`.
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatVP90",
        support: || CodecSupport::any(Codec::VP9),
    },
    // VP8 shares the VP9 extension's listing (§3); its playback was not checked (not verified).
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatVP80",
        support: || CodecSupport::any(Codec::VP8),
    },
    // §3: the MPEG-2 Video Extension is listed by `CodecQuery`; playback of MPEG-2 was not checked (not verified).
    Row {
        kind: CodecKind::Video,
        subtype: "VideoFormatMpeg2",
        support: || CodecSupport::any(Codec::MPEG2),
    },
    // §5: AAC 2.0 and 5.1 play.
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatAac",
        support: || CodecSupport::any(Codec::AAC),
    },
    // §3, §5: AC-3 and E-AC-3 play through the Dolby decoder the PC maker installed; a PC without one has none.
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatDolbyAC3",
        support: || CodecSupport::any(Codec::AC3),
    },
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatDolbyDDPlus",
        support: || CodecSupport::any(Codec::EAC3),
    },
    // §5: FLAC 5.1 plays.
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatFlac",
        support: || CodecSupport::any(Codec::FLAC),
    },
    // §5: Opus 2.0 plays; Opus 5.1 in MKV fails (`UnsupportedEncoderProperties`): stereo only.
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatOpus",
        support: || CodecSupport {
            max_channels: Some(2),
            ..CodecSupport::any(Codec::OPUS)
        },
    },
    // §3: MP3 and ALAC are among the built-in decoders seen; their playback was not checked (not verified).
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatMP3",
        support: || CodecSupport::any(Codec::MP3),
    },
    Row {
        kind: CodecKind::Audio,
        subtype: "AudioFormatAlac",
        support: || CodecSupport::any(Codec::ALAC),
    },
];

/// What the UI on Windows is asked to look for, in the table's order.
pub fn windows_codec_queries() -> Vec<CodecQuery> {
    WINDOWS
        .iter()
        .map(|row| CodecQuery {
            kind: row.kind,
            subtype: row.subtype.to_owned(),
        })
        .collect()
}

/// Windows' player (Media Foundation in `MediaPlayerElement`) from the UI's answers: the codecs of the rows that found a decoder,
/// with W1's limits, the containers and subtitles W1 checked, and Windows' rules (departures 2, 3 and 8).
pub fn windows_engine(answers: &[CodecAnswer]) -> (EngineProfile, PlatformRules) {
    let found = |subtype: &str| {
        answers
            .iter()
            .any(|answer| answer.subtype == subtype && answer.decoders > 0)
    };
    let pick = |kind: CodecKind| -> Vec<CodecSupport> {
        WINDOWS
            .iter()
            .filter(|row| row.kind == kind && found(row.subtype))
            .map(|row| (row.support)())
            .collect()
    };
    let capabilities = EngineCapabilities {
        // §2: MKV, MP4, MPEG-TS and WebM played. MOV is not listed on its own; the server names MP4 and MOV files alike
        // `mov,mp4,m4a,3gp,3g2,mj2`, which matches MP4, so they go to the native route (a `.mov` file natively is not verified).
        containers: vec![
            Container::MKV,
            Container::MP4,
            Container::TS,
            Container::WEBM,
        ],
        video: pick(CodecKind::Video),
        audio: pick(CodecKind::Audio),
        // None until W5 (ADR 0025, feat/windows-native-subtitles): the Windows engine draws no subtitles yet, so the subtitle rule
        // sends a file with a subtitle selected to the app engine, PGS too (departure 3 waits for W5). W5 lists what §6 found:
        // SubRip arrives as timed text cues and PGS as image cues (SUBRIP, SRT, PGSSUB); ASS keeps only its static styling (choice
        // 1 sends it to the app engine), DVD subtitles showed no cue (not verified). With W5 the core also hands external subtitles
        // over as bytes (ADR 0025, "Subtitles on the native route"), and `external_subtitles` becomes true.
        subtitles: Vec::new(),
        external_subtitles: false,
    };
    // §7: HLS with fMP4 and TS segments plays through `AdaptiveMediaSource`.
    (
        EngineProfile::new(capabilities, true),
        PlatformRules::WINDOWS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(found: &[&str]) -> Vec<CodecAnswer> {
        windows_codec_queries()
            .into_iter()
            .map(|query| CodecAnswer {
                decoders: u32::from(found.contains(&query.subtype.as_str())),
                subtype: query.subtype,
            })
            .collect()
    }

    fn codecs(supports: &[CodecSupport]) -> Vec<&str> {
        supports
            .iter()
            .map(|support| support.codec.as_str())
            .collect()
    }

    #[test]
    fn every_row_is_asked_for_by_its_codec_subtypes_name() {
        let queries = windows_codec_queries();
        assert_eq!(queries.len(), WINDOWS.len());
        assert!(
            queries
                .iter()
                .all(|query| query.subtype.starts_with("VideoFormat")
                    == (query.kind == CodecKind::Video))
        );
        assert!(queries.contains(&CodecQuery {
            kind: CodecKind::Video,
            subtype: "VideoFormatHevc".into()
        }));
    }

    #[test]
    fn a_codec_is_native_only_when_windows_has_a_decoder_for_it() {
        // A PC without the Store extensions or a Dolby decoder: H.264, AAC and the built-in rest.
        let (bare, rules) = windows_engine(&answers(&[
            "VideoFormatH264",
            "AudioFormatAac",
            "AudioFormatFlac",
            "AudioFormatOpus",
        ]));
        assert_eq!(codecs(&bare.capabilities.video), ["h264"]);
        assert_eq!(codecs(&bare.capabilities.audio), ["aac", "flac", "opus"]);
        assert_eq!(rules, PlatformRules::WINDOWS);
        assert!(bare.hls);
        // With the HEVC and AV1 extensions and a Dolby decoder.
        let (full, _) = windows_engine(&answers(&[
            "VideoFormatH264",
            "VideoFormatHevc",
            "VideoFormatAv1",
            "AudioFormatAac",
            "AudioFormatDolbyAC3",
            "AudioFormatDolbyDDPlus",
        ]));
        assert_eq!(codecs(&full.capabilities.video), ["h264", "hevc", "av1"]);
        assert_eq!(codecs(&full.capabilities.audio), ["aac", "ac3", "eac3"]);
    }

    #[test]
    fn w1s_limits_hold() {
        let (engine, _) = windows_engine(&answers(&["VideoFormatH264", "AudioFormatOpus"]));
        assert_eq!(engine.capabilities.video[0].max_bit_depth, Some(8));
        assert_eq!(engine.capabilities.audio[0].max_channels, Some(2));
        // Never natively, whatever the answers: TrueHD, DTS.
        assert!(!engine.capabilities.decodes_audio(&Codec::TRUEHD));
        assert!(!engine.capabilities.decodes_audio(&Codec::DTS));
        // No subtitle natively until W5: a selected one goes to the app engine.
        assert!(engine.capabilities.subtitles.is_empty());
        assert!(!engine.capabilities.external_subtitles);
    }

    #[test]
    fn answers_the_core_did_not_ask_for_or_with_no_decoder_are_ignored() {
        let (engine, _) = windows_engine(&[
            CodecAnswer {
                subtype: "AudioFormatDts".into(),
                decoders: 3,
            },
            CodecAnswer {
                subtype: "SomethingNew".into(),
                decoders: 1,
            },
            CodecAnswer {
                subtype: "VideoFormatHevc".into(),
                decoders: 0,
            },
        ]);
        assert!(engine.capabilities.video.is_empty());
        assert!(engine.capabilities.audio.is_empty());
    }
}
