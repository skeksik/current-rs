//! The HLS playlist same-origin check (ADR 0019, "Playlists and headers") against `tools/jellyfin-stand-in`, behind a header
//! gateway: the check's requests carry the gateway header and the token, and the `ApiKey` never reaches the request log.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use current_jellyfin::{
    ClientIdentity, JellyfinClient, MAX_PLAYLIST_BYTES, MAXIMUM_STREAMING_BITRATE, PlaybackRequest,
    PlaylistCheckError,
};
use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, ItemId, ServerKey, ServerRef, ServiceType,
    TrackChoice,
};
use current_net::{
    AddressOutcome, AddressProvider, AuthorizationUse, ExtraHeader, Gateway, GatewayAddress,
    GatewayCredentials, RequestLog, Selected, Url,
};
use jellyfin_stand_in::{
    GATE_HEADER, GATE_HEADER_VALUE, Gate, MASTER_PLAYLIST, PASSWORD, StandIn, TRANSCODE_ITEM,
    USER_NAME, master_playlist_path, media_playlist_path,
};

/// The stand-in behind a header gateway, at an address the test can move to another (as roaming would).
struct Address {
    addresses: [GatewayAddress; 2],
    moved: AtomicBool,
}

impl AddressProvider for Address {
    fn current(&self) -> Option<Selected> {
        self.addresses[usize::from(self.moved.load(Ordering::SeqCst))].current()
    }

    fn wait(
        &self,
        _deadline: Instant,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
        Box::pin(std::future::ready(self.current()))
    }

    fn report(&self, _selected: &Selected, _outcome: AddressOutcome) {}
}

struct Setup {
    stand_in: StandIn,
    address: Arc<Address>,
    log: Arc<RequestLog>,
    client: JellyfinClient,
    /// The transcode's stream URL, with the token as `ApiKey`.
    stream: String,
    token: String,
}

async fn setup() -> Setup {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_gate(Gate::Header);
    let header =
        ExtraHeader::new(GATE_HEADER, GATE_HEADER_VALUE, AuthorizationUse::Service).unwrap();
    let credentials = GatewayCredentials::new(None, vec![header], Vec::new()).unwrap();
    let gateway = Arc::new(Gateway::new(credentials));
    let url = Url::parse(stand_in.address()).unwrap();
    let mut other = url.clone();
    other.set_host(Some("localhost")).unwrap();
    let address = Arc::new(Address {
        addresses: [
            GatewayAddress::new(url, gateway.clone(), false),
            GatewayAddress::new(other, gateway, false),
        ],
        moved: AtomicBool::new(false),
    });
    let log = Arc::new(RequestLog::default());
    let provider: Arc<dyn AddressProvider> = address.clone();
    let client = JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        provider,
        ClientIdentity::current("Office PC", "0123456789abcdef0123456789abcdef"),
        log.clone(),
    )
    .unwrap();
    let signed_in = client.sign_in_by_name(USER_NAME, PASSWORD).await.unwrap();
    client.set_token(signed_in.token);
    let capabilities = EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4],
        video: vec![CodecSupport::any(Codec::HEVC)],
        audio: vec![CodecSupport::any(Codec::AAC)],
        subtitles: Vec::new(),
        external_subtitles: true,
    };
    let request = PlaybackRequest {
        media_source: None,
        start: Duration::ZERO,
        tracks: TrackChoice::default(),
        max_streaming_bitrate: MAXIMUM_STREAMING_BITRATE,
        direct_play: true,
    };
    let answer = client
        .playback_info(&ItemId::new(TRANSCODE_ITEM), &request, &capabilities)
        .await
        .unwrap();
    let (_, playable) = answer.playable(None).unwrap();
    let stream = playable.url.expose().to_owned();
    let token = Url::parse(&stream)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "ApiKey")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    Setup {
        stand_in,
        address,
        log,
        client,
        stream,
        token,
    }
}

impl Setup {
    async fn check(&self) -> Result<(), PlaylistCheckError> {
        self.client.check_playlist_origin(&self.stream).await
    }

    fn port(&self) -> u16 {
        Url::parse(self.stand_in.address()).unwrap().port().unwrap()
    }
}

fn media(lines: &str) -> String {
    format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:6\n{lines}\n#EXT-X-ENDLIST\n")
}

#[tokio::test]
async fn relative_playlists_pass_with_the_gateway_header_and_no_api_key_in_the_log() {
    let setup = setup().await;
    setup.check().await.unwrap();

    let seen = setup.stand_in.playlists();
    let targets: Vec<&str> = seen
        .iter()
        .map(|seen| seen.target.split('?').next().unwrap())
        .collect();
    assert_eq!(targets, [master_playlist_path(), media_playlist_path()]);
    for seen in &seen {
        assert_eq!(seen.gate_header, GATE_HEADER_VALUE, "{}", seen.target);
        assert!(!seen.target.contains("ApiKey"), "{}", seen.target);
        assert!(!seen.target.contains(&setup.token), "{}", seen.target);
    }
    let lines: Vec<String> = setup
        .log
        .entries()
        .iter()
        .map(|entry| entry.to_line())
        .collect();
    let playlist_lines: Vec<&String> = lines.iter().filter(|line| line.contains(".m3u8")).collect();
    assert_eq!(playlist_lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(!line.contains(&setup.token), "{line}");
        assert!(!line.contains("ApiKey"), "{line}");
        assert!(!line.contains("not-a-real-variant-key"), "{line}");
        assert!(!line.contains(GATE_HEADER_VALUE), "{line}");
    }
}

#[tokio::test]
async fn absolute_uris_on_the_same_origin_pass() {
    let setup = setup().await;
    let origin = setup.stand_in.address().trim_end_matches('/').to_owned();
    setup.stand_in.set_playlist(
        &master_playlist_path(),
        &format!(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n{origin}{}?DeviceId=d\n",
            media_playlist_path()
        ),
    );
    setup.stand_in.set_playlist(
        &media_playlist_path(),
        &media(&format!(
            "#EXT-X-MAP:URI=\"{origin}/videos/x/init.mp4\"\n#EXTINF:6.0,\n{origin}/videos/x/0.mp4"
        )),
    );
    setup.check().await.unwrap();
}

#[tokio::test]
async fn a_uri_on_another_host_port_or_scheme_is_refused_wherever_it_is() {
    let setup = setup().await;
    let port = setup.port();
    let elsewhere = [
        "http://192.168.1.10/x".to_owned(),
        format!("http://127.0.0.1:{}/x", port.wrapping_add(1)),
        format!("https://127.0.0.1:{port}/x"),
        "//media.example.com/x".to_owned(),
    ];
    let master_with = |line: &str| format!("{MASTER_PLAYLIST}{line}\n");
    for uri in &elsewhere {
        let cases = [
            // A variant, a media rendition, an I-frame playlist and a session key in the master playlist.
            (
                master_with(&format!("#EXT-X-STREAM-INF:BANDWIDTH=1\n{uri}")),
                None,
            ),
            (
                master_with(&format!(
                    "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"a\",URI=\"{uri}\""
                )),
                None,
            ),
            (
                master_with(&format!(
                    "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=1,URI=\"{uri}\""
                )),
                None,
            ),
            (
                master_with(&format!("#EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"{uri}\"")),
                None,
            ),
            // A segment, a key and a map in the media playlist.
            (
                MASTER_PLAYLIST.to_owned(),
                Some(media(&format!("#EXTINF:6.0,\n{uri}"))),
            ),
            (
                MASTER_PLAYLIST.to_owned(),
                Some(media(&format!(
                    "#EXT-X-KEY:METHOD=AES-128,URI=\"{uri}\"\n#EXTINF:6.0,\n0.mp4"
                ))),
            ),
            (
                MASTER_PLAYLIST.to_owned(),
                Some(media(&format!(
                    "#EXT-X-MAP:URI=\"{uri}\"\n#EXTINF:6.0,\n0.mp4"
                ))),
            ),
        ];
        for (master, media_playlist) in cases {
            setup
                .stand_in
                .set_playlist(&master_playlist_path(), &master);
            setup.stand_in.set_playlist(
                &media_playlist_path(),
                media_playlist
                    .as_deref()
                    .unwrap_or(jellyfin_stand_in::MEDIA_PLAYLIST),
            );
            assert_eq!(
                setup.check().await,
                Err(PlaylistCheckError::Elsewhere),
                "{master} {media_playlist:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_huge_playlist_is_refused() {
    let setup = setup().await;
    let segment = "#EXTINF:6.0,\nhls1/main/0.mp4?PlaySessionId=57a2d1e0000000000000000000000013\n";
    let huge = media(&segment.repeat(MAX_PLAYLIST_BYTES / segment.len() + 1));
    assert!(huge.len() > MAX_PLAYLIST_BYTES);
    setup.stand_in.set_playlist(&media_playlist_path(), &huge);
    assert_eq!(setup.check().await, Err(PlaylistCheckError::TooLarge));
}

#[tokio::test]
async fn a_page_instead_of_a_playlist_is_malformed() {
    let setup = setup().await;
    setup
        .stand_in
        .set_playlist(&master_playlist_path(), "<!DOCTYPE html><html></html>");
    assert_eq!(setup.check().await, Err(PlaylistCheckError::Malformed));
}

#[tokio::test]
async fn a_url_built_on_an_address_the_client_left_is_not_checked_against_the_new_one() {
    let setup = setup().await;
    setup.address.moved.store(true, Ordering::SeqCst);
    let before = setup.stand_in.playlists().len();
    assert_eq!(setup.check().await, Err(PlaylistCheckError::AddressChanged));
    assert_eq!(setup.stand_in.playlists().len(), before, "nothing fetched");
}
