//! What the player asks besides PlaybackInfo and the reports, against `tools/jellyfin-stand-in`: the next episode, the transcoding
//! policy, a subtitle file, and the stream's secrets and path for the loopback relay.

use std::sync::Arc;

use current_jellyfin::{
    ClientIdentity, JellyfinClient, MAXIMUM_STREAMING_BITRATE, PlaybackRequest, TranscodePolicy,
};
use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, ItemId, ItemRef, ServerKey, ServerRef,
    ServiceType, TrackChoice,
};
use current_net::{FixedAddress, RequestLog, Url};
use jellyfin_stand_in::library::EPISODE_IDS;
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, MEDIA_SOURCE, PASSWORD, REMOTE_BITRATE_LIMIT, SUBTITLE, StandIn, USER_NAME,
};

fn server() -> ServerRef {
    ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"))
}

async fn signed_in(stand_in: &StandIn, log: &Arc<RequestLog>) -> JellyfinClient {
    let client = JellyfinClient::new(
        server(),
        Arc::new(FixedAddress(Url::parse(stand_in.address()).unwrap())),
        ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef"),
        log.clone(),
    )
    .unwrap();
    let signed_in = client.sign_in_by_name(USER_NAME, PASSWORD).await.unwrap();
    client.set_token(signed_in.token);
    client
}

#[tokio::test]
async fn the_next_episode_follows_in_the_series_and_the_last_has_none() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in, &Arc::default()).await;
    let episode = |id: &str| ItemRef::new(server(), ItemId::new(id));
    let next = client
        .next_episode(&episode(EPISODE_IDS[0]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.item, episode(EPISODE_IDS[1]));
    assert!(
        stand_in
            .paths()
            .iter()
            .any(|path| path.starts_with("/Shows/"))
    );
    assert_eq!(
        client.next_episode(&episode(EPISODE_IDS[1])).await.unwrap(),
        None
    );
    // A film has no series.
    assert_eq!(
        client
            .next_episode(&episode(DIRECT_PLAY_ITEM))
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn the_policy_is_read_with_its_defaults() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in, &Arc::default()).await;
    assert_eq!(
        client.transcode_policy().await.unwrap(),
        TranscodePolicy {
            video_transcoding: true,
            remuxing: false,
            force_remote_source_transcoding: false,
            remote_bitrate_limit: Some(REMOTE_BITRATE_LIMIT),
        }
    );
}

#[tokio::test]
async fn a_subtitle_is_fetched_through_the_client_in_its_own_format() {
    let stand_in = StandIn::start().await.unwrap();
    let log = Arc::new(RequestLog::default());
    let client = signed_in(&stand_in, &log).await;
    let data = client
        .subtitle(
            &ItemId::new(DIRECT_PLAY_ITEM),
            MEDIA_SOURCE,
            2,
            Some(&Codec::new("subrip")),
        )
        .await
        .unwrap();
    assert_eq!(data, SUBTITLE.as_bytes());
    assert!(stand_in.paths().contains(&format!(
        "/Videos/{DIRECT_PLAY_ITEM}/{MEDIA_SOURCE}/Subtitles/2/Stream.srt"
    )));
}

#[tokio::test]
async fn a_stream_without_a_gateway_needs_only_the_token_redacted() {
    let stand_in = StandIn::start().await.unwrap();
    let client = signed_in(&stand_in, &Arc::default()).await;
    let request = PlaybackRequest {
        media_source: None,
        start: std::time::Duration::ZERO,
        tracks: TrackChoice::default(),
        max_streaming_bitrate: MAXIMUM_STREAMING_BITRATE,
        direct_play: true,
    };
    let capabilities = EngineCapabilities {
        containers: vec![Container::MKV],
        video: vec![CodecSupport::any(Codec::HEVC)],
        audio: vec![CodecSupport::any(Codec::AAC)],
        ..EngineCapabilities::default()
    };
    let answer = client
        .playback_info(&ItemId::new(DIRECT_PLAY_ITEM), &request, &capabilities)
        .await
        .unwrap();
    let (_, stream) = answer.playable(None).unwrap();
    let secrets = client.stream_secrets();
    assert_eq!(secrets.len(), 1);
    assert!(stream.url.expose().contains(&secrets[0]));
    assert!(!client.address_has_gateway());
    // For the loopback relay: the path on the address, without the token.
    let path = client.stream_path(stream.url.expose()).unwrap();
    assert!(
        path.starts_with(&format!("/Videos/{DIRECT_PLAY_ITEM}/stream?")),
        "{path}"
    );
    assert!(path.contains("static=true"), "{path}");
    assert!(
        !path.contains(&secrets[0]) && !path.contains("ApiKey"),
        "{path}"
    );
}
