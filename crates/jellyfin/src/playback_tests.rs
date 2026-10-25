use super::*;

#[test]
fn reasons_map_to_constants_and_unknown_ones_pass_through() {
    assert_eq!(
        route_reason("ContainerNotSupported"),
        RouteReason::CONTAINER_NOT_SUPPORTED
    );
    assert_eq!(
        route_reason("VideoRangeTypeNotSupported"),
        RouteReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED
    );
    assert_eq!(
        route_reason("DirectPlayError"),
        RouteReason::DIRECT_PLAY_ERROR
    );
    let unknown = route_reason("SomeFutureReason");
    assert!(!unknown.is_known());
    assert_eq!(unknown.as_str(), "SomeFutureReason");
}

#[test]
fn ticks_are_100_nanoseconds() {
    assert_eq!(ticks(Duration::from_secs(1)), 10_000_000);
    assert_eq!(duration(25_000_000), Duration::from_millis(2500));
    assert_eq!(duration(-5), Duration::ZERO);
}

fn source(video: &str, audio: &str) -> MediaSource {
    let stream = |index, kind: StreamKind, codec: &str| MediaStream {
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
        video: None,
        audio: None,
        subtitle_delivery: None,
    };
    MediaSource {
        id: "s1".into(),
        name: None,
        container: Some("mkv".into()),
        size: None,
        bitrate: None,
        duration: None,
        streams: vec![
            stream(0, StreamKind::VIDEO, video),
            stream(1, StreamKind::AUDIO, audio),
        ],
        default_audio: Some(1),
        default_subtitle: None,
        has_segments: false,
        is_remote: false,
    }
}

fn pairs(query: &str) -> Vec<(String, String)> {
    url_query_pairs(query)
}

fn actions(route: &Route) -> (StreamAction, StreamAction) {
    match route {
        Route::ServerStream { video, audio, .. } => (*video, *audio),
        Route::DirectPlay => panic!("not a server stream"),
    }
}

#[test]
fn a_container_reason_predicts_a_remux() {
    let (route, reasons) = predict(
        StreamProtocol::Hls,
        Some("mp4".into()),
        &pairs("VideoCodec=hevc,h264&AudioCodec=aac,ac3&TranscodeReasons=ContainerNotSupported"),
        &source("hevc", "ac3"),
    );
    assert_eq!(actions(&route), (StreamAction::Copy, StreamAction::Copy));
    assert_eq!(reasons, vec![RouteReason::CONTAINER_NOT_SUPPORTED]);
}

#[test]
fn an_audio_reason_predicts_direct_stream() {
    let (route, _) = predict(
        StreamProtocol::Hls,
        None,
        &pairs("VideoCodec=hevc,h264&AudioCodec=aac&TranscodeReasons=AudioCodecNotSupported"),
        &source("h264", "dts"),
    );
    assert_eq!(
        actions(&route),
        (StreamAction::Copy, StreamAction::Transcode)
    );
}

#[test]
fn burned_in_subtitles_or_refused_copy_predict_a_video_transcode() {
    let (route, _) = predict(
        StreamProtocol::Hls,
        None,
        &pairs(
            "VideoCodec=h264&AudioCodec=aac&SubtitleMethod=Encode&TranscodeReasons=SubtitleCodecNotSupported",
        ),
        &source("h264", "aac"),
    );
    assert_eq!(
        actions(&route),
        (StreamAction::Transcode, StreamAction::Copy)
    );
    let (route, _) = predict(
        StreamProtocol::Hls,
        None,
        &pairs(
            "VideoCodec=h264&AudioCodec=aac&allowVideoStreamCopy=false&allowAudioStreamCopy=false",
        ),
        &source("h264", "aac"),
    );
    assert_eq!(
        actions(&route),
        (StreamAction::Transcode, StreamAction::Transcode)
    );
}

#[test]
fn a_codec_missing_from_the_output_list_is_converted() {
    let (route, _) = predict(
        StreamProtocol::Hls,
        None,
        &pairs("VideoCodec=h264&AudioCodec=aac&TranscodeReasons=ContainerBitrateExceedsLimit"),
        &source("vc1", "aac"),
    );
    assert_eq!(
        actions(&route),
        (StreamAction::Transcode, StreamAction::Copy)
    );
}
