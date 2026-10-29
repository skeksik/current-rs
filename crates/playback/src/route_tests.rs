//! The per-file route check (ADR 0025) with made-up engines: a native engine shaped like Windows' (MKV, MP4, TS; H.264 up to 8 bit,
//! HEVC, AV1; no TrueHD or DTS; PGS but no ASS) and an app engine shaped like libmpv.

use std::time::Duration;

use current_model::{
    AudioFacts, Codec, CodecSupport, Container, EngineCapabilities, EngineRole, GapKind,
    MediaSource, MediaStream, Route, RouteReason, ServerOffer, StreamAction, StreamKind,
    StreamProtocol, SubtitleChoice, SubtitleDelivery, SubtitleFormat, TrackChoice, TrackGap,
    VideoFacts, VideoRangeType,
};

use super::*;
use crate::engines::{
    DeviceRemux, DisplayKind, EnginePreference, EngineProfile, Engines, PlatformRules,
};

fn native_caps() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4, Container::TS],
        video: vec![
            CodecSupport {
                max_bit_depth: Some(8),
                ..CodecSupport::any(Codec::H264)
            },
            CodecSupport::any(Codec::HEVC),
            CodecSupport::any(Codec::AV1),
        ],
        audio: vec![
            CodecSupport::any(Codec::AAC),
            CodecSupport::any(Codec::AC3),
            CodecSupport::any(Codec::EAC3),
            CodecSupport::any(Codec::FLAC),
            // W1: multichannel Opus failed natively.
            CodecSupport {
                max_channels: Some(2),
                ..CodecSupport::any(Codec::OPUS)
            },
        ],
        subtitles: vec![
            SubtitleFormat::SUBRIP,
            SubtitleFormat::SRT,
            SubtitleFormat::PGSSUB,
        ],
        external_subtitles: true,
    }
}

fn app_caps() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![
            Container::MKV,
            Container::MP4,
            Container::TS,
            Container::AVI,
        ],
        video: [Codec::H264, Codec::HEVC, Codec::AV1, Codec::VP9]
            .map(CodecSupport::any)
            .to_vec(),
        audio: [
            Codec::AAC,
            Codec::AC3,
            Codec::EAC3,
            Codec::TRUEHD,
            Codec::DTS,
            Codec::FLAC,
            Codec::OPUS,
        ]
        .map(CodecSupport::any)
        .to_vec(),
        subtitles: vec![
            SubtitleFormat::ASS,
            SubtitleFormat::SSA,
            SubtitleFormat::SUBRIP,
            SubtitleFormat::SRT,
            SubtitleFormat::PGSSUB,
            SubtitleFormat::DVDSUB,
        ],
        external_subtitles: true,
    }
}

fn windows() -> Engines {
    Engines {
        native: Some(EngineProfile::new(native_caps(), true)),
        app: EngineProfile::new(app_caps(), true),
        remux: None,
        rules: PlatformRules::WINDOWS,
    }
}

/// A platform with a remux on the device for MKV, whose native engine does not read MKV (Apple's shape, ADR 0025).
fn with_remux() -> Engines {
    let mut native = native_caps();
    native
        .containers
        .retain(|container| *container != Container::MKV);
    Engines {
        native: Some(EngineProfile::new(native, true)),
        app: EngineProfile::new(app_caps(), true),
        remux: Some(DeviceRemux {
            containers: vec![Container::MKV],
        }),
        rules: PlatformRules::default(),
    }
}

fn plain(index: u32, kind: StreamKind, codec: &str) -> MediaStream {
    MediaStream {
        index,
        kind,
        codec: Some(Codec::new(codec)),
        profile: None,
        level: None,
        language: None,
        title: None,
        display_title: None,
        is_default: false,
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
            width: Some(3840),
            height: Some(2160),
            bit_depth: Some(bit_depth),
            frame_rate: Some(23.976),
            reference_frame_rate: None,
            range: None,
            range_type: Some(range),
            dv_profile,
        }),
        ..plain(0, StreamKind::VIDEO, codec)
    }
}

fn audio(index: u32, codec: &str) -> MediaStream {
    MediaStream {
        audio: Some(AudioFacts {
            channels: Some(6),
            layout: None,
            sample_rate: Some(48_000),
        }),
        ..plain(index, StreamKind::AUDIO, codec)
    }
}

/// An MKV with HEVC SDR video 0, E-AC-3 audio 1, TrueHD audio 2, subtitles SubRip 3, PGS 4, ASS 5, DVD 6, and an external SubRip 7.
fn movie() -> MediaSource {
    let mut external = plain(7, StreamKind::SUBTITLE, "srt");
    external.is_external = true;
    MediaSource {
        id: "s1".into(),
        name: None,
        container: Some("mkv".into()),
        size: None,
        bitrate: Some(20_000_000),
        duration: Some(Duration::from_secs(3600)),
        streams: vec![
            video("hevc", 8, VideoRangeType::SDR, None),
            audio(1, "eac3"),
            audio(2, "truehd"),
            plain(3, StreamKind::SUBTITLE, "subrip"),
            plain(4, StreamKind::SUBTITLE, "PGSSUB"),
            plain(5, StreamKind::SUBTITLE, "ass"),
            plain(6, StreamKind::SUBTITLE, "dvdsub"),
            external,
        ],
        default_audio: Some(1),
        default_subtitle: None,
        has_segments: false,
        is_remote: false,
    }
}

fn with_video(stream: MediaStream) -> MediaSource {
    let mut source = movie();
    source.streams[0] = stream;
    source
}

fn tracks(audio: u32, subtitle: Option<u32>) -> TrackChoice {
    TrackChoice {
        audio: Some(audio),
        subtitle: subtitle.map_or(SubtitleChoice::Off, SubtitleChoice::Stream),
    }
}

fn automatic(display: DisplayKind) -> RouteContext {
    RouteContext {
        display,
        ..RouteContext::default()
    }
}

/// The routes in order, with the first route's reasons.
fn check(
    source: &MediaSource,
    tracks: TrackChoice,
    engines: &Engines,
    context: RouteContext,
) -> (Vec<Route>, Vec<RouteReason>, Vec<TrackGap>) {
    let choices = local_routes(source, tracks, engines, context, &[]);
    let order = choices.iter().map(|choice| choice.route.clone()).collect();
    let (reasons, lacks) = choices
        .into_iter()
        .next()
        .map(|choice| (choice.reasons, choice.native_lacks))
        .unwrap_or_default();
    (order, reasons, lacks)
}

fn gap(kind: GapKind, name: &str) -> TrackGap {
    TrackGap {
        kind,
        name: Some(name.into()),
    }
}

#[test]
fn native_first_when_it_plays_every_selected_track() {
    let (order, reasons, lacks) = check(
        &movie(),
        tracks(1, Some(3)),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::Native, Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_FULLY_COMPATIBLE]);
    assert!(lacks.is_empty());
    // An external text subtitle too: the core fetches it and hands it over.
    let (order, ..) = check(
        &movie(),
        tracks(1, Some(7)),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::Native);
}

#[test]
fn a_track_the_native_engine_lacks_goes_to_the_app_engine() {
    let (order, reasons, lacks) = check(
        &movie(),
        tracks(2, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(lacks, vec![gap(GapKind::Audio, "truehd")]);
}

#[test]
fn a_limit_counts_h264_high_10_goes_to_the_app_engine() {
    let source = with_video(video("h264", 10, VideoRangeType::SDR, None));
    let (order, reasons, lacks) = check(
        &source,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(lacks, vec![gap(GapKind::Video, "h264")]);
    let eight_bit = with_video(video("h264", 8, VideoRangeType::SDR, None));
    let (order, ..) = check(
        &eight_bit,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::Native);
}

#[test]
fn pgs_stays_native_where_the_native_engine_shows_it() {
    // Departure 3: Windows lists PGS.
    let (order, ..) = check(
        &movie(),
        tracks(1, Some(4)),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::Native, Route::AppEngine]);
}

#[test]
fn ass_and_dvd_subtitles_need_rendering_on_the_app_engine() {
    for subtitle in [5, 6] {
        let (order, reasons, lacks) = check(
            &movie(),
            tracks(1, Some(subtitle)),
            &windows(),
            automatic(DisplayKind::Hdr),
        );
        assert_eq!(order, vec![Route::AppEngine], "subtitle {subtitle}");
        assert_eq!(reasons, vec![RouteReason::SUBTITLE_NEEDS_RENDERING]);
        assert!(lacks.is_empty());
    }
    // With a track the native engine lacks as well, both reasons.
    let (_, reasons, lacks) = check(
        &movie(),
        tracks(2, Some(5)),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(
        reasons,
        vec![
            RouteReason::NATIVE_LACKS,
            RouteReason::SUBTITLE_NEEDS_RENDERING
        ]
    );
    assert_eq!(lacks, vec![gap(GapKind::Audio, "truehd")]);
}

#[test]
fn a_container_the_native_engine_lacks_goes_to_the_remux_where_there_is_one() {
    let (order, reasons, _) = check(
        &movie(),
        tracks(1, Some(3)),
        &with_remux(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::DeviceRemux, Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::KEEPS_ORIGINAL_STREAMS]);
    // Windows has no remux (departure 1): an AVI goes to the app engine.
    let mut avi = movie();
    avi.container = Some("avi".into());
    let (order, reasons, lacks) = check(
        &avi,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(lacks, vec![gap(GapKind::Container, "avi")]);
}

#[test]
fn no_remux_with_a_subtitle_that_needs_rendering_or_dolby_vision_7() {
    let (order, reasons, _) = check(
        &movie(),
        tracks(1, Some(5)),
        &with_remux(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(
        reasons,
        vec![
            RouteReason::NATIVE_LACKS,
            RouteReason::SUBTITLE_NEEDS_RENDERING
        ]
    );
    let profile_7 = with_video(video("hevc", 10, VideoRangeType::DOVI_WITH_EL, Some(7)));
    let (order, reasons, lacks) = check(
        &profile_7,
        tracks(1, None),
        &with_remux(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(
        reasons,
        vec![
            RouteReason::NATIVE_LACKS,
            RouteReason::DOLBY_VISION_PROFILE_7
        ]
    );
    assert_eq!(lacks, vec![gap(GapKind::Container, "mkv")]);
}

#[test]
fn dolby_vision_plays_on_the_app_engine_first_on_windows() {
    for profile in [5, 8] {
        let source = with_video(video(
            "hevc",
            10,
            VideoRangeType::DOVI_WITH_HDR10,
            Some(profile),
        ));
        let (order, reasons, _) = check(
            &source,
            tracks(1, None),
            &windows(),
            automatic(DisplayKind::Hdr),
        );
        assert_eq!(
            order,
            vec![Route::AppEngine, Route::Native],
            "profile {profile}"
        );
        assert_eq!(reasons, vec![RouteReason::DOLBY_VISION_ON_APP_ENGINE]);
    }
    // Elsewhere the rule is off: native.
    let source = with_video(video("hevc", 10, VideoRangeType::DOVI, Some(5)));
    let (order, ..) = check(
        &source,
        tracks(1, None),
        &with_remux(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::DeviceRemux);
}

#[test]
fn pq_video_on_an_sdr_display_plays_on_the_app_engine_and_hlg_stays_native() {
    for range in [VideoRangeType::HDR10, VideoRangeType::HDR10_PLUS] {
        let source = with_video(video("hevc", 10, range.clone(), None));
        let (order, reasons, _) = check(
            &source,
            tracks(1, None),
            &windows(),
            automatic(DisplayKind::Sdr),
        );
        assert_eq!(order, vec![Route::AppEngine, Route::Native], "{range:?}");
        assert_eq!(reasons, vec![RouteReason::HDR_ON_SDR_DISPLAY]);
        // On an HDR display the rule applies unchanged.
        let (order, ..) = check(
            &source,
            tracks(1, None),
            &windows(),
            automatic(DisplayKind::Hdr),
        );
        assert_eq!(order[0], Route::Native);
    }
    let hlg = with_video(video("hevc", 10, VideoRangeType::HLG, None));
    let (order, ..) = check(
        &hlg,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Sdr),
    );
    assert_eq!(order[0], Route::Native);
    // Dolby Vision on an SDR display: both rules.
    let dv = with_video(video("hevc", 10, VideoRangeType::DOVI_WITH_HDR10, Some(8)));
    let (_, reasons, _) = check(
        &dv,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Sdr),
    );
    assert_eq!(
        reasons,
        vec![
            RouteReason::DOLBY_VISION_ON_APP_ENGINE,
            RouteReason::HDR_ON_SDR_DISPLAY
        ]
    );
}

#[test]
fn an_unknown_display_keeps_pq_video_native() {
    // The Windows UI always reports the display (W4); until it has, departure 8 changes nothing.
    let source = with_video(video("hevc", 10, VideoRangeType::HDR10, None));
    let (order, reasons, _) = check(
        &source,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Unknown),
    );
    assert_eq!(order, vec![Route::Native, Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_FULLY_COMPATIBLE]);
}

#[test]
fn the_setting_and_the_per_playback_choice_give_the_app_engine_only() {
    let setting = RouteContext {
        preference: EnginePreference::AlwaysAppEngine,
        ..automatic(DisplayKind::Hdr)
    };
    let (order, reasons, _) = check(&movie(), tracks(1, None), &windows(), setting);
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::SETTING_ALWAYS_APP_ENGINE]);
    let chosen = RouteContext {
        app_engine_chosen: true,
        ..setting
    };
    let (order, reasons, _) = check(&movie(), tracks(1, None), &windows(), chosen);
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::USER_CHOSE_APP_ENGINE]);
    // A file the app engine does not play: no local route, the server's stream.
    let mut webm = with_video(plain(0, StreamKind::VIDEO, "vp8"));
    webm.container = Some("webm".into());
    assert!(
        check(&webm, tracks(1, None), &windows(), chosen)
            .0
            .is_empty()
    );
}

#[test]
fn routes_already_tried_are_left_out_and_none_left_means_the_server() {
    let engines = windows();
    let context = automatic(DisplayKind::Hdr);
    let after_native = local_routes(
        &movie(),
        tracks(1, None),
        &engines,
        context,
        &[Route::Native],
    );
    assert_eq!(
        after_native
            .iter()
            .map(|choice| &choice.route)
            .collect::<Vec<_>>(),
        vec![&Route::AppEngine]
    );
    assert!(
        local_routes(
            &movie(),
            tracks(1, None),
            &engines,
            context,
            &[Route::Native, Route::AppEngine]
        )
        .is_empty()
    );
    // A container only the native engine reads with a codec only the app engine decodes: no local route.
    let mut odd = with_video(plain(0, StreamKind::VIDEO, "vp9"));
    odd.container = Some("mov,mp4,m4a".into());
    let mut native_only_mp4 = engines.clone();
    native_only_mp4.app.capabilities.containers = vec![Container::MKV];
    assert!(local_routes(&odd, tracks(1, None), &native_only_mp4, context, &[]).is_empty());
}

#[test]
fn with_the_app_engine_only_the_servers_answer_stands() {
    // Whatever the file, and even after a failure: today's direct play (ADR 0019).
    let engines = Engines::app_only(EngineCapabilities::default());
    for context in [
        automatic(DisplayKind::Sdr),
        RouteContext {
            preference: EnginePreference::AlwaysAppEngine,
            app_engine_chosen: true,
            display: DisplayKind::Unknown,
            speed_not_supported: true,
        },
    ] {
        let choices = local_routes(
            &movie(),
            tracks(2, Some(6)),
            &engines,
            context,
            &[Route::AppEngine],
        );
        assert_eq!(
            choices,
            vec![RouteChoice::new(Route::AppEngine, Vec::new(), Vec::new())]
        );
    }
    let offer = hls_offer(StreamAction::Transcode, StreamAction::Copy);
    assert_eq!(
        server_routes(
            &offer,
            &movie(),
            tracks(1, None),
            &engines,
            RouteContext::default(),
            &[]
        ),
        vec![Route::server_stream(&offer, EngineRole::App).expect("a stream")]
    );
}

fn hls_offer(video: StreamAction, audio: StreamAction) -> ServerOffer {
    ServerOffer::Stream {
        protocol: StreamProtocol::Hls,
        container: Some("mp4".into()),
        video,
        audio,
    }
}

fn engines_of(routes: &[Route]) -> Vec<EngineRole> {
    routes.iter().map(Route::engine).collect()
}

#[test]
fn a_server_stream_goes_to_the_first_engine_that_takes_it() {
    let engines = windows();
    let context = automatic(DisplayKind::Hdr);
    // Copied HEVC and transcoded audio, in fMP4 HLS: native, then the app engine.
    let offer = hls_offer(StreamAction::Copy, StreamAction::Transcode);
    let routes = server_routes(&offer, &movie(), tracks(2, None), &engines, context, &[]);
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
    assert_eq!(
        routes[0],
        Route::server_stream(&offer, EngineRole::Native).expect("a stream")
    );
    // Copied TrueHD: the native engine cannot take it.
    let copied = hls_offer(StreamAction::Copy, StreamAction::Copy);
    let routes = server_routes(&copied, &movie(), tracks(2, None), &engines, context, &[]);
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
    // A transcode to codecs the native engine decodes.
    let transcode = hls_offer(StreamAction::Transcode, StreamAction::Transcode);
    let routes = server_routes(
        &transcode,
        &movie(),
        tracks(2, None),
        &engines,
        context,
        &[],
    );
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
    // Already tried natively: the app engine.
    let routes = server_routes(
        &transcode,
        &movie(),
        tracks(2, None),
        &engines,
        context,
        &routes[..1],
    );
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
}

#[test]
fn a_native_engine_without_hls_or_a_transcode_codec_leaves_the_stream_to_the_app_engine() {
    let context = automatic(DisplayKind::Hdr);
    let transcode = hls_offer(StreamAction::Transcode, StreamAction::Transcode);
    let mut no_hls = windows();
    no_hls.native.as_mut().expect("native").hls = false;
    let routes = server_routes(&transcode, &movie(), tracks(1, None), &no_hls, context, &[]);
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
    // A progressive stream needs no HLS.
    let progressive = ServerOffer::Stream {
        protocol: StreamProtocol::Progressive,
        container: Some("mkv".into()),
        video: StreamAction::Copy,
        audio: StreamAction::Transcode,
    };
    let routes = server_routes(
        &progressive,
        &movie(),
        tracks(1, None),
        &no_hls,
        context,
        &[],
    );
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
    // Without HEVC (no Store extension) the server may send HEVC, which the profile has through the app engine.
    let mut no_hevc = windows();
    no_hevc
        .native
        .as_mut()
        .expect("native")
        .capabilities
        .video
        .retain(|support| support.codec != Codec::HEVC);
    let routes = server_routes(
        &transcode,
        &movie(),
        tracks(1, None),
        &no_hevc,
        context,
        &[],
    );
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
}

#[test]
fn a_server_stream_follows_the_display_rules_and_the_users_choice() {
    let engines = windows();
    let copied = hls_offer(StreamAction::Copy, StreamAction::Transcode);
    let hdr10 = with_video(video("hevc", 10, VideoRangeType::HDR10, None));
    let routes = server_routes(
        &copied,
        &hdr10,
        tracks(1, None),
        &engines,
        automatic(DisplayKind::Sdr),
        &[],
    );
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
    // A transcode of a PQ source may keep HDR (not verified): departure 8 holds for it too.
    let transcode = hls_offer(StreamAction::Transcode, StreamAction::Transcode);
    let routes = server_routes(
        &transcode,
        &hdr10,
        tracks(1, None),
        &engines,
        automatic(DisplayKind::Sdr),
        &[],
    );
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
    // On an HDR display, native first.
    let routes = server_routes(
        &transcode,
        &hdr10,
        tracks(1, None),
        &engines,
        automatic(DisplayKind::Hdr),
        &[],
    );
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
    // Dolby Vision transcoded: the server's picture, not the source's Dolby Vision; departure 2 does not apply.
    let dv = with_video(video("hevc", 10, VideoRangeType::DOVI_WITH_HDR10, Some(8)));
    let routes = server_routes(
        &transcode,
        &dv,
        tracks(1, None),
        &engines,
        automatic(DisplayKind::Hdr),
        &[],
    );
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
    let chosen = RouteContext {
        app_engine_chosen: true,
        ..automatic(DisplayKind::Hdr)
    };
    let routes = server_routes(&transcode, &movie(), tracks(1, None), &engines, chosen, &[]);
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
}

#[test]
fn gaps_name_what_is_missing() {
    let mut source = movie();
    source.container = None;
    source.streams[1].codec = None;
    let lacks = gaps(&native_caps(), &source, tracks(1, Some(7)), Unnamed::Lacks);
    assert_eq!(
        lacks,
        vec![
            TrackGap {
                kind: GapKind::Container,
                name: None
            },
            TrackGap {
                kind: GapKind::Audio,
                name: None
            },
        ]
    );
    let mut no_external = native_caps();
    no_external.external_subtitles = false;
    assert_eq!(
        gaps(&no_external, &movie(), tracks(1, Some(7)), Unnamed::Lacks),
        vec![gap(GapKind::Subtitle, "srt")]
    );
    // Profile, level and range limits.
    let mut limited = native_caps();
    limited.video[1] = CodecSupport {
        profiles: Some(vec!["main".into()]),
        level: Some(150),
        ranges: Some(vec![VideoRangeType::SDR]),
        ..CodecSupport::any(Codec::HEVC)
    };
    let mut main = movie();
    main.streams[0].profile = Some("Main".into());
    main.streams[0].level = Some(150.0);
    assert!(gaps(&limited, &main, tracks(1, None), Unnamed::Lacks).is_empty());
    for change in 0..3 {
        let mut other = main.clone();
        match change {
            0 => other.streams[0].profile = Some("Main 10".into()),
            1 => other.streams[0].level = Some(153.0),
            _ => {
                other.streams[0]
                    .video
                    .as_mut()
                    .expect("video facts")
                    .range_type = Some(VideoRangeType::HDR10)
            }
        }
        assert_eq!(
            gaps(&limited, &other, tracks(1, None), Unnamed::Lacks),
            vec![gap(GapKind::Video, "hevc")],
            "change {change}"
        );
    }
}

#[test]
fn a_rendered_subtitle_never_goes_through_the_remux_even_one_the_native_engine_lists() {
    // An Apple-shaped native engine that lists PGS, with a remux for MKV: PGS needs rendering, so no remux (choice 1).
    let mut engines = with_remux();
    let native = engines.native.as_mut().expect("native");
    native.capabilities.subtitles.push(SubtitleFormat::PGSSUB);
    let (order, reasons, lacks) = check(
        &movie(),
        tracks(1, Some(4)),
        &engines,
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(
        reasons,
        vec![
            RouteReason::NATIVE_LACKS,
            RouteReason::SUBTITLE_NEEDS_RENDERING
        ]
    );
    assert_eq!(lacks, vec![gap(GapKind::Container, "mkv")]);
    // A text subtitle: the remux.
    let (order, ..) = check(
        &movie(),
        tracks(1, Some(3)),
        &engines,
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::DeviceRemux);
}

#[test]
fn only_the_platforms_rules_keep_a_rendered_subtitle_native() {
    // A native engine that lists ASS and DVD subtitles too: they still go to the app engine (choice 1); only PGS stays (departure 3).
    let mut engines = windows();
    let native = engines.native.as_mut().expect("native");
    native
        .capabilities
        .subtitles
        .extend([SubtitleFormat::ASS, SubtitleFormat::DVDSUB]);
    for subtitle in [5, 6] {
        let (order, reasons, lacks) = check(
            &movie(),
            tracks(1, Some(subtitle)),
            &engines,
            automatic(DisplayKind::Hdr),
        );
        assert_eq!(order, vec![Route::AppEngine], "subtitle {subtitle}");
        assert_eq!(reasons, vec![RouteReason::SUBTITLE_NEEDS_RENDERING]);
        assert!(lacks.is_empty());
    }
    let (order, ..) = check(
        &movie(),
        tracks(1, Some(4)),
        &engines,
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::Native);
    // Elsewhere no rendered format is kept native, PGS included.
    let mut elsewhere = engines.clone();
    elsewhere.rules = PlatformRules::default();
    let (order, reasons, _) = check(
        &movie(),
        tracks(1, Some(4)),
        &elsewhere,
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::SUBTITLE_NEEDS_RENDERING]);
}

#[test]
fn a_server_stream_with_a_subtitle_the_native_route_does_not_show_goes_to_the_app_engine() {
    let engines = windows();
    let context = automatic(DisplayKind::Hdr);
    let offer = hls_offer(StreamAction::Copy, StreamAction::Transcode);
    // Styled ASS sent beside the picture.
    let routes = server_routes(&offer, &movie(), tracks(1, Some(5)), &engines, context, &[]);
    assert_eq!(engines_of(&routes), vec![EngineRole::App]);
    // PGS on Windows, and a text subtitle: native first.
    for subtitle in [3, 4] {
        let routes = server_routes(
            &offer,
            &movie(),
            tracks(1, Some(subtitle)),
            &engines,
            context,
            &[],
        );
        assert_eq!(
            engines_of(&routes),
            vec![EngineRole::Native, EngineRole::App],
            "subtitle {subtitle}"
        );
    }
    // Burned into the picture by the server: no subtitle to show, native first.
    let mut burned = movie();
    burned.streams[5].subtitle_delivery = Some(SubtitleDelivery::ENCODE);
    let transcode = hls_offer(StreamAction::Transcode, StreamAction::Transcode);
    let routes = server_routes(
        &transcode,
        &burned,
        tracks(1, Some(5)),
        &engines,
        context,
        &[],
    );
    assert_eq!(
        engines_of(&routes),
        vec![EngineRole::Native, EngineRole::App]
    );
}

#[test]
fn multichannel_opus_goes_to_the_app_engine() {
    let mut source = movie();
    let mut opus = audio(8, "opus");
    opus.audio.as_mut().expect("audio facts").channels = Some(8);
    source.streams.push(opus);
    let (order, reasons, lacks) = check(
        &source,
        tracks(8, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(lacks, vec![gap(GapKind::Audio, "opus")]);
    // Stereo Opus plays natively.
    source.streams[8]
        .audio
        .as_mut()
        .expect("audio facts")
        .channels = Some(2);
    let (order, ..) = check(
        &source,
        tracks(8, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order[0], Route::Native);
}

#[test]
fn a_route_after_a_failed_one_says_so() {
    let engines = windows();
    let context = automatic(DisplayKind::Hdr);
    let after_native = local_routes(
        &movie(),
        tracks(1, None),
        &engines,
        context,
        &[Route::Native],
    );
    assert_eq!(after_native[0].route, Route::AppEngine);
    assert_eq!(
        after_native[0].reasons,
        vec![RouteReason::PREVIOUS_ROUTE_FAILED]
    );
    // After the app engine failed under a display rule: native, saying so, with its own reason.
    let hdr10 = with_video(video("hevc", 10, VideoRangeType::HDR10, None));
    let after_app = local_routes(
        &hdr10,
        tracks(1, None),
        &engines,
        automatic(DisplayKind::Sdr),
        &[Route::AppEngine],
    );
    assert_eq!(after_app[0].route, Route::Native);
    assert_eq!(
        after_app[0].reasons,
        vec![
            RouteReason::PREVIOUS_ROUTE_FAILED,
            RouteReason::NATIVE_FULLY_COMPATIBLE
        ]
    );
    // Nothing failed: no such reason.
    let first = local_routes(&movie(), tracks(1, None), &engines, context, &[]);
    assert_eq!(first[0].reasons, vec![RouteReason::NATIVE_FULLY_COMPATIBLE]);
}

#[test]
fn an_unnamed_codec_is_not_native_but_the_app_engine_plays_it() {
    // The server offered direct play: as before ADR 0025, the app engine plays what the server does not name.
    let mut source = movie();
    source.streams[0].codec = None;
    let (order, reasons, lacks) = check(
        &source,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert_eq!(reasons, vec![RouteReason::NATIVE_LACKS]);
    assert_eq!(
        lacks,
        vec![TrackGap {
            kind: GapKind::Video,
            name: None
        }]
    );
    let mut no_container = movie();
    no_container.container = None;
    let (order, ..) = check(
        &no_container,
        tracks(1, None),
        &windows(),
        automatic(DisplayKind::Hdr),
    );
    assert_eq!(order, vec![Route::AppEngine]);
    assert!(gaps(&app_caps(), &source, tracks(1, None), Unnamed::Plays).is_empty());
    assert!(gaps(&app_caps(), &no_container, tracks(1, None), Unnamed::Plays).is_empty());
}

#[test]
fn a_container_gap_names_the_container_as_the_user_knows_it() {
    let mut native = native_caps();
    native.containers = vec![Container::MKV];
    let mut mp4 = movie();
    mp4.container = Some("mov,mp4,m4a,3gp,3g2,mj2".into());
    assert_eq!(
        gaps(&native, &mp4, tracks(1, None), Unnamed::Lacks),
        vec![gap(GapKind::Container, "mp4")]
    );
}
