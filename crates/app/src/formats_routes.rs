//! Settings' "Supported formats" page per route (ADR 0025, W6; ADR 0021, "Per route"): with Windows' own player beside the app
//! engine, each row says which one plays the format on this PC, or that the server converts it, and why.
//!
//! The answer is the player's own: each row's route comes from the core's route check (`current_playback::route::local_routes`) run
//! on a typical file for that row (below, per tab), with the native engine built from the UI's CodecQuery answers exactly as the player
//! builds it ([`windows_engine`]) and the app engine's reported capabilities. So Windows' rules apply as they do when playing: Dolby
//! Vision plays in the app engine first (departure 2), PQ video on an SDR display too (departure 8), PGS stays native (departure 3),
//! and a subtitle that needs rendering, or one Windows' player cannot read as a file, goes to the app engine (W5). Rows neither
//! engine plays keep what the server does with them (ADR 0021).

use current_model::{
    AudioFacts, Codec, MediaSource, MediaStream, Route, RouteReason, StreamKind, SubtitleChoice,
    SubtitleFormat, TrackChoice, VideoFacts, VideoRangeType,
};
use current_playback::route::local_routes;
use current_playback::{DisplayKind, EnginePreference, EngineProfile, Engines, RouteContext};

use crate::formats::{
    AUDIO, CONTAINERS, FormatAction, FormatRow, FormatSupport, FormatTabKind, HDR, SUBTITLES,
    SupportedFormats, VIDEO, notes,
};
use crate::player::{CodecAnswer, DisplayFacts, windows_engine};

/// Windows' own player beside the app engine: the UI's answers to [`crate::player::windows_codec_queries`], and whether the "Player"
/// setting is "Always use the app engine".
#[derive(Clone, Copy, Debug)]
pub struct NativeFacts<'a> {
    pub answers: &'a [CodecAnswer],
    pub always_app_engine: bool,
}

/// A Store extension Windows' player needs for a codec: the CodecQuery subtype that finds its decoder (as
/// [`crate::player::windows_codec_queries`] names it), its name and its Store product id. The ids and names are the Microsoft Store's
/// product pages (`https://apps.microsoft.com/detail/<id>`, their titles read 2026-10-29); W1 opened the HEVC and AV1 links from the
/// unpackaged app (docs/notes/windows-native-player.md §3). HEVC is the paid "HEVC Video Extensions": W1 found that the device maker's
/// package (9N4WGH0Z6VHQ) cannot be installed by a user. VP8 is decoded by the VP9 extension (W1 §3: it lists `VP80` and `VP90`).
/// MPEG-2: Microsoft's Supported codecs page says MPEG-1/2 come "with install of the optional Windows DVD Player app" (ms.date
/// 2026-05-12, read 2026-10-29), and W1 §3 saw CodecQuery list an `MPEG2VideoExtension`; that the Store's MPEG-2 Video Extension is
/// the one Windows' player then uses is not verified, and its note says so.
const STORE_EXTENSIONS: &[(Codec, &str, &str, &str, bool)] = &[
    (
        Codec::HEVC,
        "VideoFormatHevc",
        "HEVC Video Extensions",
        "9NMZLZ57R3T7",
        true,
    ),
    (
        Codec::AV1,
        "VideoFormatAv1",
        "AV1 Video Extension",
        "9MVZQVXJBQ9V",
        true,
    ),
    (
        Codec::VP9,
        "VideoFormatVP90",
        "VP9 Video Extensions",
        "9N4D0MSMP0PT",
        true,
    ),
    (
        Codec::VP8,
        "VideoFormatVP80",
        "VP9 Video Extensions",
        "9N4D0MSMP0PT",
        true,
    ),
    (
        Codec::MPEG2,
        "VideoFormatMpeg2",
        "MPEG-2 Video Extension",
        "9N95Q1ZZPMH4",
        false,
    ),
];

/// What W1 did not play in Windows' player (docs/notes/windows-native-player.md §2, §5, §6, §4): a row the route check sends there
/// says "Not verified on a device".
const NOT_CHECKED_NATIVELY: &[&str] = &[
    "VP8",
    "MPEG-2",
    "MP3",
    "ALAC",
    "PCM, 16-bit",
    "PCM, 24-bit",
    "QuickTime (MOV)",
    "WebVTT",
    "HDR10+",
    "Dolby Vision profile 7",
];

const NOT_VERIFIED: &str = "Not verified on a device";

/// Applies the routes to `page`, built for the app engine (`app`) as before. Without the app engine's answer the page stays as it is.
pub fn apply_routes(
    page: &mut SupportedFormats,
    app: &current_model::EngineCapabilities,
    native: NativeFacts<'_>,
    display: Option<&DisplayFacts>,
) {
    let (profile, rules) = windows_engine(native.answers);
    let engines = Engines {
        native: Some(profile),
        app: EngineProfile::new(app.clone(), true),
        remux: None,
        rules,
    };
    let context = RouteContext {
        display: match display {
            Some(display) if display.hdr => DisplayKind::Hdr,
            Some(_) => DisplayKind::Sdr,
            None => DisplayKind::Unknown,
        },
        preference: if native.always_app_engine {
            EnginePreference::AlwaysAppEngine
        } else {
            EnginePreference::Automatic
        },
        app_engine_chosen: false,
        speed_not_supported: false,
    };
    if native.always_app_engine {
        page.description.push_str(
            " Settings is set to always use the app engine, so nothing plays in Windows' player.",
        );
    }
    for tab in &mut page.tabs {
        for row in &mut tab.rows {
            let Some(file) = typical_file(tab.kind, &row.name) else {
                continue;
            };
            let choices = local_routes(&file.source, file.tracks, &engines, context, &[]);
            let Some(first) = choices.first() else {
                continue;
            };
            match first.route {
                Route::Native => native_row(row, tab.kind, display, &file),
                Route::AppEngine => {
                    app_row(row, &first.reasons, tab.kind, &file, native.answers);
                }
                _ => {}
            }
            // A text subtitle as a separate file may go elsewhere than inside the file (W5: WebVTT only as a file).
            if let Some(as_file) = &file.as_file {
                let elsewhere =
                    local_routes(&as_file.source, as_file.tracks, &engines, context, &[]);
                if let Some(other) = elsewhere.first().map(|choice| &choice.route)
                    && *other != first.route
                {
                    let word = match other {
                        Route::Native => "Windows' player",
                        _ => "the app engine",
                    };
                    row.notes = notes(&[
                        &row.notes,
                        &format!("As a separate file it plays in {word}"),
                    ]);
                }
            }
        }
    }
}

fn native_row(
    row: &mut FormatRow,
    kind: FormatTabKind,
    display: Option<&DisplayFacts>,
    file: &TypicalFile,
) {
    let display_note = match (kind, display) {
        (FormatTabKind::Hdr, Some(display)) if display.hdr => {
            "In HDR, on the display this window is on, which is in HDR mode"
        }
        (FormatTabKind::Hdr, Some(_)) => {
            "As it is on this SDR display (HLG is made to look right on SDR)"
        }
        (FormatTabKind::Hdr, None) => "HDR or SDR, depending on the display",
        _ => "",
    };
    let dolby = if file.dolby_audio {
        "Through the Dolby decoder this PC has"
    } else {
        ""
    };
    let unchecked = if NOT_CHECKED_NATIVELY.contains(&row.name.as_str()) {
        NOT_VERIFIED
    } else {
        ""
    };
    row.support = FormatSupport::Native;
    row.support_label = match (kind, display) {
        (FormatTabKind::Hdr, Some(display)) if display.hdr => "Windows' player, HDR".to_owned(),
        _ => FormatSupport::Native.label().to_owned(),
    };
    row.notes = notes(&[display_note, dolby, unchecked]);
    row.action = None;
}

fn app_row(
    row: &mut FormatRow,
    reasons: &[RouteReason],
    kind: FormatTabKind,
    file: &TypicalFile,
    answers: &[CodecAnswer],
) {
    row.support_label = match row.support {
        FormatSupport::Hardware => "App engine (hardware)",
        FormatSupport::Software if kind == FormatTabKind::Video => "App engine (software)",
        FormatSupport::Hdr => "App engine, HDR",
        FormatSupport::ToneMapped => "App engine, tone-mapped",
        FormatSupport::Software | FormatSupport::Plays => "App engine",
        // The server's rows stay as they are.
        _ => return,
    }
    .to_owned();
    let mut why: Vec<String> = Vec::new();
    for reason in reasons {
        let words = if *reason == RouteReason::DOLBY_VISION_ON_APP_ENGINE {
            "Dolby Vision plays in the app engine first"
        } else if *reason == RouteReason::HDR_ON_SDR_DISPLAY {
            "On this SDR display it plays in the app engine, which tone-maps it"
        } else if *reason == RouteReason::SUBTITLE_NEEDS_RENDERING {
            "Windows' player can't draw it"
        } else {
            continue;
        };
        why.push(words.to_owned());
    }
    // What Windows' player lacks: a Store extension it can install, the PC maker's Dolby decoder, or it does not play the format.
    if reasons.contains(&RouteReason::NATIVE_LACKS) {
        if let Some((name, id, verified)) = missing_extension(file, answers) {
            let note = if verified {
                format!("Windows' player needs the {name}")
            } else {
                format!(
                    "Windows' player needs the {name} (not verified: Windows' DVD Player app may bring it too)"
                )
            };
            why.push(note);
            row.action = Some(FormatAction {
                label: format!("Install {name}"),
                uri: format!("ms-windows-store://pdp/?ProductId={id}"),
            });
        } else if file.dolby_audio {
            why.push("Windows' player has no Dolby decoder on this PC: it comes with the PC maker's software".to_owned());
        } else {
            why.push("Windows' player can't play it".to_owned());
        }
    }
    let why: Vec<&str> = why.iter().map(String::as_str).collect();
    row.notes = notes(&[&notes(&why), &row.notes]);
}

/// The Store extension for the typical file's video codec when CodecQuery found no decoder for it: its name, id and whether the
/// link is known to be the one (see [`STORE_EXTENSIONS`]).
fn missing_extension(
    file: &TypicalFile,
    answers: &[CodecAnswer],
) -> Option<(&'static str, &'static str, bool)> {
    let codec = file.video_codec.as_ref()?;
    let (_, subtype, name, id, verified) =
        STORE_EXTENSIONS.iter().find(|(known, ..)| known == codec)?;
    let found = answers
        .iter()
        .any(|answer| answer.subtype == *subtype && answer.decoders > 0);
    (!found).then_some((*name, *id, *verified))
}

/// A file for a row's route check, with what the notes need to know about it.
struct TypicalFile {
    source: MediaSource,
    tracks: TrackChoice,
    /// The row is about this video codec (the Video tab).
    video_codec: Option<Codec>,
    /// The row is about AC-3 or E-AC-3 (Windows decodes them only with the PC maker's Dolby decoder, W1 §3).
    dolby_audio: bool,
    /// The same subtitle as a separate file (text subtitles).
    as_file: Option<Box<TypicalFile>>,
}

fn stream(index: u32, kind: StreamKind, codec: &str) -> MediaStream {
    MediaStream {
        index,
        kind,
        codec: Some(Codec::new(codec.to_owned())),
        profile: None,
        level: None,
        language: None,
        title: None,
        display_title: None,
        is_default: true,
        is_forced: false,
        is_hearing_impaired: false,
        is_external: false,
        bitrate: None,
        video: None,
        audio: None,
        subtitle_delivery: None,
    }
}

fn video(
    codec: &str,
    bit_depth: u32,
    range: VideoRangeType,
    dv_profile: Option<u32>,
) -> MediaStream {
    MediaStream {
        video: Some(VideoFacts {
            width: Some(1920),
            height: Some(1080),
            bit_depth: Some(bit_depth),
            frame_rate: Some(24.0),
            reference_frame_rate: None,
            range: None,
            range_type: Some(range),
            dv_profile,
        }),
        ..stream(0, StreamKind::VIDEO, codec)
    }
}

fn audio(codec: &str, channels: u32) -> MediaStream {
    MediaStream {
        audio: Some(AudioFacts {
            channels: Some(channels),
            layout: None,
            sample_rate: Some(48_000),
        }),
        ..stream(1, StreamKind::AUDIO, codec)
    }
}

fn source(container: &str, streams: Vec<MediaStream>) -> MediaSource {
    MediaSource {
        id: "typical".into(),
        name: None,
        container: Some(container.to_owned()),
        size: None,
        bitrate: Some(8_000_000),
        duration: None,
        streams,
        default_audio: Some(1),
        default_subtitle: None,
        has_segments: false,
        is_remote: false,
    }
}

/// The typical file's tracks: its audio stream, no subtitle.
const PLAIN: TrackChoice = TrackChoice {
    audio: Some(1),
    subtitle: SubtitleChoice::Off,
};

/// The file a row is checked with. Per tab, a typical file that differs from an everyday one only in the row's format:
/// - Video: MKV, the codec at 8 bit, SDR, 1080p, with AAC stereo.
/// - Audio: MKV, H.264 8-bit SDR, with the codec; six channels for AC-3, E-AC-3, TrueHD and DTS (and Dolby Atmos, as E-AC-3, the way
///   W1's file had it), two for the rest.
/// - Containers: the container with H.264 8-bit SDR and AAC stereo.
/// - Subtitles: MKV, H.264 and AAC, with the subtitle inside the file (its first spelling); for a text format, also as a separate
///   file.
/// - HDR: MKV, HEVC 10-bit with the range (HDR10, HDR10+, HLG; Dolby Vision profiles 5, 7 and 8 with their usual base layers), and AAC
///   stereo.
///
/// Rows of formats the tables do not know (open values) are not checked: they keep the app engine's answer.
fn typical_file(kind: FormatTabKind, name: &str) -> Option<TypicalFile> {
    let h264 = || video("h264", 8, VideoRangeType::SDR, None);
    let aac = || audio("aac", 2);
    let file = |source: MediaSource| TypicalFile {
        tracks: PLAIN,
        source,
        video_codec: None,
        dolby_audio: false,
        as_file: None,
    };
    match kind {
        FormatTabKind::Video => {
            let (codec, ..) = VIDEO.iter().find(|(_, known, _)| *known == name)?;
            let mut typical = file(source(
                "mkv",
                vec![video(codec.as_str(), 8, VideoRangeType::SDR, None), aac()],
            ));
            typical.video_codec = Some(codec.clone());
            Some(typical)
        }
        FormatTabKind::Audio => {
            let codec = if name == "Dolby Atmos" {
                Codec::EAC3
            } else {
                AUDIO.iter().find(|(_, known)| *known == name)?.0.clone()
            };
            let surround = [Codec::AC3, Codec::EAC3, Codec::TRUEHD, Codec::DTS].contains(&codec);
            let mut typical = file(source(
                "mkv",
                vec![h264(), audio(codec.as_str(), if surround { 6 } else { 2 })],
            ));
            typical.dolby_audio = codec == Codec::AC3 || codec == Codec::EAC3;
            Some(typical)
        }
        FormatTabKind::Containers => {
            let (container, _) = CONTAINERS.iter().find(|(_, known)| *known == name)?;
            Some(file(source(container.as_str(), vec![h264(), aac()])))
        }
        FormatTabKind::Subtitles => {
            let (spellings, ..) = SUBTITLES.iter().find(|(_, known, _)| *known == name)?;
            let format: &SubtitleFormat = spellings.first()?;
            let with = |external: bool| {
                let mut subtitle = stream(2, StreamKind::SUBTITLE, format.as_str());
                subtitle.is_external = external;
                let mut typical = file(source("mkv", vec![h264(), aac(), subtitle]));
                typical.tracks.subtitle = SubtitleChoice::Stream(2);
                typical
            };
            let mut typical = with(false);
            let text = !name.contains("PGS") && !name.contains("VobSub") && !name.contains("DVB");
            if text {
                typical.as_file = Some(Box::new(with(true)));
            }
            Some(typical)
        }
        FormatTabKind::Hdr => {
            let (range, dv) = match name {
                "HDR10" => (VideoRangeType::HDR10, None),
                "HDR10+" => (VideoRangeType::HDR10_PLUS, None),
                "HLG" => (VideoRangeType::HLG, None),
                "Dolby Vision profile 5" => (VideoRangeType::DOVI, Some(5)),
                "Dolby Vision profile 7" => (VideoRangeType::DOVI_WITH_EL, Some(7)),
                "Dolby Vision profile 8" => (VideoRangeType::DOVI_WITH_HDR10, Some(8)),
                _ => return None,
            };
            HDR.iter().find(|(known, _)| *known == name)?;
            Some(file(source(
                "mkv",
                vec![video("hevc", 10, range, dv), aac()],
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use current_model::{CodecSupport, Container, EngineCapabilities};

    use super::*;
    use crate::formats::{FormatTab, supported_formats};
    use crate::player::{EngineReport, windows_codec_queries};

    /// An app engine shaped like our libmpv build (as `formats`' own tests have it).
    fn app() -> EngineCapabilities {
        EngineCapabilities {
            containers: CONTAINERS
                .iter()
                .map(|(c, _)| c.clone())
                .collect::<Vec<Container>>(),
            video: VIDEO
                .iter()
                .filter(|(codec, ..)| *codec != Codec::MPEG4)
                .map(|(codec, ..)| CodecSupport::any(codec.clone()))
                .collect(),
            audio: AUDIO
                .iter()
                .map(|(codec, _)| CodecSupport::any(codec.clone()))
                .collect(),
            subtitles: SUBTITLES
                .iter()
                .flat_map(|(formats, ..)| formats.iter().cloned())
                .collect(),
            external_subtitles: true,
        }
    }

    /// CodecQuery's answers: one decoder for each subtype named, none for the rest.
    fn answers(found: &[&str]) -> Vec<CodecAnswer> {
        windows_codec_queries()
            .into_iter()
            .map(|query| CodecAnswer {
                decoders: u32::from(found.contains(&query.subtype.as_str())),
                subtype: query.subtype,
            })
            .collect()
    }

    /// A PC with the HEVC, AV1 and VP9 extensions and a Dolby decoder.
    const FULL: &[&str] = &[
        "VideoFormatH264",
        "VideoFormatHevc",
        "VideoFormatAv1",
        "VideoFormatVP90",
        "VideoFormatVP80",
        "AudioFormatAac",
        "AudioFormatDolbyAC3",
        "AudioFormatDolbyDDPlus",
        "AudioFormatFlac",
        "AudioFormatOpus",
    ];

    fn display(hdr: bool) -> DisplayFacts {
        DisplayFacts {
            hdr,
            peak_nits: 400.0,
            sdr_white_nits: 200.0,
            refresh_hz: 60.0,
        }
    }

    fn page(found: &[&str], hdr: Option<bool>, always: bool) -> SupportedFormats {
        let caps = app();
        let display = hdr.map(display);
        let mut page =
            supported_formats(&EngineReport::Ready(caps.clone()), None, display.as_ref());
        let answers = answers(found);
        apply_routes(
            &mut page,
            &caps,
            NativeFacts {
                answers: &answers,
                always_app_engine: always,
            },
            display.as_ref(),
        );
        page
    }

    fn tab(page: &SupportedFormats, kind: FormatTabKind) -> &FormatTab {
        page.tabs
            .iter()
            .find(|tab| tab.kind == kind)
            .expect("the tab")
    }

    fn row<'a>(page: &'a SupportedFormats, kind: FormatTabKind, name: &str) -> &'a FormatRow {
        tab(page, kind)
            .rows
            .iter()
            .find(|row| row.name == name)
            .expect(name)
    }

    #[test]
    fn a_codec_windows_decodes_plays_in_windows_player() {
        let page = page(FULL, Some(true), false);
        for name in ["H.264 (AVC)", "HEVC (H.265)", "AV1", "VP9"] {
            let hevc = row(&page, FormatTabKind::Video, name);
            assert_eq!(hevc.support, FormatSupport::Native, "{name}");
            assert_eq!(hevc.support_label, "Windows' player");
            assert_eq!(hevc.action, None);
            assert_eq!(hevc.notes, "", "{name}");
        }
        // What W1 did not play natively says so.
        let vp8 = row(&page, FormatTabKind::Video, "VP8");
        assert_eq!(vp8.support, FormatSupport::Native);
        assert_eq!(vp8.notes, "Not verified on a device");
        // Windows' player lacks VC-1 (not in its table): the app engine, as today's row has it.
        let vc1 = row(&page, FormatTabKind::Video, "VC-1");
        assert_eq!(vc1.support_label, "App engine");
        assert!(
            vc1.notes.starts_with("Windows' player can't play it"),
            "{}",
            vc1.notes
        );
        // MPEG-4 Part 2: neither engine; the server's row is unchanged.
        let mpeg4 = row(&page, FormatTabKind::Video, "MPEG-4 Part 2");
        assert_eq!(mpeg4.support, FormatSupport::ServerConverts);
    }

    #[test]
    fn a_missing_store_extension_names_it_and_links_its_store_page() {
        let page = page(&["VideoFormatH264", "AudioFormatAac"], Some(true), false);
        let hevc = row(&page, FormatTabKind::Video, "HEVC (H.265)");
        assert_eq!(hevc.support_label, "App engine");
        assert_eq!(
            hevc.notes,
            "Windows' player needs the HEVC Video Extensions"
        );
        assert_eq!(
            hevc.action,
            Some(FormatAction {
                label: "Install HEVC Video Extensions".into(),
                uri: "ms-windows-store://pdp/?ProductId=9NMZLZ57R3T7".into(),
            })
        );
        let av1 = row(&page, FormatTabKind::Video, "AV1");
        assert_eq!(
            av1.action.as_ref().map(|action| action.uri.as_str()),
            Some("ms-windows-store://pdp/?ProductId=9MVZQVXJBQ9V")
        );
        let vp9 = row(&page, FormatTabKind::Video, "VP9");
        assert_eq!(
            vp9.action.as_ref().map(|action| action.uri.as_str()),
            Some("ms-windows-store://pdp/?ProductId=9N4D0MSMP0PT")
        );
        // MPEG-2's link is not known to be the one: its note says so.
        let mpeg2 = row(&page, FormatTabKind::Video, "MPEG-2");
        assert!(mpeg2.notes.contains("not verified"), "{}", mpeg2.notes);
        assert!(mpeg2.action.is_some());
        // Every link is a Store product page.
        for row in page.tabs.iter().flat_map(|tab| &tab.rows) {
            if let Some(action) = &row.action {
                let id = action
                    .uri
                    .strip_prefix("ms-windows-store://pdp/?ProductId=")
                    .expect("a Store link");
                assert!(
                    id.len() == 12
                        && id
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                );
            }
        }
    }

    #[test]
    fn dolby_audio_plays_natively_only_with_the_pc_makers_decoder() {
        let with = page(FULL, Some(true), false);
        let ac3 = row(&with, FormatTabKind::Audio, "Dolby Digital (AC-3)");
        assert_eq!(ac3.support, FormatSupport::Native);
        assert_eq!(ac3.notes, "Through the Dolby decoder this PC has");
        let without = page(&["VideoFormatH264", "AudioFormatAac"], Some(true), false);
        let ac3 = row(&without, FormatTabKind::Audio, "Dolby Digital (AC-3)");
        assert_eq!(ac3.support_label, "App engine");
        assert!(
            ac3.notes
                .starts_with("Windows' player has no Dolby decoder on this PC"),
            "{}",
            ac3.notes
        );
        assert_eq!(ac3.action, None);
        // TrueHD and DTS never natively (W1 §5).
        for name in ["Dolby TrueHD", "DTS"] {
            assert_eq!(
                row(&with, FormatTabKind::Audio, name).support_label,
                "App engine",
                "{name}"
            );
        }
    }

    #[test]
    fn hdr_follows_departures_2_and_8() {
        let hdr = page(FULL, Some(true), false);
        let hdr10 = row(&hdr, FormatTabKind::Hdr, "HDR10");
        assert_eq!(hdr10.support_label, "Windows' player, HDR");
        for name in ["Dolby Vision profile 5", "Dolby Vision profile 8"] {
            let dv = row(&hdr, FormatTabKind::Hdr, name);
            assert_eq!(dv.support_label, "App engine, HDR", "{name}");
            assert!(
                dv.notes
                    .starts_with("Dolby Vision plays in the app engine first"),
                "{}",
                dv.notes
            );
        }
        // On an SDR display PQ video plays in the app engine, which tone-maps it; HLG stays native.
        let sdr = page(FULL, Some(false), false);
        let hdr10 = row(&sdr, FormatTabKind::Hdr, "HDR10");
        assert_eq!(hdr10.support_label, "App engine, tone-mapped");
        assert!(
            hdr10
                .notes
                .starts_with("On this SDR display it plays in the app engine"),
            "{}",
            hdr10.notes
        );
        let hlg = row(&sdr, FormatTabKind::Hdr, "HLG");
        assert_eq!(hlg.support, FormatSupport::Native);
        assert_eq!(hlg.support_label, "Windows' player");
    }

    #[test]
    fn subtitles_follow_w5s_rules() {
        let page = page(FULL, Some(true), false);
        assert_eq!(
            row(&page, FormatTabKind::Subtitles, "SubRip (SRT)").support,
            FormatSupport::Native
        );
        assert_eq!(
            row(&page, FormatTabKind::Subtitles, "PGS (Blu-ray)").support,
            FormatSupport::Native
        );
        let ass = row(
            &page,
            FormatTabKind::Subtitles,
            "SubStation Alpha (ASS, SSA)",
        );
        assert_eq!(ass.support_label, "App engine");
        assert!(
            ass.notes.starts_with("Windows' player can't draw it"),
            "{}",
            ass.notes
        );
        assert_eq!(
            row(&page, FormatTabKind::Subtitles, "VobSub (DVD)").support_label,
            "App engine"
        );
        // WebVTT: the app engine inside a file, Windows' player as a file.
        let vtt = row(&page, FormatTabKind::Subtitles, "WebVTT");
        assert_eq!(vtt.support_label, "App engine");
        assert!(
            vtt.notes
                .ends_with("As a separate file it plays in Windows' player"),
            "{}",
            vtt.notes
        );
        // Containers: MKV natively; AVI, which Windows' player does not list, in the app engine.
        assert_eq!(
            row(&page, FormatTabKind::Containers, "Matroska (MKV)").support,
            FormatSupport::Native
        );
        assert_eq!(
            row(&page, FormatTabKind::Containers, "AVI").support_label,
            "App engine"
        );
    }

    #[test]
    fn the_setting_always_app_engine_keeps_every_row_off_windows_player() {
        let page = page(FULL, Some(true), true);
        assert!(
            page.description
                .ends_with("so nothing plays in Windows' player.")
        );
        assert!(
            page.tabs
                .iter()
                .flat_map(|tab| &tab.rows)
                .all(|row| row.support != FormatSupport::Native)
        );
        assert_eq!(
            row(&page, FormatTabKind::Video, "HEVC (H.265)").support_label,
            "App engine"
        );
    }

    #[test]
    fn the_page_names_no_route_without_the_native_facts() {
        // What every platform without Windows' player shows: ADR 0021's page, untouched.
        let caps = app();
        let page = supported_formats(&EngineReport::Ready(caps), None, Some(&display(true)));
        assert!(
            page.tabs
                .iter()
                .flat_map(|tab| &tab.rows)
                .all(|row| row.support != FormatSupport::Native && row.action.is_none())
        );
        assert_eq!(
            row(&page, FormatTabKind::Video, "HEVC (H.265)").support_label,
            "Plays"
        );
    }
}
