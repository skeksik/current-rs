//! The playback info panel's lines (ADR 0019, "Playback information"): the route, its reasons and notes, the source and its
//! streams, the server's transcode, the engine's output, the display the window is on and the panel's sizes. No URL: the
//! information holds none. The sizes are measured by the platform ([`PanelSizes`]); numbers with decimals use the UI's decimal
//! separator ([`NumberFormat`]), as the Windows app's .NET formats did before these lines moved here.

use current_model::{Route, StreamProtocol};

use super::PlaybackInfo;
use super::backend::DisplayFacts;
use super::labels::{NumberFormat, bitrate_label};
use super::words::{plan_note_text, reason_text, route_name};

/// What the platform measures for the panel's last line: the swap chain panel's size in physical pixels, the view's and the
/// content's sizes in physical pixels (device-independent size × the window's scale, so fractional), and the window's client size
/// in pixels. The content's and the window's are left out when the platform has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelSizes {
    pub composition_width: u32,
    pub composition_height: u32,
    pub view_width: f64,
    pub view_height: f64,
    pub content: Option<(f64, f64)>,
    pub window: Option<(i32, i32)>,
}

/// The panel's lines; an empty line separates the groups. The facts not known are left out of their line, and so are empty ones.
pub fn info_panel_lines(
    info: &PlaybackInfo,
    display: Option<&DisplayFacts>,
    panel: &PanelSizes,
    format: &NumberFormat,
) -> Vec<String> {
    let mut lines = Vec::new();
    let details = info.details.as_ref();
    let plan = details.and_then(|details| details.plan.as_ref());
    if let Some(plan) = plan {
        let (protocol, container) = match &plan.route {
            Route::DirectPlay => (None, None),
            Route::ServerStream {
                protocol,
                container,
                ..
            } => (
                // The Windows app wrote the bridge's enum name upper-cased.
                Some(match protocol {
                    StreamProtocol::Hls => "HLS",
                    StreamProtocol::Progressive => "PROGRESSIVE",
                }),
                container.as_deref(),
            ),
        };
        lines.push(join([
            Some(route_name(plan.route.label()).to_owned()),
            protocol.map(str::to_owned),
            container.map(str::to_owned),
        ]));
        for reason in &plan.reasons {
            lines.push(format!("• {}", reason_text(reason.as_str())));
        }
    }
    for note in details
        .map(|details| details.notes.as_slice())
        .unwrap_or_default()
    {
        lines.push(format!("• {}", plan_note_text(note, format)));
    }
    if let Some(source) = plan.map(|plan| &plan.source) {
        lines.push(String::new());
        lines.push(join([
            Some("Source".to_owned()),
            source.container.clone(),
            source.bitrate.map(|bits| bitrate_label(bits, format)),
            source.size.map(|bytes| {
                // Bytes to GB as a double, as the Windows app divided them.
                let gigabytes = bytes as f64 / 1_000_000_000.0;
                format!("{} GB", decimals(Digits::double(gigabytes), 1, 1, format))
            }),
        ]));
        for stream in &source.streams {
            let video = stream.video.as_ref();
            let audio = stream.audio.as_ref();
            lines.push(join([
                Some(format!("#{} {}", stream.index, stream.kind.as_str())),
                stream.codec.as_ref().map(|codec| codec.as_str().to_owned()),
                stream.profile.clone(),
                size(
                    video.and_then(|video| video.width),
                    video.and_then(|video| video.height),
                ),
                video
                    .and_then(|video| video.frame_rate)
                    .map(|fps| format!("{} fps", decimals(Digits::single(fps), 3, 0, format))),
                video
                    .and_then(|video| video.bit_depth)
                    .map(|depth| format!("{depth}-bit")),
                video
                    .and_then(|video| video.range_type.as_ref())
                    .map(|range| range.as_str().to_owned()),
                audio
                    .and_then(|audio| audio.channels)
                    .map(|channels| format!("{channels} ch")),
                stream.language.clone(),
                stream
                    .display_title
                    .clone()
                    .or_else(|| stream.title.clone()),
                stream.is_external.then(|| "external".to_owned()),
            ]));
        }
    }
    if let Some(t) = details.and_then(|details| details.transcoding.as_ref()) {
        let codec = |codec: &Option<current_model::Codec>| {
            codec
                .as_ref()
                .map(|codec| codec.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        lines.push(String::new());
        lines.push(join([
            Some("Server".to_owned()),
            Some(if t.video_direct {
                "video copied".to_owned()
            } else {
                format!("video to {}", codec(&t.video_codec))
            }),
            Some(if t.audio_direct {
                "audio copied".to_owned()
            } else {
                format!("audio to {}", codec(&t.audio_codec))
            }),
            t.container.clone(),
            t.bitrate.map(|bits| bitrate_label(bits, format)),
            size(t.width, t.height),
            t.frame_rate
                .map(|fps| format!("{} fps", decimals(Digits::single(fps), 3, 0, format))),
            t.hardware_acceleration.clone(),
            t.completion
                .map(|done| format!("{}% done", decimals(Digits::double(done), 0, 0, format))),
        ]));
        for reason in &t.reasons {
            lines.push(format!("• {}", reason_text(reason.as_str())));
        }
    }
    let engine = &info.engine;
    let video = engine.video.as_ref();
    lines.push(String::new());
    lines.push(join([
        Some("Player".to_owned()),
        Some(match engine.hardware_decoder.as_deref() {
            Some(decoder) if !decoder.is_empty() => format!("decoder {decoder}"),
            _ => "software decoder".to_owned(),
        }),
        video.map(|video| format!("{}×{}", video.width, video.height)),
        video.and_then(|video| video.pixel_format.clone()),
        video.and_then(|video| video.primaries.clone()),
        video.and_then(|video| video.transfer.clone()),
    ]));
    if let Some(display) = display {
        lines.push(join([
            Some("Display".to_owned()),
            Some(if display.hdr { "HDR" } else { "SDR" }.to_owned()),
            (display.peak_nits > 0.0).then(|| {
                format!(
                    "peak {} nits",
                    decimals(Digits::single(display.peak_nits), 0, 0, format)
                )
            }),
            (display.sdr_white_nits > 0.0).then(|| {
                format!(
                    "SDR white {} nits",
                    decimals(Digits::single(display.sdr_white_nits), 0, 0, format)
                )
            }),
            (display.refresh_hz > 0.0).then(|| {
                format!(
                    "{} Hz",
                    decimals(Digits::double(display.refresh_hz), 3, 0, format)
                )
            }),
        ]));
    }
    let two = |value: f64| decimals(Digits::double(value), 2, 0, format);
    lines.push(join([
        Some("Panel".to_owned()),
        Some(format!(
            "{}×{}",
            panel.composition_width, panel.composition_height
        )),
        Some(format!(
            "view {}×{}",
            two(panel.view_width),
            two(panel.view_height)
        )),
        panel
            .content
            .map(|(width, height)| format!("content {}×{}", two(width), two(height))),
        panel
            .window
            .map(|(width, height)| format!("window {width}×{height}")),
    ]));
    lines
}

/// The parts known and not empty, joined by " · ".
fn join<const N: usize>(parts: [Option<String>; N]) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// "1920×1080" when both are known.
fn size(width: Option<u32>, height: Option<u32>) -> Option<String> {
    Some(format!("{}×{}", width?, height?))
}

/// A number's first significant digits, as .NET takes them before applying a custom numeric format: 15 for a `double` and 7 for a
/// `float` (dotnet/runtime main, `Double.cs` and `Single.cs` `MaxPrecisionCustomFormat => 15` and `=> 7`, `Number.Formatting.cs`
/// `FormatFloat`, read 2026-10-27). .NET makes them with Grisu3 or Dragon4, correctly rounded, an exact tie to even
/// (`Number.Dragon4.cs`, read 2026-10-27); Rust's `{:.Ne}` formatting is correctly rounded too, ties to even (checked on the
/// workspace's toolchain: 2.5 → "2e0", 3.5 → "4e0").
#[derive(Debug, PartialEq)]
struct Digits {
    negative: bool,
    /// The digits without trailing zeros; empty for 0.
    digits: Vec<u8>,
    /// Where the decimal point is: the number is 0.`digits` × 10^`scale` (.NET's `NumberBuffer.Scale`).
    scale: i32,
}

impl Digits {
    fn double(value: f64) -> Self {
        Self::parse(&format!("{value:.14e}"))
    }

    fn single(value: f32) -> Self {
        Self::parse(&format!("{value:.6e}"))
    }

    /// From Rust's exponent form, "-1.234560e2".
    fn parse(text: &str) -> Self {
        let (negative, text) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (mantissa, exponent) = text.split_once('e').unwrap_or((text, "0"));
        let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
        while digits.last() == Some(&b'0') {
            digits.pop();
        }
        let scale = if digits.is_empty() {
            0
        } else {
            exponent.parse::<i32>().unwrap_or(0) + 1
        };
        Self {
            negative,
            digits,
            scale,
        }
    }
}

/// A custom numeric format of `0` and at most `max` decimals of which `min` are always shown ("0.###" is 3 and 0, "0.0" 1 and 1):
/// the digits are rounded half up at the last decimal (dotnet/runtime main, `Number.Formatting.Common.cs` `RoundNumber`, read
/// 2026-10-27), trailing optional zeros dropped, the UI's decimal separator between. A number that is not finite never reaches the
/// panel (the facts are measured or parsed numbers) and is written as Rust writes it: not verified against .NET.
fn decimals(number: Digits, max: usize, min: usize, format: &NumberFormat) -> String {
    let Digits {
        negative,
        mut digits,
        mut scale,
    } = number;
    let max_i32 = i32::try_from(max).unwrap_or(i32::MAX);
    // `pos` is the index of the first digit dropped; .NET rounds only when that digit is in the buffer.
    let pos = scale.saturating_add(max_i32);
    if let Ok(pos) = usize::try_from(pos) {
        if pos < digits.len() {
            let round_up = digits[pos] >= b'5';
            digits.truncate(pos);
            if round_up {
                let mut i = pos;
                while i > 0 && digits[i - 1] == b'9' {
                    i -= 1;
                }
                digits.truncate(i);
                if let Some(last) = digits.last_mut() {
                    *last += 1;
                } else {
                    scale += 1;
                    digits.push(b'1');
                }
            }
            while digits.last() == Some(&b'0') {
                digits.pop();
            }
        }
    } else {
        digits.clear();
    }
    if digits.is_empty() {
        scale = 0;
    }
    let digit = |index: i32| -> char {
        usize::try_from(index)
            .ok()
            .and_then(|index| digits.get(index))
            .map_or('0', |digit| char::from(*digit))
    };
    let mut text = String::new();
    if negative && !digits.is_empty() {
        text.push('-');
    }
    if scale <= 0 {
        text.push('0');
    } else {
        text.extend((0..scale).map(digit));
    }
    let mut fraction: Vec<char> = (0..max_i32).map(|k| digit(scale + k)).collect();
    while fraction.len() > min && fraction.last() == Some(&'0') {
        fraction.pop();
    }
    if !fraction.is_empty() {
        text.push_str(&format.decimal_separator);
        text.extend(fraction);
    }
    text
}

#[cfg(test)]
mod tests {
    use current_model::{
        AudioFacts, Codec, ItemId, ItemRef, MediaSource, MediaStream, PlaybackPlan, RouteReason,
        ServerKey, ServerRef, ServiceType, StreamAction, StreamKind, TrackChoice, TranscodingFacts,
        VideoFacts, VideoRangeType,
    };
    use current_playback::{PlanNote, PlaybackDetails};

    use super::super::backend::{EngineOutput, VideoOutput};
    use super::*;

    fn dot() -> NumberFormat {
        NumberFormat::default()
    }

    fn comma() -> NumberFormat {
        NumberFormat {
            decimal_separator: ",".to_owned(),
        }
    }

    fn double(value: f64, max: usize, min: usize) -> String {
        decimals(Digits::double(value), max, min, &dot())
    }

    fn single(value: f32, max: usize, min: usize) -> String {
        decimals(Digits::single(value), max, min, &dot())
    }

    #[test]
    fn frame_rates_read_as_dotnet_wrote_them() {
        // "0.###" on a float: 7 significant digits, then half up.
        for (fps, text) in [
            (23.976_f32, "23.976"),
            (24_000.0 / 1_001.0, "23.976"),
            (29.97, "29.97"),
            (30_000.0 / 1_001.0, "29.97"),
            (59.94, "59.94"),
            (60_000.0 / 1_001.0, "59.94"),
            (119.88, "119.88"),
            (120_000.0 / 1_001.0, "119.88"),
            (25.0, "25"),
            (0.0, "0"),
        ] {
            assert_eq!(single(fps, 3, 0), text, "{fps}");
        }
    }

    #[test]
    fn custom_formats_round_half_up_after_their_significant_digits() {
        // "0.##": 1.005 is 1.00499999999999989... as a double, but its 15 digits are 1.00500000000000, so .NET wrote 1.01.
        assert_eq!(double(1.005, 2, 0), "1.01");
        assert_eq!(double(2.675, 2, 0), "2.68");
        assert_eq!(double(1_280.0, 2, 0), "1280");
        assert_eq!(double(1_706.666_666_666_666_7, 2, 0), "1706.67");
        assert_eq!(double(0.006, 2, 0), "0.01");
        assert_eq!(double(0.004, 2, 0), "0");
        assert_eq!(double(0.000_4, 2, 0), "0");
        assert_eq!(double(9.999, 2, 0), "10");
        assert_eq!(double(99.995, 2, 0), "100");
        // "0": half up, not to even as Math.Round would.
        assert_eq!(double(0.5, 0, 0), "1");
        assert_eq!(double(2.5, 0, 0), "3");
        assert_eq!(double(42.4, 0, 0), "42");
        assert_eq!(single(1_000.5, 0, 0), "1001");
        // "0.0": one decimal always.
        assert_eq!(double(0.0, 1, 1), "0.0");
        assert_eq!(double(4.0, 1, 1), "4.0");
        assert_eq!(double(4.25, 1, 1), "4.3");
        assert_eq!(double(0.05, 1, 1), "0.1");
        // "0.###" on a double: 59.940059940059939 has 15 digits 59.9400599400599.
        assert_eq!(double(60_000.0 / 1_001.0, 3, 0), "59.94");
        assert_eq!(double(143.999, 3, 0), "143.999");
        assert_eq!(double(143.9996, 3, 0), "144");
        assert_eq!(decimals(Digits::double(1.5), 2, 0, &comma()), "1,5");
    }

    fn info() -> PlaybackInfo {
        let video = MediaStream {
            index: 0,
            kind: StreamKind::from("video"),
            codec: Some(Codec::from("hevc")),
            profile: Some("Main 10".into()),
            level: None,
            language: None,
            title: None,
            display_title: Some("4K HDR10".into()),
            is_default: true,
            is_forced: false,
            is_hearing_impaired: false,
            is_external: false,
            video: Some(VideoFacts {
                width: Some(3_840),
                height: Some(2_160),
                bit_depth: Some(10),
                frame_rate: Some(23.976),
                range: None,
                range_type: Some(VideoRangeType::from("HDR10")),
                dv_profile: None,
            }),
            audio: None,
            subtitle_delivery: None,
        };
        let audio = MediaStream {
            index: 1,
            kind: StreamKind::from("audio"),
            codec: Some(Codec::from("eac3")),
            profile: None,
            language: Some("eng".into()),
            title: Some("Surround".into()),
            display_title: None,
            is_default: true,
            video: None,
            audio: Some(AudioFacts {
                channels: Some(6),
                layout: Some("5.1".into()),
                sample_rate: Some(48_000),
            }),
            ..video.clone()
        };
        let subtitle = MediaStream {
            index: 2,
            kind: StreamKind::from("subtitle"),
            codec: Some(Codec::from("srt")),
            profile: None,
            language: None,
            title: None,
            display_title: None,
            is_default: false,
            is_external: true,
            video: None,
            ..video.clone()
        };
        PlaybackInfo {
            details: Some(PlaybackDetails {
                plan: Some(PlaybackPlan {
                    item: ItemRef::new(
                        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
                        ItemId::new("item"),
                    ),
                    source: MediaSource {
                        id: "source".into(),
                        name: None,
                        container: Some("mkv".into()),
                        size: Some(12_345_678_901),
                        bitrate: Some(26_100_000),
                        duration: None,
                        streams: vec![video, audio, subtitle],
                        default_audio: None,
                        default_subtitle: None,
                        has_segments: false,
                        is_remote: false,
                    },
                    play_session_id: String::new(),
                    route: Route::ServerStream {
                        protocol: StreamProtocol::Hls,
                        container: Some("mp4".into()),
                        video: StreamAction::Copy,
                        audio: StreamAction::Transcode,
                    },
                    reasons: vec![
                        RouteReason::from("audio_codec_not_supported"),
                        RouteReason::from("a_reason_no_release_knows"),
                    ],
                    start: std::time::Duration::ZERO,
                    tracks: TrackChoice::default(),
                }),
                notes: vec![PlanNote::RemoteBitrateLimit { limit: 8_500_000 }],
                transcoding: Some(TranscodingFacts {
                    video_direct: true,
                    audio_direct: false,
                    audio_codec: Some(Codec::from("aac")),
                    container: Some("mp4".into()),
                    bitrate: Some(27_000_000),
                    frame_rate: Some(23.976),
                    completion: Some(12.5),
                    hardware_acceleration: Some("nvenc".into()),
                    reasons: vec![RouteReason::from("audio_codec_not_supported")],
                    ..TranscodingFacts::default()
                }),
            }),
            engine: EngineOutput {
                hardware_decoder: Some("d3d11va".into()),
                video: Some(VideoOutput {
                    width: 3_840,
                    height: 2_160,
                    pixel_format: Some("p010".into()),
                    primaries: Some("bt.2020".into()),
                    transfer: Some("pq".into()),
                }),
            },
        }
    }

    fn panel() -> PanelSizes {
        PanelSizes {
            composition_width: 1_920,
            composition_height: 1_080,
            view_width: 1_920.0,
            view_height: 1_080.0,
            content: Some((1_706.666_666_666_666_7, 960.0)),
            window: Some((1_920, 1_080)),
        }
    }

    #[test]
    fn the_panel_reads_as_the_windows_app_wrote_it() {
        let display = DisplayFacts {
            hdr: true,
            peak_nits: 1_000.4,
            sdr_white_nits: 240.0,
            refresh_hz: 60_000.0 / 1_001.0,
        };
        assert_eq!(
            info_panel_lines(&info(), Some(&display), &panel(), &dot()),
            [
                "Direct stream · HLS · mp4",
                "• The player can't decode this audio codec.",
                "• a_reason_no_release_knows",
                "• The server limits streaming outside your network to 8.5 Mbit/s.",
                "",
                "Source · mkv · 26.1 Mbit/s · 12.3 GB",
                "#0 video · hevc · Main 10 · 3840×2160 · 23.976 fps · 10-bit · HDR10 · 4K HDR10",
                "#1 audio · eac3 · 6 ch · eng · Surround",
                "#2 subtitle · srt · external",
                "",
                "Server · video copied · audio to aac · mp4 · 27 Mbit/s · 23.976 fps · nvenc · 13% done",
                "• The player can't decode this audio codec.",
                "",
                "Player · decoder d3d11va · 3840×2160 · p010 · bt.2020 · pq",
                "Display · HDR · peak 1000 nits · SDR white 240 nits · 59.94 Hz",
                "Panel · 1920×1080 · view 1920×1080 · content 1706.67×960 · window 1920×1080",
            ]
        );
    }

    #[test]
    fn the_panel_uses_the_platforms_separator_and_leaves_out_what_is_unknown() {
        let display = DisplayFacts {
            hdr: false,
            peak_nits: 0.0,
            sdr_white_nits: 0.0,
            refresh_hz: 0.0,
        };
        let mut info = info();
        if let Some(details) = info.details.as_mut() {
            details.notes.clear();
            if let Some(t) = details.transcoding.as_mut() {
                t.video_direct = false;
                t.video_codec = None;
                t.completion = None;
                t.reasons.clear();
            }
        }
        let panel = PanelSizes {
            view_width: 1_280.5,
            view_height: 720.25,
            content: None,
            window: None,
            ..panel()
        };
        let lines = info_panel_lines(&info, Some(&display), &panel, &comma());
        assert_eq!(lines[4], "Source · mkv · 26,1 Mbit/s · 12,3 GB");
        assert_eq!(
            lines[5],
            "#0 video · hevc · Main 10 · 3840×2160 · 23,976 fps · 10-bit · HDR10 · 4K HDR10"
        );
        // An unknown codec is written as nothing after "to", as it was.
        assert_eq!(
            lines[9],
            "Server · video to  · audio to aac · mp4 · 27 Mbit/s · 23,976 fps · nvenc"
        );
        assert_eq!(lines[12], "Display · SDR");
        assert_eq!(lines[13], "Panel · 1920×1080 · view 1280,5×720,25");
    }

    #[test]
    fn without_a_plan_the_panel_has_the_player_and_the_panel() {
        let lines = info_panel_lines(
            &PlaybackInfo::default(),
            None,
            &PanelSizes::default(),
            &dot(),
        );
        assert_eq!(
            lines,
            ["", "Player · software decoder", "Panel · 0×0 · view 0×0"]
        );
    }
}
