//! Settings' "Supported formats" page (ADR 0021): what this PC plays, directly, after the server repackages the stream (remux) and
//! after it converts it (transcode), with whether the graphics card decodes each video codec. The core builds the whole page, words
//! included (tab titles, column headers, the support column's labels and the notes); the UI only renders it.
//!
//! Every answer comes from three inputs, never from a fixed list of what should work:
//! - the engine's own capabilities ([`EngineReport`], from libmpv's `decoder-list` and `demuxer-lavf-list`);
//! - what the device profile asks of the server for them (`current_jellyfin::transcode_targets` and
//!   `current_jellyfin::external_subtitle_formats`, read from the profiles themselves);
//! - the facts the UI passes in: the graphics card's video decoder profiles ([`HardwareDecoding`], a Direct3D 11 test on Windows) and
//!   the display the window is on ([`DisplayFacts`]).
//!
//! The tables below only name and order the rows. A codec, container or subtitle format the engine reports that they lack still gets
//! a row, under its own name (open values).

use current_jellyfin::{TranscodeTargets, external_subtitle_formats, transcode_targets};
use current_model::{Codec, Container, EngineCapabilities, SubtitleFormat};

use crate::player::{DisplayFacts, EngineReport};

/// One of the graphics card's video decoder profiles, as Direct3D 11 lists it (`ID3D11VideoDevice::GetVideoDecoderProfile`), with the
/// output formats the driver accepts for it (`CheckVideoDecoderFormat`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderProfile {
    /// The profile's GUID, in any case, with or without braces.
    pub guid: String,
    /// `DXGI_FORMAT_NV12` (8-bit 4:2:0) is accepted.
    pub nv12: bool,
    /// `DXGI_FORMAT_P010` (10-bit 4:2:0) is accepted.
    pub p010: bool,
}

/// What the UI found out about the graphics card the engine decodes on.
#[derive(Clone, PartialEq, Eq)]
pub struct HardwareDecoding {
    /// The adapter's name as the system reports it: shown on the page, never logged.
    pub adapter: String,
    pub profiles: Vec<DecoderProfile>,
}

impl std::fmt::Debug for HardwareDecoding {
    /// Leaves out the adapter's name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareDecoding")
            .field("adapter", &"<redacted>")
            .field("profiles", &self.profiles)
            .finish()
    }
}

/// The tabs, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormatTabKind {
    Video,
    Audio,
    Containers,
    Subtitles,
    Hdr,
}

impl FormatTabKind {
    fn title(self) -> &'static str {
        match self {
            FormatTabKind::Video => "Video",
            FormatTabKind::Audio => "Audio",
            FormatTabKind::Containers => "Containers",
            FormatTabKind::Subtitles => "Subtitles",
            FormatTabKind::Hdr => "HDR",
        }
    }

    fn name_header(self) -> &'static str {
        match self {
            FormatTabKind::Video | FormatTabKind::Audio => "Codec",
            FormatTabKind::Containers | FormatTabKind::Subtitles | FormatTabKind::Hdr => "Format",
        }
    }
}

/// The "On this PC" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormatSupport {
    /// Plays, decoded by the graphics card.
    Hardware,
    /// Plays, decoded by the processor.
    Software,
    /// Plays (containers, subtitles; and video when the graphics card could not be tested).
    Plays,
    /// Plays after the server repackages the stream into a container the engine reads (remux).
    ServerRepackages,
    /// Plays after the server converts it to a format the engine takes (transcode).
    ServerConverts,
    /// Does not play.
    NotSupported,
    /// HDR video shown in HDR: the display is in HDR mode.
    Hdr,
    /// HDR video converted to SDR by the player: the display is not in HDR mode.
    ToneMapped,
    /// Not known: the player could not be started to find out.
    Unknown,
}

impl FormatSupport {
    /// The column's word for it.
    pub fn label(self) -> &'static str {
        match self {
            FormatSupport::Hardware => "Hardware",
            FormatSupport::Software => "Software",
            FormatSupport::Plays => "Plays",
            FormatSupport::ServerRepackages => "Server repackages",
            FormatSupport::ServerConverts => "Server converts",
            FormatSupport::NotSupported => "Not supported",
            FormatSupport::Hdr => "HDR",
            FormatSupport::ToneMapped => "Tone-mapped",
            FormatSupport::Unknown => "Couldn't check",
        }
    }
}

/// One row: the format's name, the column (its value and its word), and a note (empty when there is nothing to say).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatRow {
    pub name: String,
    pub support: FormatSupport,
    pub support_label: String,
    pub notes: String,
}

/// One tab: its title, its three column headers and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatTab {
    pub kind: FormatTabKind,
    pub title: String,
    /// "Codec" or "Format".
    pub name_header: String,
    pub support_header: String,
    pub notes_header: String,
    pub rows: Vec<FormatRow>,
}

/// The page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportedFormats {
    /// Shown above the tabs.
    pub description: String,
    /// What the Hardware and Software values rest on: the graphics card tested, or why none was.
    pub hardware_note: String,
    pub tabs: Vec<FormatTab>,
}

const DESCRIPTION: &str = "What this PC plays. Whether a file plays also depends on the file itself, on how the server sends it, and \
                           on the display.";

const SUPPORT_HEADER: &str = "On this PC";
const NOTES_HEADER: &str = "Notes";

/// Every row's note when the player could not be started to check.
const UNKNOWN_NOTE: &str = "The player couldn't be started to check this";

/// Codecs mpv tries hardware decoding for: `--hwdec-codecs`' default, "h264,vc1,hevc,vp8,vp9,av1,prores,prores_raw,ffv1,dpx"
/// (`DOCS/man/options.rst` and `video/decode/vd_lavc.c`, mpv v0.41.0, read 2026-10-26). `hwdec=auto-safe` (ADR 0007, ADR 0019) is
/// "exactly the same as `auto`", which enables "any whitelisted hw decoder" (same page); the engine does not change the codec list.
const HWDEC_CODECS: &[&str] = &[
    "h264",
    "vc1",
    "hevc",
    "vp8",
    "vp9",
    "av1",
    "prores",
    "prores_raw",
    "ffv1",
    "dpx",
];

/// The bit depth a decoder profile is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Eight,
    Ten,
    /// AV1 Main: one profile for both; the output format says which.
    Both,
}

/// The Direct3D 11 decoder profiles FFmpeg's d3d11va decoder uses, per codec (`dxva_modes` in `libavcodec/dxva2.c`, FFmpeg n8.0, read
/// 2026-10-26), with the GUIDs of `d3d11.h` and `dxva.h` (mingw-w64's `d3d11.idl` and `dxva.h`, read 2026-10-26: `H264_VLD_NOFGT`,
/// `H264_VLD_FGT`, Intel's `ModeH264_E`, `VC1_D2010`, `VC1_VLD`, `MPEG2_VLD`, `MPEG2and1_VLD`, `HEVC_VLD_MAIN`, `HEVC_VLD_MAIN10`,
/// `VP9_VLD_PROFILE0`, `VP9_VLD_10BIT_PROFILE2`, `DXVA_ModeAV1_VLD_Profile0`). FFmpeg checks NV12 for 8-bit output and P010 for
/// 10-bit (`d3d11va_map_sw_to_hw_format`). VP8 has none: FFmpeg has no D3D11 VP8 decoder.
const D3D11_PROFILES: &[(&str, &str, Depth)] = &[
    ("h264", "1b81be68-a0c7-11d3-b984-00c04f2e73c5", Depth::Eight),
    ("h264", "1b81be69-a0c7-11d3-b984-00c04f2e73c5", Depth::Eight),
    ("h264", "604f8e68-4951-4c54-88fe-abd25c15b3d6", Depth::Eight),
    ("vc1", "1b81bea4-a0c7-11d3-b984-00c04f2e73c5", Depth::Eight),
    ("vc1", "1b81bea3-a0c7-11d3-b984-00c04f2e73c5", Depth::Eight),
    (
        "mpeg2video",
        "ee27417f-5e28-4e65-beea-1d26b508adc9",
        Depth::Eight,
    ),
    (
        "mpeg2video",
        "86695f12-340e-4f04-9fd3-9253dd327460",
        Depth::Eight,
    ),
    ("hevc", "5b11d51b-2f4c-4452-bcc3-09f2a1160cc0", Depth::Eight),
    ("hevc", "107af0e0-ef1a-4d19-aba8-67a163073d13", Depth::Ten),
    ("vp9", "463707f8-a1d0-4585-876d-83aa6d60b89e", Depth::Eight),
    ("vp9", "a4c749ef-6ecf-48aa-8448-50a7a1165ff7", Depth::Ten),
    ("av1", "b8be4ccb-cf53-46ba-8d59-d6b8a6da5d2a", Depth::Both),
];

/// Video rows: the codec, its name, and a note that always applies.
const VIDEO: &[(Codec, &str, &str)] = &[
    (Codec::H264, "H.264 (AVC)", ""),
    (Codec::HEVC, "HEVC (H.265)", ""),
    (Codec::AV1, "AV1", ""),
    (Codec::VP9, "VP9", ""),
    (Codec::VP8, "VP8", ""),
    (Codec::MPEG2, "MPEG-2", ""),
    (Codec::VC1, "VC-1", ""),
    // Not in the engine's codec table today (`current-mpv`'s `caps.rs`), so the server converts it; its row shows that.
    (Codec::MPEG4, "MPEG-4 Part 2", "DivX, Xvid"),
];

/// Audio rows. The Dolby Atmos row follows TrueHD (see [`atmos_row`]).
const AUDIO: &[(Codec, &str)] = &[
    (Codec::AAC, "AAC"),
    (Codec::AC3, "Dolby Digital (AC-3)"),
    (Codec::EAC3, "Dolby Digital Plus (E-AC-3)"),
    (Codec::TRUEHD, "Dolby TrueHD"),
    (Codec::DTS, "DTS"),
    (Codec::FLAC, "FLAC"),
    (Codec::OPUS, "Opus"),
    (Codec::VORBIS, "Vorbis"),
    (Codec::MP3, "MP3"),
    (Codec::ALAC, "ALAC"),
    (Codec::PCM_S16LE, "PCM, 16-bit"),
    (Codec::PCM_S24LE, "PCM, 24-bit"),
];

/// Compressed audio a receiver could decode itself if it were passed through.
const BITSTREAM_AUDIO: &[Codec] = &[Codec::AC3, Codec::EAC3, Codec::TRUEHD, Codec::DTS];

const CONTAINERS: &[(Container, &str)] = &[
    (Container::MKV, "Matroska (MKV)"),
    (Container::WEBM, "WebM"),
    (Container::MP4, "MP4"),
    (Container::MOV, "QuickTime (MOV)"),
    (Container::TS, "MPEG-TS"),
    (Container::AVI, "AVI"),
    (Container::OGG, "Ogg"),
    (Container::FLAC, "FLAC"),
];

/// Subtitle rows: one per format, with every spelling the server uses for it (`SubtitleFormat`), and a note.
const SUBTITLES: &[(&[SubtitleFormat], &str, &str)] = &[
    (
        &[SubtitleFormat::ASS, SubtitleFormat::SSA],
        "SubStation Alpha (ASS, SSA)",
        "With its styles",
    ),
    (
        &[SubtitleFormat::SUBRIP, SubtitleFormat::SRT],
        "SubRip (SRT)",
        "",
    ),
    (&[SubtitleFormat::WEBVTT, SubtitleFormat::VTT], "WebVTT", ""),
    (&[SubtitleFormat::MOV_TEXT], "MP4 text", ""),
    (&[SubtitleFormat::PGSSUB], "PGS (Blu-ray)", "Picture-based"),
    (&[SubtitleFormat::DVDSUB], "VobSub (DVD)", "Picture-based"),
    (&[SubtitleFormat::DVBSUB], "DVB", "Picture-based"),
];

/// HDR rows: the name and a note that always applies. HDR10+ and Dolby Vision were not checked on a device (docs/notes/windows-video.md,
/// "ADR 0007 item 5").
const HDR: &[(&str, &str)] = &[
    ("HDR10", ""),
    ("HDR10+", "Not checked on a device yet"),
    ("HLG", ""),
    ("Dolby Vision profile 5", "Not checked on a device yet"),
    (
        "Dolby Vision profile 7",
        "Dual-layer. Not checked on a device yet",
    ),
    ("Dolby Vision profile 8", "Not checked on a device yet"),
];

/// The page for what is known now. `hardware` is `None` when the graphics card could not be tested (or the platform has no test);
/// `display` is `None` when the window's display is unknown.
pub fn supported_formats(
    engine: &EngineReport,
    hardware: Option<&HardwareDecoding>,
    display: Option<&DisplayFacts>,
) -> SupportedFormats {
    let empty = EngineCapabilities::default();
    let caps = match engine {
        EngineReport::Ready(capabilities) => capabilities,
        EngineReport::Unavailable | EngineReport::Failed => &empty,
    };
    let ready = matches!(engine, EngineReport::Ready(_));
    let targets = transcode_targets(caps);
    let description = match engine {
        EngineReport::Ready(_) => DESCRIPTION,
        EngineReport::Unavailable => {
            "This version of Current has no player, so nothing plays on this PC."
        }
        EngineReport::Failed => {
            "The player couldn't be started to find out what it plays. Open this page again to try again."
        }
    };
    let hardware_note = match hardware {
        _ if !ready => String::new(),
        Some(hardware) if hardware.adapter.trim().is_empty() => "Tested on this PC's graphics card.".to_owned(),
        Some(hardware) => format!("Tested on: {}", hardware.adapter.trim()),
        None => "The graphics card couldn't be tested, so video shows as \"Plays\" rather than Hardware or Software.".to_owned(),
    };
    let mut tabs = vec![
        tab(FormatTabKind::Video, video_rows(caps, &targets, hardware)),
        tab(FormatTabKind::Audio, audio_rows(caps, &targets)),
        tab(FormatTabKind::Containers, container_rows(caps, &targets)),
        tab(FormatTabKind::Subtitles, subtitle_rows(caps, &targets)),
        tab(FormatTabKind::Hdr, hdr_rows(ready, display)),
    ];
    // Without the engine's answer nothing is known: no row claims a format does not play.
    if *engine == EngineReport::Failed {
        for row in tabs.iter_mut().flat_map(|tab| &mut tab.rows) {
            *row = self::row(
                std::mem::take(&mut row.name),
                FormatSupport::Unknown,
                UNKNOWN_NOTE,
            );
        }
    }
    SupportedFormats {
        description: description.to_owned(),
        hardware_note,
        tabs,
    }
}

fn tab(kind: FormatTabKind, rows: Vec<FormatRow>) -> FormatTab {
    FormatTab {
        kind,
        title: kind.title().to_owned(),
        name_header: kind.name_header().to_owned(),
        support_header: SUPPORT_HEADER.to_owned(),
        notes_header: NOTES_HEADER.to_owned(),
        rows,
    }
}

fn row(name: impl Into<String>, support: FormatSupport, notes: impl Into<String>) -> FormatRow {
    FormatRow {
        name: name.into(),
        support,
        support_label: support.label().to_owned(),
        notes: notes.into(),
    }
}

/// Notes joined into sentences.
fn notes(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(". ")
}

/// Names joined as "a, b or c".
fn either(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// The display names of `codecs`, from `names`, or their own.
fn codec_names(codecs: &[Codec], names: impl Fn(&Codec) -> Option<&'static str>) -> String {
    let names: Vec<String> = codecs
        .iter()
        .map(|codec| names(codec).map_or_else(|| codec.as_str().to_owned(), str::to_owned))
        .collect();
    either(&names)
}

pub(crate) fn video_name(codec: &Codec) -> Option<&'static str> {
    VIDEO
        .iter()
        .find(|(known, _, _)| known == codec)
        .map(|(_, name, _)| *name)
}

pub(crate) fn audio_name(codec: &Codec) -> Option<&'static str> {
    AUDIO
        .iter()
        .find(|(known, _)| known == codec)
        .map(|(_, name)| *name)
}

fn video_rows(
    caps: &EngineCapabilities,
    targets: &TranscodeTargets,
    hardware: Option<&HardwareDecoding>,
) -> Vec<FormatRow> {
    let converts = format!(
        "The server converts it to {}",
        codec_names(&targets.video, video_name)
    );
    let mut codecs: Vec<(Codec, String, &str)> = VIDEO
        .iter()
        .map(|(codec, name, note)| (codec.clone(), (*name).to_owned(), *note))
        .collect();
    for support in &caps.video {
        if !codecs.iter().any(|(codec, _, _)| *codec == support.codec) {
            codecs.push((support.codec.clone(), support.codec.as_str().to_owned(), ""));
        }
    }
    codecs
        .into_iter()
        .map(|(codec, name, always)| {
            if caps.decodes_video(&codec) {
                match hardware {
                    Some(hardware) => {
                        let (support, note) = hardware_support(&codec, hardware);
                        row(name, support, notes(&[always, &note]))
                    }
                    None => row(name, FormatSupport::Plays, always),
                }
            } else if !targets.video.is_empty() {
                row(
                    name,
                    FormatSupport::ServerConverts,
                    notes(&[always, &converts]),
                )
            } else {
                row(name, FormatSupport::NotSupported, always)
            }
        })
        .collect()
}

/// `{8D2...}` or `8d2...` as lower case without braces.
fn normalized_guid(guid: &str) -> String {
    guid.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_lowercase()
}

/// Whether the engine decodes `codec` on the graphics card, and why not when it does not. The engine plays with mpv's
/// `hwdec=auto-safe` (ADR 0007, ADR 0019), which tries the whitelisted methods of `hwdec_autoprobe_info` (`video/decode/vd_lavc.c`,
/// mpv v0.41.0, read 2026-10-26) with the direct ones first ("List non-copying entries first, so --hwdec=auto takes them",
/// `hwdec_compare`): on Windows `d3d11va` comes first, and this answers for it. A codec mpv tries hardware decoding for, with a decoder
/// profile the card lists and an output format the driver accepts for it, is decoded there. When d3d11va cannot take a file, auto-safe
/// can still fall back to a copying method (`d3d11va-copy`, `dxva2-copy`, `nvdec-copy`), so "Software" may still be decoded by the card
/// through a copy (not verified).
fn hardware_support(codec: &Codec, hardware: &HardwareDecoding) -> (FormatSupport, String) {
    let listed: Vec<(String, &DecoderProfile)> = hardware
        .profiles
        .iter()
        .map(|profile| (normalized_guid(&profile.guid), profile))
        .collect();
    let (mut eight, mut ten) = (false, false);
    let mut any_profile = false;
    for (_, guid, depth) in D3D11_PROFILES
        .iter()
        .filter(|(name, _, _)| *name == codec.as_str())
    {
        any_profile = true;
        for (_, profile) in listed.iter().filter(|(listed, _)| listed == guid) {
            match depth {
                Depth::Eight => eight |= profile.nv12,
                Depth::Ten => ten |= profile.p010,
                Depth::Both => {
                    eight |= profile.nv12;
                    ten |= profile.p010;
                }
            }
        }
    }
    let software = FormatSupport::Software;
    if !any_profile {
        return (
            software,
            "The player has no hardware decoding for this codec".to_owned(),
        );
    }
    if !HWDEC_CODECS.contains(&codec.as_str()) {
        let note = if eight || ten {
            "The graphics card has a decoder for it, but the player decodes it in software"
        } else {
            ""
        };
        return (software, note.to_owned());
    }
    let ten_bit = codec == &Codec::HEVC || codec == &Codec::VP9 || codec == &Codec::AV1;
    match (eight, ten) {
        (false, false) => (
            software,
            "No hardware decoder on this graphics card".to_owned(),
        ),
        (true, true) => (FormatSupport::Hardware, "8-bit and 10-bit".to_owned()),
        (true, false) if ten_bit => (
            FormatSupport::Hardware,
            "8-bit; 10-bit is decoded in software".to_owned(),
        ),
        (true, false) => (FormatSupport::Hardware, String::new()),
        (false, true) => (
            FormatSupport::Hardware,
            "10-bit; 8-bit is decoded in software".to_owned(),
        ),
    }
}

fn audio_rows(caps: &EngineCapabilities, targets: &TranscodeTargets) -> Vec<FormatRow> {
    let converts = format!(
        "The server converts it to {}",
        codec_names(&targets.audio, audio_name)
    );
    let decoded = "Decoded on this PC, not passed through to a receiver";
    let mut codecs: Vec<(Codec, String)> = AUDIO
        .iter()
        .map(|(codec, name)| (codec.clone(), (*name).to_owned()))
        .collect();
    for support in &caps.audio {
        if !codecs.iter().any(|(codec, _)| *codec == support.codec) {
            codecs.push((support.codec.clone(), support.codec.as_str().to_owned()));
        }
    }
    let mut rows = Vec::new();
    for (codec, name) in codecs {
        let codec_row = if caps.decodes_audio(&codec) {
            let note = if BITSTREAM_AUDIO.contains(&codec) {
                decoded
            } else {
                ""
            };
            row(name, FormatSupport::Software, note)
        } else if !targets.audio.is_empty() {
            row(name, FormatSupport::ServerConverts, converts.clone())
        } else {
            row(name, FormatSupport::NotSupported, "")
        };
        rows.push(codec_row);
        if codec == Codec::TRUEHD {
            rows.push(atmos_row(caps, targets, &converts));
        }
    }
    rows
}

/// Dolby Atmos travels inside TrueHD or E-AC-3. The engine sets no `audio-spdif` (`current-mpv`'s `start_options`), and mpv passes
/// nothing through by default (`audio-spdif` has no default value, `filters/f_decoder_wrapper.c`, mpv v0.41.0, read 2026-10-26), so
/// it decodes both to PCM; that the height information is lost then is not verified against FFmpeg's decoders.
fn atmos_row(caps: &EngineCapabilities, targets: &TranscodeTargets, converts: &str) -> FormatRow {
    let name = "Dolby Atmos";
    if caps.decodes_audio(&Codec::TRUEHD) || caps.decodes_audio(&Codec::EAC3) {
        row(
            name,
            FormatSupport::Software,
            "Plays as ordinary surround: decoded on this PC, not passed through to a receiver, so the height information is lost \
             (not verified)",
        )
    } else if !targets.audio.is_empty() {
        row(name, FormatSupport::ServerConverts, converts)
    } else {
        row(name, FormatSupport::NotSupported, "")
    }
}

/// A container the engine does not read is repackaged by the server. Jellyfin turns direct stream off for every client
/// (`MediaInfoHelper`: "direct-stream http streaming is currently broken"), so a remux arrives as a transcode into one of the device
/// profile's HLS transcoding profiles whose job copies the video and audio streams the device takes (`AllowVideoStreamCopy`,
/// `AllowAudioStreamCopy`, default true; `GetVideoTranscodeProfile` ranks profiles by whether the video can be copied) (Jellyfin v12.2,
/// commit ca0f16eb, read 2026-10-25, docs/notes/playback.md "How the server decides"). So it needs a transcoding profile.
fn container_rows(caps: &EngineCapabilities, targets: &TranscodeTargets) -> Vec<FormatRow> {
    let mut containers: Vec<(Container, String)> = CONTAINERS
        .iter()
        .map(|(container, name)| (container.clone(), (*name).to_owned()))
        .collect();
    for container in &caps.containers {
        if !containers.iter().any(|(known, _)| known == container) {
            containers.push((container.clone(), container.as_str().to_owned()));
        }
    }
    let repackages = !targets.video.is_empty();
    containers
        .into_iter()
        .map(|(container, name)| {
            if caps.containers.contains(&container) {
                row(name, FormatSupport::Plays, "")
            } else if repackages {
                row(
                    name,
                    FormatSupport::ServerRepackages,
                    "The server repackages it as an HLS stream, copying the video and audio this PC decodes",
                )
            } else {
                row(name, FormatSupport::NotSupported, "")
            }
        })
        .collect()
}

/// Whether Jellyfin counts `format` as text (`MediaStream.IsTextFormat`: anything but PGS, DVD, VobSub, DVB, `sup` and `sub`).
fn is_text_subtitle(format: &str) -> bool {
    let format = format.to_ascii_lowercase();
    format.contains("microdvd")
        || !(format.contains("pgs")
            || format.contains("dvdsub")
            || format.contains("vobsub")
            || format.contains("dvbsub")
            || format == "sup"
            || format == "sub")
}

/// Whether Jellyfin can convert a text subtitle from or to `format` (`MediaStream.SupportsSubtitleConversionTo`: not from or to ASS
/// or SSA).
fn converts_text(format: &str) -> bool {
    !format.eq_ignore_ascii_case("ass") && !format.eq_ignore_ascii_case("ssa")
}

pub(crate) fn subtitle_name(format: &SubtitleFormat) -> String {
    SUBTITLES
        .iter()
        .find(|(spellings, _, _)| {
            spellings
                .iter()
                .any(|known| known.as_str().eq_ignore_ascii_case(format.as_str()))
        })
        .map_or_else(
            || format.as_str().to_owned(),
            |(_, name, _)| (*name).to_owned(),
        )
}

/// A subtitle format the engine does not render, by Jellyfin's rule (`StreamBuilder.GetSubtitleProfile` and
/// `GetExternalSubtitleProfile`, `MediaStream.SupportsSubtitleConversionTo`; Jellyfin v12.2, commit ca0f16eb, read 2026-10-26): an
/// `External` profile of the same kind is looked for without conversion, then with one, which only a text subtitle allows ("Can't
/// convert from this": `ass`, `ssa`; "Can't convert to this": the same); otherwise the server falls back to
/// `SubtitleDeliveryMethod.Encode`, burning it into the picture, which converts the video. So a text format other than ASS and SSA
/// becomes one of the profile's external text formats; ASS, SSA and picture-based formats are burned in. That the server can extract
/// the track (its `EnableSubtitleExtraction` setting, on by default) is assumed.
fn subtitle_rows(caps: &EngineCapabilities, targets: &TranscodeTargets) -> Vec<FormatRow> {
    let mut formats: Vec<(Vec<SubtitleFormat>, String, &str)> = SUBTITLES
        .iter()
        .map(|(formats, name, note)| (formats.to_vec(), (*name).to_owned(), *note))
        .collect();
    for format in &caps.subtitles {
        let known = formats.iter().any(|(spellings, _, _)| {
            spellings
                .iter()
                .any(|known| known.as_str().eq_ignore_ascii_case(format.as_str()))
        });
        if !known {
            formats.push((vec![format.clone()], format.as_str().to_owned(), ""));
        }
    }
    let renders = |spellings: &[SubtitleFormat]| {
        caps.subtitles.iter().any(|format| {
            spellings
                .iter()
                .any(|known| known.as_str().eq_ignore_ascii_case(format.as_str()))
        })
    };
    let mut text_targets: Vec<String> = Vec::new();
    for format in external_subtitle_formats(caps) {
        let name = subtitle_name(&format);
        if is_text_subtitle(format.as_str())
            && converts_text(format.as_str())
            && !text_targets.contains(&name)
        {
            text_targets.push(name);
        }
    }
    formats
        .into_iter()
        .map(|(spellings, name, always)| {
            let format = spellings[0].as_str();
            if renders(&spellings) {
                row(name, FormatSupport::Plays, always)
            } else if is_text_subtitle(format) && converts_text(format) && !text_targets.is_empty()
            {
                let note = format!("The server converts it to {}", either(&text_targets));
                row(name, FormatSupport::ServerConverts, notes(&[always, &note]))
            } else if !targets.video.is_empty() {
                row(
                    name,
                    FormatSupport::ServerConverts,
                    notes(&[
                        always,
                        "The server burns it into the picture, which means converting the video",
                    ]),
                )
            } else {
                row(name, FormatSupport::NotSupported, always)
            }
        })
        .collect()
}

/// HDR as ADR 0007 maps it: on a display in HDR mode the engine outputs HDR (PQ, BT.2020); on any other it tone-maps to SDR. The facts
/// are those of the display the window showing the page is on; with none the page cannot say which.
fn hdr_rows(engine: bool, display: Option<&DisplayFacts>) -> Vec<FormatRow> {
    HDR.iter()
        .map(|(name, always)| {
            if !engine {
                return row(*name, FormatSupport::NotSupported, "");
            }
            match display {
                Some(display) if display.hdr => row(
                    *name,
                    FormatSupport::Hdr,
                    notes(&["On the display this window is on, which is in HDR mode", always]),
                ),
                Some(_) => row(
                    *name,
                    FormatSupport::ToneMapped,
                    notes(&[
                        "The display this window is on isn't in HDR mode, so the player converts it to SDR",
                        always,
                    ]),
                ),
                None => row(
                    *name,
                    FormatSupport::Plays,
                    notes(&["HDR or tone-mapped to SDR, depending on the display", always]),
                ),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use current_model::CodecSupport;

    use super::*;

    /// What `current-mpv` reports for our Windows build (`build/mpv-windows/check.py`), as in `current-jellyfin`'s profile tests.
    fn mpv_windows() -> EngineCapabilities {
        EngineCapabilities {
            containers: CONTAINERS.iter().map(|(c, _)| c.clone()).collect(),
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
            audio: AUDIO
                .iter()
                .map(|(codec, _)| CodecSupport::any(codec.clone()))
                .collect(),
            subtitles: vec![
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
            ],
            external_subtitles: true,
        }
    }

    fn profile(guid: &str, nv12: bool, p010: bool) -> DecoderProfile {
        DecoderProfile {
            guid: guid.to_owned(),
            nv12,
            p010,
        }
    }

    /// A card with H.264, HEVC Main and Main10, VP9 profile 0, MPEG-2 and no AV1 (GUIDs as Direct3D lists them, in upper case and
    /// braces for one).
    fn card() -> HardwareDecoding {
        HardwareDecoding {
            adapter: "Example Graphics".to_owned(),
            profiles: vec![
                profile("{1B81BE68-A0C7-11D3-B984-00C04F2E73C5}", true, false),
                profile("5b11d51b-2f4c-4452-bcc3-09f2a1160cc0", true, false),
                profile("107af0e0-ef1a-4d19-aba8-67a163073d13", false, true),
                profile("463707f8-a1d0-4585-876d-83aa6d60b89e", true, false),
                profile("ee27417f-5e28-4e65-beea-1d26b508adc9", true, false),
                profile("00000000-0000-0000-0000-000000000001", true, true),
            ],
        }
    }

    fn tab(page: &SupportedFormats, kind: FormatTabKind) -> &FormatTab {
        page.tabs.iter().find(|tab| tab.kind == kind).unwrap()
    }

    fn find<'a>(tab: &'a FormatTab, name: &str) -> &'a FormatRow {
        tab.rows.iter().find(|row| row.name == name).unwrap()
    }

    fn sdr() -> DisplayFacts {
        DisplayFacts {
            hdr: false,
            peak_nits: 300.0,
            sdr_white_nits: 200.0,
            refresh_hz: 60.0,
        }
    }

    #[test]
    fn every_tab_has_rows_in_order() {
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), Some(&card()), None);
        let kinds: Vec<FormatTabKind> = page.tabs.iter().map(|tab| tab.kind).collect();
        assert_eq!(
            kinds,
            [
                FormatTabKind::Video,
                FormatTabKind::Audio,
                FormatTabKind::Containers,
                FormatTabKind::Subtitles,
                FormatTabKind::Hdr
            ]
        );
        assert!(page.tabs.iter().all(|tab| !tab.rows.is_empty()));
        assert_eq!(tab(&page, FormatTabKind::Video).name_header, "Codec");
        assert_eq!(tab(&page, FormatTabKind::Containers).name_header, "Format");
        assert_eq!(page.description, DESCRIPTION);
        assert_eq!(page.hardware_note, "Tested on: Example Graphics");
    }

    #[test]
    fn video_follows_the_graphics_card() {
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), Some(&card()), None);
        let video = tab(&page, FormatTabKind::Video);
        assert_eq!(
            find(video, "H.264 (AVC)"),
            &row("H.264 (AVC)", FormatSupport::Hardware, "")
        );
        assert_eq!(
            find(video, "HEVC (H.265)"),
            &row("HEVC (H.265)", FormatSupport::Hardware, "8-bit and 10-bit")
        );
        assert_eq!(
            find(video, "VP9"),
            &row(
                "VP9",
                FormatSupport::Hardware,
                "8-bit; 10-bit is decoded in software"
            )
        );
        assert_eq!(
            find(video, "AV1"),
            &row(
                "AV1",
                FormatSupport::Software,
                "No hardware decoder on this graphics card"
            )
        );
        // mpv leaves MPEG-2 out of its hardware codecs; VP8 has no D3D11 decoder in FFmpeg.
        assert_eq!(find(video, "MPEG-2").support, FormatSupport::Software);
        assert!(find(video, "MPEG-2").notes.contains("has a decoder"));
        assert_eq!(find(video, "VP8").support, FormatSupport::Software);
        assert_eq!(find(video, "VC-1").support, FormatSupport::Software);
        // Not in the engine's table: the server converts it to what the transcoding profiles carry.
        assert_eq!(
            find(video, "MPEG-4 Part 2"),
            &row(
                "MPEG-4 Part 2",
                FormatSupport::ServerConverts,
                "DivX, Xvid. The server converts it to HEVC (H.265) or H.264 (AVC)"
            )
        );
    }

    #[test]
    fn a_ten_bit_only_profile_and_av1_main() {
        let hardware = HardwareDecoding {
            adapter: " ".to_owned(),
            profiles: vec![
                profile("107af0e0-ef1a-4d19-aba8-67a163073d13", false, true),
                profile("b8be4ccb-cf53-46ba-8d59-d6b8a6da5d2a", true, true),
                // Listed, but the driver takes no output format for it.
                profile("463707f8-a1d0-4585-876d-83aa6d60b89e", false, false),
            ],
        };
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), Some(&hardware), None);
        let video = tab(&page, FormatTabKind::Video);
        assert_eq!(
            find(video, "HEVC (H.265)").notes,
            "10-bit; 8-bit is decoded in software"
        );
        assert_eq!(find(video, "AV1").notes, "8-bit and 10-bit");
        assert_eq!(find(video, "VP9").support, FormatSupport::Software);
        assert_eq!(page.hardware_note, "Tested on this PC's graphics card.");
    }

    #[test]
    fn without_a_test_video_only_says_it_plays() {
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), None, None);
        let video = tab(&page, FormatTabKind::Video);
        assert_eq!(find(video, "HEVC (H.265)").support, FormatSupport::Plays);
        assert!(page.hardware_note.contains("couldn't be tested"));
    }

    #[test]
    fn what_the_engine_lacks_the_server_converts_or_repackages() {
        let mut caps = mpv_windows();
        caps.video.retain(|support| support.codec == Codec::H264);
        caps.audio.retain(|support| support.codec == Codec::AAC);
        caps.containers
            .retain(|container| *container == Container::MP4 || *container == Container::TS);
        caps.subtitles = vec![SubtitleFormat::SRT];
        let page = supported_formats(&EngineReport::Ready(caps), Some(&card()), None);
        assert_eq!(
            find(tab(&page, FormatTabKind::Video), "HEVC (H.265)"),
            &row(
                "HEVC (H.265)",
                FormatSupport::ServerConverts,
                "The server converts it to H.264 (AVC)"
            )
        );
        let audio = tab(&page, FormatTabKind::Audio);
        assert_eq!(
            find(audio, "Dolby TrueHD"),
            &row(
                "Dolby TrueHD",
                FormatSupport::ServerConverts,
                "The server converts it to AAC"
            )
        );
        assert_eq!(
            find(audio, "Dolby Atmos").support,
            FormatSupport::ServerConverts
        );
        let containers = tab(&page, FormatTabKind::Containers);
        assert_eq!(find(containers, "MP4").support, FormatSupport::Plays);
        assert_eq!(
            find(containers, "Matroska (MKV)").support,
            FormatSupport::ServerRepackages
        );
        let subtitles = tab(&page, FormatTabKind::Subtitles);
        // One spelling of a format is enough for its row.
        assert_eq!(
            find(subtitles, "SubRip (SRT)").support,
            FormatSupport::Plays
        );
        // Picture-based and ASS are burned in; another text format becomes the external text format the profile takes.
        for burned in ["PGS (Blu-ray)", "SubStation Alpha (ASS, SSA)"] {
            let row = find(subtitles, burned);
            assert_eq!(row.support, FormatSupport::ServerConverts, "{burned}");
            assert!(
                row.notes.ends_with(
                    "The server burns it into the picture, which means converting the video"
                ),
                "{burned}"
            );
        }
        assert_eq!(
            find(subtitles, "WebVTT"),
            &row(
                "WebVTT",
                FormatSupport::ServerConverts,
                "The server converts it to SubRip (SRT)"
            )
        );
        assert_eq!(
            find(containers, "AVI").notes,
            "The server repackages it as an HLS stream, copying the video and audio this PC decodes"
        );
    }

    #[test]
    fn dolby_audio_is_decoded_here_and_atmos_loses_its_height() {
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), Some(&card()), None);
        let audio = tab(&page, FormatTabKind::Audio);
        let names: Vec<&str> = audio.rows.iter().map(|row| row.name.as_str()).collect();
        let truehd = names
            .iter()
            .position(|name| *name == "Dolby TrueHD")
            .unwrap();
        assert_eq!(names[truehd + 1], "Dolby Atmos");
        let atmos = find(audio, "Dolby Atmos");
        assert_eq!(atmos.support, FormatSupport::Software);
        assert!(atmos.notes.contains("height information is lost"));
        assert!(atmos.notes.contains("not verified"));
        assert!(
            find(audio, "Dolby Digital Plus (E-AC-3)")
                .notes
                .contains("not passed through")
        );
        assert_eq!(find(audio, "AAC").notes, "");
    }

    #[test]
    fn hdr_follows_the_display() {
        let engine = EngineReport::Ready(mpv_windows());
        let hdr_display = DisplayFacts { hdr: true, ..sdr() };
        let on = supported_formats(&engine, None, Some(&hdr_display));
        assert!(
            tab(&on, FormatTabKind::Hdr)
                .rows
                .iter()
                .all(|row| row.support == FormatSupport::Hdr)
        );
        assert_eq!(
            find(tab(&on, FormatTabKind::Hdr), "HDR10").notes,
            "On the display this window is on, which is in HDR mode"
        );
        let off = supported_formats(&engine, None, Some(&sdr()));
        let hdr10 = find(tab(&off, FormatTabKind::Hdr), "HDR10");
        assert_eq!(hdr10.support, FormatSupport::ToneMapped);
        let unknown = supported_formats(&engine, None, None);
        let dv = find(tab(&unknown, FormatTabKind::Hdr), "Dolby Vision profile 5");
        assert_eq!(dv.support, FormatSupport::Plays);
        assert_eq!(
            dv.notes,
            "HDR or tone-mapped to SDR, depending on the display. Not checked on a device yet"
        );
    }

    #[test]
    fn unknown_values_from_the_engine_show_their_own_names() {
        let mut caps = mpv_windows();
        caps.video.push(CodecSupport::any(Codec::new("prores")));
        caps.audio.push(CodecSupport::any(Codec::new("wavpack")));
        caps.containers.push(Container::new("mxf"));
        caps.subtitles.push(SubtitleFormat::new("ttml"));
        let page = supported_formats(&EngineReport::Ready(caps), Some(&card()), None);
        assert_eq!(
            find(tab(&page, FormatTabKind::Video), "prores").support,
            FormatSupport::Software
        );
        assert_eq!(
            find(tab(&page, FormatTabKind::Audio), "wavpack").support,
            FormatSupport::Software
        );
        assert_eq!(
            find(tab(&page, FormatTabKind::Containers), "mxf").support,
            FormatSupport::Plays
        );
        assert_eq!(
            find(tab(&page, FormatTabKind::Subtitles), "ttml").support,
            FormatSupport::Plays
        );
        // Spelled as the table spells it, in another case: no second row.
        let mut caps = mpv_windows();
        caps.subtitles.push(SubtitleFormat::new("PGSSUB"));
        let page = supported_formats(&EngineReport::Ready(caps), None, None);
        assert_eq!(
            tab(&page, FormatTabKind::Subtitles).rows.len(),
            SUBTITLES.len()
        );
    }

    #[test]
    fn without_an_engine_nothing_plays() {
        let page = supported_formats(&EngineReport::Unavailable, Some(&card()), Some(&sdr()));
        assert!(page.tabs.iter().all(|tab| !tab.rows.is_empty()));
        assert!(
            page.tabs
                .iter()
                .flat_map(|tab| &tab.rows)
                .all(|row| row.support == FormatSupport::NotSupported)
        );
        assert_eq!(page.hardware_note, "");
        assert_ne!(page.description, DESCRIPTION);
    }

    #[test]
    fn a_failed_start_claims_nothing() {
        let page = supported_formats(&EngineReport::Failed, Some(&card()), Some(&sdr()));
        assert!(page.tabs.iter().all(|tab| !tab.rows.is_empty()));
        for row in page.tabs.iter().flat_map(|tab| &tab.rows) {
            assert_eq!(row.support, FormatSupport::Unknown, "{}", row.name);
            assert_eq!(row.support_label, "Couldn't check");
            assert_eq!(row.notes, UNKNOWN_NOTE);
        }
        assert_eq!(
            find(tab(&page, FormatTabKind::Video), "H.264 (AVC)").support,
            FormatSupport::Unknown
        );
        assert_eq!(
            page.description,
            "The player couldn't be started to find out what it plays. Open this page again to try again."
        );
        assert_eq!(page.hardware_note, "");
    }

    #[test]
    fn the_core_gives_every_word_the_page_shows() {
        let page = supported_formats(&EngineReport::Ready(mpv_windows()), Some(&card()), None);
        let titles: Vec<&str> = page.tabs.iter().map(|tab| tab.title.as_str()).collect();
        assert_eq!(titles, ["Video", "Audio", "Containers", "Subtitles", "HDR"]);
        let headers: Vec<&str> = page
            .tabs
            .iter()
            .map(|tab| tab.name_header.as_str())
            .collect();
        assert_eq!(headers, ["Codec", "Codec", "Format", "Format", "Format"]);
        assert!(
            page.tabs
                .iter()
                .all(|tab| tab.support_header == "On this PC" && tab.notes_header == "Notes")
        );
        for row in page.tabs.iter().flat_map(|tab| &tab.rows) {
            assert_eq!(row.support_label, row.support.label());
        }
        assert_eq!(FormatSupport::ServerRepackages.label(), "Server repackages");
    }

    #[test]
    fn the_adapter_name_stays_out_of_debug() {
        let text = format!("{:?}", card());
        assert!(!text.contains("Example Graphics"), "{text}");
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn the_hardware_table_matches_mpv_codec_names() {
        for (name, guid, _) in D3D11_PROFILES {
            assert!(
                VIDEO.iter().any(|(codec, _, _)| codec.as_str() == *name),
                "{name}"
            );
            assert_eq!(*guid, normalized_guid(guid));
        }
    }
}
