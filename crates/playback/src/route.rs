//! The per-file route check (ADR 0025, "How the core decides"): after the server's answer, which local routes play the source with
//! the selected tracks, in the order native, remux on the device, app engine; and which engine plays a server stream.
//!
//! With the app engine as the only engine (today's players, ADR 0019), the server's answer stands as it is: it was made from exactly
//! that engine's capabilities, so a check here could only change routes the server already chose.

use current_model::{
    Codec, CodecSupport, EngineCapabilities, GapKind, HowToPlay, MediaSource, MediaStream, Route,
    RouteReason, ServerOffer, StreamAction, StreamKind, StreamProtocol, SubtitleChoice,
    SubtitleDelivery, TrackChoice, TrackGap, VideoRangeType,
};

use crate::engines::{DisplayKind, EngineProfile, Engines};

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

/// Subtitle formats that need rendering (ADR 0025, choice 1): picture formats, and ASS/SSA whose styling must be kept. They go to the
/// app engine, except those the platform's rules keep native (PGS on Windows, departure 3), and never through the remux. By the
/// server's names, compared without case.
const RENDERED_SUBTITLES: &[&str] = &[
    "ass", "ssa", "pgssub", "pgs", "dvdsub", "dvbsub", "xsub", "vobsub",
];

/// What decides the route besides the source and the engines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RouteContext {
    pub display: DisplayKind,
    /// "How to play" (ADR 0025, choice 3 as amended): the playback's own choice, else Settings'.
    pub how: HowToPlay,
    /// `how` was chosen in the player for this playback, not taken from Settings.
    pub chosen_here: bool,
    /// The native engine ignores the speed chosen with this playback's tracks (ADR 0025, "Track changes"): the app engine plays it.
    pub speed_not_supported: bool,
}

impl RouteContext {
    /// The reason a route at the chosen step carries; `None` with Auto.
    pub fn chosen_reason(self) -> Option<RouteReason> {
        self.how.step()?;
        Some(if self.chosen_here {
            RouteReason::USER_CHOSE_ROUTE
        } else {
            RouteReason::SETTING_CHOSE_ROUTE
        })
    }

    /// The app engine plays whatever the native engine could, server streams included: "How to play" starts at it, or a speed the
    /// native engine ignores.
    fn app_first(self) -> bool {
        self.how == HowToPlay::AppEngine || self.speed_not_supported
    }
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

/// The local routes that play `source` with `tracks`, in the order to try them, leaving out `tried` (except with the app engine only,
/// below). The caller resolves [`SubtitleChoice::Default`] to the server's pick first (`planner::resolve_tracks`); here it counts as
/// no subtitle. Empty when no route plays the file: the plan is then asked again without direct play (ADR 0025, step 4).
///
/// With a native engine: native when it plays the container and every selected track, and the subtitle rule and the platform's
/// display rules allow it; else the remux, when only the container is missing, the remux reads it, and neither Dolby Vision profile 7
/// nor a subtitle that needs rendering is in the way; else the app engine. A display rule (departures 2 and 8) puts the app engine
/// before the native route instead of removing it. With "Always app engine", "Play with app engine" or a speed the native engine
/// ignores, the app engine only. A route
/// chosen because one before it failed says so ([`RouteReason::PREVIOUS_ROUTE_FAILED`]).
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
    // The server offered direct play from a profile the app engine is part of: a codec or container it does not name, the app
    // engine plays as before ADR 0025, the native engine is not trusted with.
    let app_plays = gaps(&engines.app.capabilities, source, tracks, Unnamed::Plays).is_empty();
    // "How to play" at a server step: no local route; the planner asks the server for its stream.
    if context.how.is_server() {
        return Vec::new();
    }
    if context.app_first() {
        // A speed says so; the app engine chosen is said by the planner ([`RouteContext::chosen_reason`]).
        let reasons = if context.speed_not_supported {
            vec![RouteReason::SPEED_NOT_SUPPORTED]
        } else {
            Vec::new()
        };
        let routes = if app_plays {
            vec![RouteChoice::new(Route::AppEngine, reasons, Vec::new())]
        } else {
            Vec::new()
        };
        return untried(routes, tried);
    }

    let mut native_gaps = gaps(&native.capabilities, source, tracks, Unnamed::Lacks);
    // A subtitle file: the platform's rules decide which formats the native engine takes as one, when they name any.
    if let Some(stream) = selected_subtitle(source, tracks).filter(|stream| stream.is_external)
        && !engines.rules.native_external_subtitles.is_empty()
    {
        native_gaps.retain(|gap| gap.kind != GapKind::Subtitle);
        if !takes_as_file(stream, engines) {
            native_gaps.push(gap(GapKind::Subtitle, stream));
        }
    }
    let display_reasons = display_reasons(source, engines, context.display);
    // Choice 1, from the format itself: a subtitle that needs rendering keeps the file off the remux, and off the native route
    // unless the platform's rules keep that format native.
    let rendered = selected_subtitle(source, tracks).filter(|stream| needs_rendering(stream));
    let rendered_off_native = rendered.is_some_and(|stream| !kept_native(stream, engines));
    if rendered_off_native {
        native_gaps.retain(|gap| gap.kind != GapKind::Subtitle);
    }
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

    let native_route = if native_gaps.is_empty() && !rendered_off_native {
        Some(RouteChoice::new(
            Route::Native,
            vec![RouteReason::NATIVE_FULLY_COMPATIBLE],
            Vec::new(),
        ))
    } else if only_container && remux_reads && !dolby_vision_7 && rendered.is_none() {
        Some(RouteChoice::new(
            Route::DeviceRemux,
            vec![RouteReason::KEEPS_ORIGINAL_STREAMS],
            Vec::new(),
        ))
    } else {
        None
    };
    // What the pinned order below needs after the match takes these.
    let display_rules = !display_reasons.is_empty();
    let only_container_gaps = native_gaps.iter().all(|gap| gap.kind == GapKind::Container);
    let mut routes = Vec::new();
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
        None if app_plays => {
            let mut reasons = Vec::new();
            if !native_gaps.is_empty() {
                reasons.push(RouteReason::NATIVE_LACKS);
            }
            let remux_was_the_way = only_container && remux_reads;
            if rendered_off_native || (remux_was_the_way && rendered.is_some()) {
                reasons.push(RouteReason::SUBTITLE_NEEDS_RENDERING);
            }
            if remux_was_the_way && dolby_vision_7 {
                reasons.push(RouteReason::DOLBY_VISION_PROFILE_7);
            }
            routes.push(RouteChoice::new(Route::AppEngine, reasons, native_gaps));
        }
        None => {}
    }
    // "How to play" at the native route or the remux: the order from that step down, never above it. A display rule rules the native
    // engine out (its reasons stay with the app engine), and the remux chosen is taken whenever it can play the file.
    if let Some(pinned) = context.how.step() {
        if display_rules {
            routes.retain(|choice| choice.route.engine() != current_model::EngineRole::Native);
        }
        let remux_plays = remux_reads
            && !dolby_vision_7
            && rendered.is_none()
            && only_container_gaps
            && !display_rules;
        if context.how == HowToPlay::DeviceRemux
            && remux_plays
            && !routes
                .iter()
                .any(|choice| choice.route == Route::DeviceRemux)
        {
            routes.insert(
                0,
                RouteChoice::new(Route::DeviceRemux, Vec::new(), Vec::new()),
            );
        }
        routes.retain(|choice| choice.route.step() >= pinned);
    }
    untried(routes, tried)
}

/// The engines that play the server's stream in `offer`, native first, leaving out an engine whose server stream is in `tried` (ADR 0025: "A server stream is played by
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
        let app_first = context.app_first();
        let copies_video = matches!(
            offer,
            ServerOffer::Stream {
                video: StreamAction::Copy,
                ..
            }
        );
        // Departure 2 on a copied picture only (a transcode is the server's picture, not the source's Dolby Vision). Departure 8 on
        // any stream from a PQ source: the server may keep HDR in an HEVC transcode (not verified), and the app engine plays every
        // stream.
        let dolby_vision = copies_video
            && display_reasons(source, engines, context.display)
                .contains(&RouteReason::DOLBY_VISION_ON_APP_ENGINE);
        let pq_on_sdr = display_reasons(source, engines, context.display)
            .contains(&RouteReason::HDR_ON_SDR_DISPLAY);
        // The subtitle rule: on a server stream the native engine shows a subtitle only burned into the picture or as a file the
        // core fetches (`External`) in a format it takes as one; one inside the stream (`Embed`, `Hls`, or no delivery named) or
        // any other leaves the stream to the app engine.
        let subtitle_off_native = selected_subtitle(source, tracks)
            .filter(|stream| stream.subtitle_delivery.as_ref() != Some(&SubtitleDelivery::ENCODE))
            .is_some_and(|stream| {
                let as_file = stream.is_external
                    || stream.subtitle_delivery.as_ref() == Some(&SubtitleDelivery::EXTERNAL);
                !(as_file && shows_as_file(stream, native, engines))
            });
        if !app_first
            && !dolby_vision
            && !pq_on_sdr
            && !subtitle_off_native
            && takes_stream(native, &engines.device_profile(), offer, source, tracks)
        {
            routes.extend(Route::server_stream(
                offer,
                current_model::EngineRole::Native,
            ));
        }
    }
    routes.extend(Route::server_stream(offer, current_model::EngineRole::App));
    // An engine that failed on a server stream in this play is not given another one (the new answer may differ in what it copies).
    routes.retain(|route| {
        !tried
            .iter()
            .any(|failed| !failed.is_local() && failed.engine() == route.engine())
    });
    routes
}

/// `routes` without those in `tried`; the first one left says why, when a route before it failed.
fn untried(routes: Vec<RouteChoice>, tried: &[Route]) -> Vec<RouteChoice> {
    let skipped = routes
        .iter()
        .take_while(|choice| tried.contains(&choice.route))
        .count();
    let mut routes: Vec<RouteChoice> = routes
        .into_iter()
        .filter(|choice| !tried.contains(&choice.route))
        .collect();
    if skipped > 0
        && let Some(first) = routes.first_mut()
    {
        first.reasons.insert(0, RouteReason::PREVIOUS_ROUTE_FAILED);
    }
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

/// Whether departure 8 sends this source to the app engine on `display` (a PQ source on an SDR display, on a platform with the rule):
/// the controller re-plans when a display change turns it (ADR 0025, "Track changes").
pub fn hdr_on_sdr(source: &MediaSource, engines: &Engines, display: DisplayKind) -> bool {
    display_reasons(source, engines, display).contains(&RouteReason::HDR_ON_SDR_DISPLAY)
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
    // Every `DOVI*` range type counts, `DOVIInvalid` (the server could not read the Dolby Vision metadata) included, on purpose: the
    // app engine shows the base layer as well as the native one does.
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
        // Dolby Vision on an HLG or SDR base layer is not PQ; an unknown Dolby Vision kind, `DOVIInvalid` included, is taken as PQ on
        // purpose: the app engine plays it either way, the native one might clip it.
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

/// What a container, codec or format the server does not name means for an engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unnamed {
    /// It lacks it (the native engine: nothing says it plays it).
    Lacks,
    /// It plays it (the app engine, when the server offered direct play from a profile it is part of).
    Plays,
}

/// What an engine with `capabilities` lacks to play `source` with `tracks`: the container, the video, the selected audio, the
/// selected subtitle.
pub fn gaps(
    capabilities: &EngineCapabilities,
    source: &MediaSource,
    tracks: TrackChoice,
    unnamed: Unnamed,
) -> Vec<TrackGap> {
    let mut gaps = Vec::new();
    let mut names = container_names(source).peekable();
    let container_ok = if names.peek().is_none() {
        unnamed == Unnamed::Plays
    } else {
        names.any(|name| {
            capabilities
                .containers
                .iter()
                .any(|known| known.as_str().eq_ignore_ascii_case(name))
        })
    };
    if !container_ok {
        // The file's container as the user knows it: the server's first name for it (`mov,mp4,m4a,...` is an MP4 file to them).
        gaps.push(TrackGap {
            kind: GapKind::Container,
            name: container_names(source).next().map(container_shown),
        });
    }
    let trusted = |stream: &MediaStream| stream.codec.is_none() && unnamed == Unnamed::Plays;
    if let Some(video) = source.video()
        && !trusted(video)
        && !plays(&capabilities.video, video)
    {
        gaps.push(gap(GapKind::Video, video));
    }
    if let Some(audio) = selected_audio(source, tracks)
        && !trusted(audio)
        && !plays(&capabilities.audio, audio)
    {
        gaps.push(gap(GapKind::Audio, audio));
    }
    if let Some(subtitle) = selected_subtitle(source, tracks) {
        let named = trusted(subtitle)
            || subtitle.codec.as_ref().is_some_and(|codec| {
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

/// The name a user knows a container by, from one of the server's names: FFmpeg's `mov` demuxer reads MP4 files too, and the server
/// names them `mov,mp4,m4a,3gp,3g2,mj2`; `matroska` is MKV.
fn container_shown(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "mov" => "mp4".to_owned(),
        "matroska" => "mkv".to_owned(),
        other => other.to_owned(),
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

/// Whether the platform's rules keep this subtitle that needs rendering on the native route (departure 3).
fn kept_native(stream: &MediaStream, engines: &Engines) -> bool {
    stream.codec.as_ref().is_some_and(|codec| {
        engines
            .rules
            .native_rendered_subtitles
            .iter()
            .any(|name| name.eq_ignore_ascii_case(codec.as_str()))
    })
}

/// Whether the native route shows this subtitle: a format its engine lists, and, for one that needs rendering, one the platform's
/// rules keep native.
fn shows_natively(stream: &MediaStream, native: &EngineProfile, engines: &Engines) -> bool {
    let listed = stream.codec.as_ref().is_some_and(|codec| {
        native
            .capabilities
            .subtitles
            .iter()
            .any(|format| format.as_str().eq_ignore_ascii_case(codec.as_str()))
    });
    listed && (!needs_rendering(stream) || kept_native(stream, engines))
}

/// Whether the native route shows this subtitle as a file: a format the platform's rules name, or, when they name none, one its
/// engine lists.
fn shows_as_file(stream: &MediaStream, native: &EngineProfile, engines: &Engines) -> bool {
    if engines.rules.native_external_subtitles.is_empty() {
        shows_natively(stream, native, engines)
    } else {
        takes_as_file(stream, engines)
    }
}

/// Whether the native engine takes this subtitle as a file: a format the platform's rules name, or any when they name none.
fn takes_as_file(stream: &MediaStream, engines: &Engines) -> bool {
    let names = engines.rules.native_external_subtitles;
    names.is_empty()
        || stream.codec.as_ref().is_some_and(|codec| {
            names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(codec.as_str()))
        })
}

fn needs_rendering(stream: &MediaStream) -> bool {
    stream.codec.as_ref().is_some_and(|codec| {
        RENDERED_SUBTITLES
            .iter()
            .any(|name| name.eq_ignore_ascii_case(codec.as_str()))
    })
}

/// Whether `supports` decodes `stream` within its limits (bit depth, profile, level, range and channels, where it has them).
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
    let channels_ok = match (
        support.max_channels,
        stream.audio.as_ref().and_then(|audio| audio.channels),
    ) {
        (Some(max), Some(channels)) => channels <= max,
        _ => true,
    };
    bit_depth_ok && profile_ok && level_ok && range_ok && channels_ok
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
