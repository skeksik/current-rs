//! The per-file route check (ADR 0025, "How the core decides"): after the server's answer, which local routes play the source with
//! the selected tracks, in the order native, remux on the device, app engine; and which engine plays a server stream.
//!
//! With the app engine as the only engine (today's players, ADR 0019), the server's answer stands as it is: it was made from exactly
//! that engine's capabilities, so a check here could only change routes the server already chose.

use current_model::{
    Codec, CodecSupport, EngineCapabilities, GapKind, MediaSource, MediaStream, Route, RouteReason,
    ServerOffer, StreamAction, StreamKind, StreamProtocol, SubtitleChoice, TrackChoice, TrackGap,
    VideoRangeType,
};

use crate::engines::{DisplayKind, EnginePreference, EngineProfile, Engines};

/// The video codecs a server transcode may send: those of `current-jellyfin`'s transcoding profiles (ADR 0019 choice 2: fMP4 HEVC
/// then H.264, TS H.264), which the profile keeps to what the device decodes. A transcoded stream goes to the native engine only when
/// it decodes each of them that the device profile has, since the server picks among them.
const TRANSCODE_VIDEO: &[Codec] = &[Codec::HEVC, Codec::H264];
/// The audio codecs of the same profiles (fMP4: AAC, AC-3, E-AC-3, Opus, FLAC; TS adds MP3).
const TRANSCODE_AUDIO: &[Codec] = &[
    Codec::AAC,
    Codec::AC3,
    Codec::EAC3,
    Codec::OPUS,
    Codec::FLAC,
    Codec::MP3,
];

/// Subtitle formats that need rendering (ADR 0025, choice 1): picture formats, and ASS/SSA whose styling must be kept. On the native
/// route only where its engine lists them (PGS on Windows, departure 3); otherwise the file goes to the app engine. By the server's
/// names, compared without case.
const RENDERED_SUBTITLES: &[&str] = &[
    "ass", "ssa", "pgssub", "pgs", "dvdsub", "dvbsub", "xsub", "vobsub",
];

/// What decides the route besides the source and the engines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RouteContext {
    pub display: DisplayKind,
    /// The "Player" setting.
    pub preference: EnginePreference,
    /// "Play with app engine" for this playback.
    pub app_engine_chosen: bool,
}

/// A route the check allows, with why it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteChoice {
    pub route: Route,
    pub reasons: Vec<RouteReason>,
    /// With [`RouteReason::NATIVE_LACKS`]: what the native engine lacks.
    pub native_lacks: Vec<TrackGap>,
}

impl RouteChoice {
    fn new(route: Route, reasons: Vec<RouteReason>, native_lacks: Vec<TrackGap>) -> Self {
        Self {
            route,
            reasons,
            native_lacks,
        }
    }
}

/// The local routes that play `source` with `tracks` (the user's choice with the server's defaults filled in), in the order to try
/// them, leaving out `tried` (except with the app engine only, below). Empty when none does: the plan is then asked again without direct play (ADR 0025, step 4).
///
/// With a native engine: native when it plays the container and every selected track (and the platform's display rules allow it);
/// else the remux, when only the container is missing and the remux reads it; else the app engine. A display rule (departures 2 and
/// 8) puts the app engine before the native route instead of removing it. With "Always app engine" or "Play with app engine", the
/// app engine only.
pub fn local_routes(
    source: &MediaSource,
    tracks: TrackChoice,
    engines: &Engines,
    context: RouteContext,
    tried: &[Route],
) -> Vec<RouteChoice> {
    let Some(native) = &engines.native else {
        // The app engine only: the server's answer stands, with no reasons, even after the app engine failed (ADR 0019: a server
        // that offers direct play again when asked not to gets direct play again, as before ADR 0025).
        return vec![RouteChoice::new(Route::AppEngine, Vec::new(), Vec::new())];
    };
    let app_plays = gaps(&engines.app.capabilities, source, tracks).is_empty();
    let chosen = if context.app_engine_chosen {
        Some(RouteReason::USER_CHOSE_APP_ENGINE)
    } else if context.preference == EnginePreference::AlwaysAppEngine {
        Some(RouteReason::SETTING_ALWAYS_APP_ENGINE)
    } else {
        None
    };
    if let Some(reason) = chosen {
        let routes = if app_plays {
            vec![RouteChoice::new(Route::AppEngine, vec![reason], Vec::new())]
        } else {
            Vec::new()
        };
        return untried(routes, tried);
    }

    let native_gaps = gaps(&native.capabilities, source, tracks);
    let display_reasons = display_reasons(source, engines, context.display);
    let subtitle_rendered = selected_subtitle(source, tracks)
        .filter(|_| native_gaps.iter().any(|gap| gap.kind == GapKind::Subtitle))
        .is_some_and(needs_rendering);
    let only_container =
        !native_gaps.is_empty() && native_gaps.iter().all(|gap| gap.kind == GapKind::Container);
    let dolby_vision_7 = source
        .video()
        .and_then(|stream| stream.video.as_ref())
        .is_some_and(|video| video.dv_profile == Some(7));
    let remux_reads = engines.remux.as_ref().is_some_and(|remux| {
        container_names(source).any(|name| {
            remux
                .containers
                .iter()
                .any(|container| container.as_str().eq_ignore_ascii_case(name))
        })
    });

    let mut routes = Vec::new();
    let mut app_reasons = Vec::new();
    let mut lacks = Vec::new();
    let mut native_route = None;
    if native_gaps.is_empty() {
        native_route = Some(RouteChoice::new(
            Route::Native,
            vec![RouteReason::NATIVE_FULLY_COMPATIBLE],
            Vec::new(),
        ));
    } else if only_container && remux_reads && dolby_vision_7 {
        app_reasons.push(RouteReason::DOLBY_VISION_PROFILE_7);
    } else if only_container && remux_reads {
        native_route = Some(RouteChoice::new(
            Route::DeviceRemux,
            vec![RouteReason::KEEPS_ORIGINAL_STREAMS],
            Vec::new(),
        ));
    } else {
        if subtitle_rendered {
            app_reasons.push(RouteReason::SUBTITLE_NEEDS_RENDERING);
        }
        lacks = native_gaps
            .into_iter()
            .filter(|gap| !(subtitle_rendered && gap.kind == GapKind::Subtitle))
            .collect();
        if !lacks.is_empty() {
            app_reasons.insert(0, RouteReason::NATIVE_LACKS);
        }
    }
    match native_route {
        Some(native_route) if display_reasons.is_empty() => {
            routes.push(native_route);
            if app_plays {
                routes.push(RouteChoice::new(Route::AppEngine, Vec::new(), Vec::new()));
            }
        }
        Some(native_route) => {
            // A display rule: the app engine first, the native route after it.
            if app_plays {
                routes.push(RouteChoice::new(
                    Route::AppEngine,
                    display_reasons,
                    Vec::new(),
                ));
            }
            routes.push(native_route);
        }
        None => {
            if app_plays {
                routes.push(RouteChoice::new(Route::AppEngine, app_reasons, lacks));
            }
        }
    }
    untried(routes, tried)
}

/// The engines that play the server's stream in `offer`, native first, leaving out `tried` (ADR 0025: "A server stream is played by
/// the first engine, native first, whose capabilities include the stream's protocol, container and codecs"). The app engine takes
/// every stream the device profile asks for, as today; with no native engine it is the only one.
pub fn server_routes(
    offer: &ServerOffer,
    source: &MediaSource,
    tracks: TrackChoice,
    engines: &Engines,
    context: RouteContext,
    tried: &[Route],
) -> Vec<Route> {
    let mut routes = Vec::new();
    if let Some(native) = &engines.native {
        let app_first =
            context.app_engine_chosen || context.preference == EnginePreference::AlwaysAppEngine;
        let copies_video = matches!(
            offer,
            ServerOffer::Stream {
                video: StreamAction::Copy,
                ..
            }
        );
        let display_rule =
            copies_video && !display_reasons(source, engines, context.display).is_empty();
        if !app_first
            && !display_rule
            && takes_stream(native, &engines.device_profile(), offer, source, tracks)
        {
            routes.extend(Route::server_stream(
                offer,
                current_model::EngineRole::Native,
            ));
        }
    }
    routes.extend(Route::server_stream(offer, current_model::EngineRole::App));
    routes.retain(|route| !tried.contains(route));
    routes
}

fn untried(mut routes: Vec<RouteChoice>, tried: &[Route]) -> Vec<RouteChoice> {
    routes.retain(|choice| !tried.contains(&choice.route));
    routes
}

/// Whether `engine` plays the server's stream: its protocol and container, the streams it copies, and every codec a transcode may
/// send (of those the device profile has).
fn takes_stream(
    engine: &EngineProfile,
    profile: &EngineCapabilities,
    offer: &ServerOffer,
    source: &MediaSource,
    tracks: TrackChoice,
) -> bool {
    let ServerOffer::Stream {
        protocol,
        container,
        video,
        audio,
    } = offer
    else {
        return false;
    };
    let capabilities = &engine.capabilities;
    if *protocol == StreamProtocol::Hls && !engine.hls {
        return false;
    }
    let container_ok = container.as_deref().is_some_and(|name| {
        capabilities
            .containers
            .iter()
            .any(|known| known.as_str().eq_ignore_ascii_case(name))
    });
    let video_ok = match video {
        StreamAction::Copy => source
            .video()
            .is_none_or(|stream| plays(&capabilities.video, stream)),
        StreamAction::Transcode => TRANSCODE_VIDEO
            .iter()
            .filter(|codec| profile.decodes_video(codec))
            .all(|codec| capabilities.decodes_video(codec)),
    };
    let audio_ok = match audio {
        StreamAction::Copy => {
            selected_audio(source, tracks).is_none_or(|stream| plays(&capabilities.audio, stream))
        }
        StreamAction::Transcode => TRANSCODE_AUDIO
            .iter()
            .filter(|codec| profile.decodes_audio(codec))
            .all(|codec| capabilities.decodes_audio(codec)),
    };
    container_ok && video_ok && audio_ok
}

/// What the platform's display rules say against the native route for this source, as reasons for the app engine.
fn display_reasons(
    source: &MediaSource,
    engines: &Engines,
    display: DisplayKind,
) -> Vec<RouteReason> {
    let Some(video) = source.video().and_then(|stream| stream.video.as_ref()) else {
        return Vec::new();
    };
    let range = video.range_type.as_ref();
    let dolby_vision = video.dv_profile.is_some()
        || range.is_some_and(|range| range.as_str().to_ascii_uppercase().starts_with("DOVI"));
    let mut reasons = Vec::new();
    if engines.rules.dolby_vision_on_app_engine && dolby_vision {
        reasons.push(RouteReason::DOLBY_VISION_ON_APP_ENGINE);
    }
    // Departure 8: PQ only (HDR10, HDR10+, Dolby Vision); HLG looked right natively on the SDR display (W1). An unknown display keeps
    // the rule unchanged; the Windows UI always reports it (W4).
    if engines.rules.pq_on_sdr_on_app_engine
        && display == DisplayKind::Sdr
        && is_pq(range, dolby_vision)
    {
        reasons.push(RouteReason::HDR_ON_SDR_DISPLAY);
    }
    reasons
}

fn is_pq(range: Option<&VideoRangeType>, dolby_vision: bool) -> bool {
    let Some(range) = range else {
        return dolby_vision;
    };
    let pq = [
        VideoRangeType::HDR10,
        VideoRangeType::HDR10_PLUS,
        VideoRangeType::DOVI,
        VideoRangeType::DOVI_WITH_HDR10,
        VideoRangeType::DOVI_WITH_EL,
        VideoRangeType::DOVI_WITH_HDR10_PLUS,
        VideoRangeType::DOVI_WITH_EL_HDR10_PLUS,
    ];
    pq.iter()
        .any(|known| known.as_str().eq_ignore_ascii_case(range.as_str()))
        // Dolby Vision on an HLG or SDR base layer is not PQ; an unknown Dolby Vision kind is taken as PQ.
        || (dolby_vision
            && ![
                VideoRangeType::DOVI_WITH_HLG,
                VideoRangeType::DOVI_WITH_SDR,
                VideoRangeType::HLG,
                VideoRangeType::SDR,
            ]
            .iter()
            .any(|known| known.as_str().eq_ignore_ascii_case(range.as_str())))
}

/// What an engine with `capabilities` lacks to play `source` with `tracks`: the container, the video, the selected audio, the
/// selected subtitle. A stream whose codec the server does not name counts as lacking.
pub fn gaps(
    capabilities: &EngineCapabilities,
    source: &MediaSource,
    tracks: TrackChoice,
) -> Vec<TrackGap> {
    let mut gaps = Vec::new();
    let container_ok = container_names(source).any(|name| {
        capabilities
            .containers
            .iter()
            .any(|known| known.as_str().eq_ignore_ascii_case(name))
    });
    if !container_ok {
        gaps.push(TrackGap {
            kind: GapKind::Container,
            name: source.container.clone(),
        });
    }
    if let Some(video) = source.video()
        && !plays(&capabilities.video, video)
    {
        gaps.push(gap(GapKind::Video, video));
    }
    if let Some(audio) = selected_audio(source, tracks)
        && !plays(&capabilities.audio, audio)
    {
        gaps.push(gap(GapKind::Audio, audio));
    }
    if let Some(subtitle) = selected_subtitle(source, tracks) {
        let named = subtitle.codec.as_ref().is_some_and(|codec| {
            capabilities
                .subtitles
                .iter()
                .any(|format| format.as_str().eq_ignore_ascii_case(codec.as_str()))
        });
        if !named || (subtitle.is_external && !capabilities.external_subtitles) {
            gaps.push(gap(GapKind::Subtitle, subtitle));
        }
    }
    gaps
}

fn gap(kind: GapKind, stream: &MediaStream) -> TrackGap {
    TrackGap {
        kind,
        name: stream.codec.as_ref().map(|codec| codec.as_str().to_owned()),
    }
}

/// The server's container name, which may be a list (`mov,mp4,m4a`).
fn container_names(source: &MediaSource) -> impl Iterator<Item = &str> {
    source
        .container
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn selected_audio(source: &MediaSource, tracks: TrackChoice) -> Option<&MediaStream> {
    match tracks.audio {
        Some(index) => source.stream(index),
        None => source.streams_of(&StreamKind::AUDIO).next(),
    }
}

fn selected_subtitle(source: &MediaSource, tracks: TrackChoice) -> Option<&MediaStream> {
    match tracks.subtitle {
        SubtitleChoice::Stream(index) => source.stream(index),
        SubtitleChoice::Default | SubtitleChoice::Off => None,
    }
}

fn needs_rendering(stream: &MediaStream) -> bool {
    stream.codec.as_ref().is_some_and(|codec| {
        RENDERED_SUBTITLES
            .iter()
            .any(|name| name.eq_ignore_ascii_case(codec.as_str()))
    })
}

/// Whether `supports` decodes `stream` within its limits (bit depth, profile, level and range, where it has them).
fn plays(supports: &[CodecSupport], stream: &MediaStream) -> bool {
    let Some(codec) = &stream.codec else {
        return false;
    };
    let Some(support) = supports
        .iter()
        .find(|support| support.codec.as_str().eq_ignore_ascii_case(codec.as_str()))
    else {
        return false;
    };
    let video = stream.video.as_ref();
    let bit_depth_ok = match (
        support.max_bit_depth,
        video.and_then(|video| video.bit_depth),
    ) {
        (Some(max), Some(depth)) => depth <= u32::from(max),
        _ => true,
    };
    let profile_ok = match (&support.profiles, &stream.profile) {
        (Some(profiles), Some(profile)) => profiles
            .iter()
            .any(|known| known.eq_ignore_ascii_case(profile)),
        _ => true,
    };
    let level_ok = match (support.level, stream.level) {
        (Some(max), Some(level)) => level <= f64::from(max),
        _ => true,
    };
    let range_ok = match (
        &support.ranges,
        video.and_then(|video| video.range_type.as_ref()),
    ) {
        (Some(ranges), Some(range)) => ranges
            .iter()
            .any(|known| known.as_str().eq_ignore_ascii_case(range.as_str())),
        _ => true,
    };
    bit_depth_ok && profile_ok && level_ok && range_ok
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
