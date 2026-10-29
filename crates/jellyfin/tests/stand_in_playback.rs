//! Playback against `tools/jellyfin-stand-in`, which the bridge tests (Rust, C#, Swift) use: its answers must stay in the shape
//! `current-jellyfin` reads.

use std::sync::Arc;
use std::time::Duration;

use current_jellyfin::{
    ClientIdentity, JellyfinClient, MAXIMUM_STREAMING_BITRATE, PlaybackRequest, ProgressReport,
    StopReport,
};
use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, EngineRole, ItemId, Route, RouteLabel,
    RouteReason, SegmentKind, ServerKey, ServerOffer, ServerRef, ServiceType, SubtitleFormat,
    TrackChoice,
};
use current_net::{FixedAddress, RequestLog, Url};
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, MEDIA_SOURCE, PASSWORD, PLAY_SESSION, StandIn, TRANSCODE_ITEM, USER_NAME,
};

async fn signed_in(stand_in: &StandIn, log: &Arc<RequestLog>) -> JellyfinClient {
    let client = JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        Arc::new(FixedAddress(Url::parse(stand_in.address()).unwrap())),
        ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef"),
        log.clone(),
    )
    .unwrap();
    let signed_in = client.sign_in_by_name(USER_NAME, PASSWORD).await.unwrap();
    client.set_token(signed_in.token);
    client
}

fn capabilities() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4, Container::TS],
        video: vec![
            CodecSupport::any(Codec::HEVC),
            CodecSupport::any(Codec::H264),
        ],
        audio: vec![CodecSupport::any(Codec::AAC), CodecSupport::any(Codec::AC3)],
        subtitles: vec![SubtitleFormat::SUBRIP, SubtitleFormat::SRT],
        external_subtitles: true,
    }
}

fn request() -> PlaybackRequest {
    PlaybackRequest {
        media_source: None,
        start: Duration::ZERO,
        tracks: TrackChoice::default(),
        max_streaming_bitrate: MAXIMUM_STREAMING_BITRATE,
        direct_play: true,
    }
}

#[tokio::test]
async fn the_stand_in_answers_playback_in_the_shape_the_client_reads() {
    let stand_in = StandIn::start().await.unwrap();
    let log = Arc::new(RequestLog::default());
    let client = signed_in(&stand_in, &log).await;

    let direct = client
        .playback_info(&ItemId::new(DIRECT_PLAY_ITEM), &request(), &capabilities())
        .await
        .unwrap();
    assert_eq!(direct.play_session_id, PLAY_SESSION);
    let (source, stream) = direct.playable(None).unwrap();
    assert_eq!(source.id, MEDIA_SOURCE);
    assert_eq!(stream.offer, ServerOffer::DirectPlay);
    assert!(source.has_segments);

    let transcode = client
        .playback_info(&ItemId::new(TRANSCODE_ITEM), &request(), &capabilities())
        .await
        .unwrap();
    let (_, stream) = transcode.playable(None).unwrap();
    assert_eq!(
        Route::server_stream(&stream.offer, EngineRole::App).map(|route| route.label()),
        Some(RouteLabel::ServerRemux)
    );
    assert_eq!(stream.reasons, vec![RouteReason::CONTAINER_NOT_SUPPORTED]);

    let segments = client
        .media_segments(&ItemId::new(DIRECT_PLAY_ITEM))
        .await
        .unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].kind, SegmentKind::INTRO);

    let facts = client.transcoding_facts().await.unwrap().unwrap();
    assert!(facts.video_direct);
    assert!(
        client
            .playback_preferences()
            .await
            .unwrap()
            .next_episode_autoplay
    );

    let report = ProgressReport {
        item: ItemId::new(DIRECT_PLAY_ITEM),
        media_source: MEDIA_SOURCE.into(),
        play_session_id: PLAY_SESSION.into(),
        position: Duration::from_secs(5),
        audio: Some(1),
        subtitle: None,
        paused: false,
        muted: false,
        volume: None,
        can_seek: true,
        route: Route::AppEngine,
    };
    client.report_start(&report).await.unwrap();
    client.report_progress(&report).await.unwrap();
    client
        .report_stopped(&StopReport {
            item: ItemId::new(DIRECT_PLAY_ITEM),
            media_source: MEDIA_SOURCE.into(),
            play_session_id: PLAY_SESSION.into(),
            position: Duration::from_secs(6),
            failed: false,
        })
        .await
        .unwrap();
    let paths: Vec<String> = stand_in
        .reports()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        paths,
        [
            "/Sessions/Playing",
            "/Sessions/Playing/Progress",
            "/Sessions/Playing/Stopped"
        ]
    );

    stand_in.revoke_all();
    assert!(
        client
            .playback_info(&ItemId::new(DIRECT_PLAY_ITEM), &request(), &capabilities())
            .await
            .is_err()
    );
}
