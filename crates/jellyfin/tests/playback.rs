//! Playback against hand-written Jellyfin stand-ins on 127.0.0.1. Responses follow Jellyfin 12.2's shapes (ADR 0019,
//! `docs/notes/playback.md`): direct play, an HLS transcode with its reasons in the URL, no playable route, segments, playstate
//! reports, the session's transcoding information, the user's playback configuration, and a 401.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_jellyfin::{
    AccessToken, AuthError, ClientIdentity, JellyfinClient, MAXIMUM_STREAMING_BITRATE,
    PlaybackError, PlaybackRequest, ProgressReport, StopReport,
};
use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, ItemId, Route, RouteLabel, RouteReason,
    SegmentKind, ServerKey, ServerRef, ServiceType, StreamAction, StreamKind, StreamProtocol,
    SubtitleChoice, SubtitleDelivery, SubtitleFormat, SubtitleMode, TrackChoice, VideoRangeType,
};
use current_net::{FixedAddress, RequestLog, Url};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const DEVICE_ID: &str = "0123456789abcdef0123456789abcdef";
const ITEM: &str = "1f0000000000000000000000000000a1";
const SOURCE: &str = "2f0000000000000000000000000000b2";
const PLAY_SESSION: &str = "3f0000000000000000000000000000c3";

/// One request as the stand-in received it.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    query: String,
    authorization: String,
    body: String,
}

type Handler = Arc<dyn Fn(&Received) -> (u16, String) + Send + Sync>;

/// A server on 127.0.0.1 that answers each request with what `handler` returns, and keeps every request.
struct StandIn {
    address: Url,
    received: Arc<Mutex<Vec<Received>>>,
}

impl StandIn {
    async fn start(handler: impl Fn(&Received) -> (u16, String) + Send + Sync + 'static) -> Self {
        Self::start_at("", handler).await
    }

    /// With the server under `base_path` (for example `/jellyfin`), as behind a reverse proxy.
    async fn start_at(
        base_path: &str,
        handler: impl Fn(&Received) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = received.clone();
        let prefix = base_path.to_owned();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                let prefix = prefix.clone();
                tokio::spawn(async move {
                    let Some(mut request) = read_request(&mut socket).await else {
                        return;
                    };
                    request.path = request
                        .path
                        .strip_prefix(&prefix)
                        .unwrap_or(&request.path)
                        .to_owned();
                    log.lock().unwrap().push(request.clone());
                    let (status, body) = handler(&request);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            address: Url::parse(&format!("http://127.0.0.1:{port}{base_path}")).unwrap(),
            received,
        }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn body(&self, path: &str) -> Value {
        let request = self
            .received()
            .into_iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("no request to {path}"));
        serde_json::from_str(&request.body).unwrap()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Received> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while data.len() < head_end + length {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    }
    let mut first = head.lines().next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Received {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        authorization: header("authorization"),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

fn client(stand_in: &StandIn, log: &Arc<RequestLog>) -> JellyfinClient {
    let client = JellyfinClient::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        Arc::new(FixedAddress(stand_in.address.clone())),
        ClientIdentity::current("Office PC", DEVICE_ID),
        log.clone(),
    )
    .unwrap();
    client.set_token(AccessToken::new(TOKEN));
    client
}

/// The lists `current-mpv` reports for our Windows build (ADR 0019).
fn capabilities() -> EngineCapabilities {
    EngineCapabilities {
        containers: vec![Container::MKV, Container::MP4, Container::TS],
        video: [Codec::H264, Codec::HEVC, Codec::AV1]
            .into_iter()
            .map(CodecSupport::any)
            .collect(),
        audio: [
            Codec::AAC,
            Codec::AC3,
            Codec::EAC3,
            Codec::FLAC,
            Codec::OPUS,
        ]
        .into_iter()
        .map(CodecSupport::any)
        .collect(),
        subtitles: vec![
            SubtitleFormat::ASS,
            SubtitleFormat::SUBRIP,
            SubtitleFormat::SRT,
            SubtitleFormat::PGSSUB,
        ],
        external_subtitles: true,
    }
}

fn request() -> PlaybackRequest {
    PlaybackRequest {
        media_source: Some(SOURCE.into()),
        start: Duration::from_secs(90),
        tracks: TrackChoice {
            audio: Some(1),
            subtitle: SubtitleChoice::Off,
        },
        max_streaming_bitrate: MAXIMUM_STREAMING_BITRATE,
        direct_play: true,
    }
}

/// Checks that the token reached neither the request log nor a value's Debug output.
fn assert_no_token(log: &RequestLog, debug: &str) {
    let text = format!("{}\n{debug}", log.to_text());
    assert!(!text.contains(TOKEN), "token in {text}");
    assert!(!text.contains("ApiKey"), "ApiKey in {text}");
}

/// A 12.2 `MediaSourceInfo` for an HEVC/E-AC-3 Matroska file with a PGS and an external SRT subtitle, as PlaybackInfo returns it
/// (`Path`, `DeliveryUrl` and `RequiredHttpHeaders` included, which the client must not read). `extra` is spliced in.
fn media_source(extra: &str) -> String {
    format!(
        r#"{{"Protocol":"File","Id":"{SOURCE}","Path":"/media/example/file.mkv","Type":"Default","Container":"mkv","Size":4831838208,
        "Name":"Example 2160p","IsRemote":false,"ETag":"5e7a9c0f","RunTimeTicks":72000000000,"ReadAtNativeFramerate":false,
        "IgnoreDts":false,"IgnoreIndex":false,"GenPtsInput":false,"SupportsTranscoding":true,"SupportsDirectStream":false,
        "IsInfiniteStream":false,"UseMostCompatibleTranscodingProfile":false,"RequiresOpening":false,"RequiresClosing":false,
        "RequiresLooping":false,"SupportsProbing":true,"VideoType":"VideoFile","MediaAttachments":[],"Formats":[],
        "Bitrate":53687091,"RequiredHttpHeaders":{{}},"DefaultAudioStreamIndex":1,
        "DefaultSubtitleStreamIndex":-1,"HasSegments":true,
        "MediaStreams":[
          {{"Codec":"hevc","ColorTransfer":"smpte2084","ColorPrimaries":"bt2020","DvProfile":8,"TimeBase":"1/1000",
            "VideoRange":"HDR","VideoRangeType":"DOVIWithHDR10","DisplayTitle":"4K HEVC Dolby Vision Profile 8.1 (HDR10)",
            "IsInterlaced":false,"BitRate":50000000,"BitDepth":10,"RefFrames":1,"IsDefault":true,"IsForced":false,
            "IsHearingImpaired":false,"Height":2160,"Width":3840,"AverageFrameRate":23.976025,"RealFrameRate":23.976025,
            "Profile":"Main 10","Type":"Video","AspectRatio":"16:9","Index":0,"IsExternal":false,"IsTextSubtitleStream":false,
            "SupportsExternalStream":false,"PixelFormat":"yuv420p10le","Level":153,"IsAnamorphic":false}},
          {{"Codec":"eac3","Language":"eng","TimeBase":"1/1000","Title":"Surround 5.1","DisplayTitle":"Surround 5.1 - English - Dolby Digital+ - 5.1 - Default",
            "IsInterlaced":false,"ChannelLayout":"5.1(side)","BitRate":640000,"Channels":6,"SampleRate":48000,"IsDefault":true,
            "IsForced":false,"IsHearingImpaired":false,"Profile":"Dolby Digital Plus","Type":"Audio","Index":1,"IsExternal":false,
            "IsTextSubtitleStream":false,"SupportsExternalStream":false,"Level":0}},
          {{"Codec":"PGSSUB","Language":"eng","TimeBase":"1/1000","DisplayTitle":"English - PGSSUB","IsInterlaced":false,
            "IsDefault":false,"IsForced":false,"IsHearingImpaired":false,"Type":"Subtitle","Index":2,"IsExternal":false,
            "DeliveryMethod":"Embed","IsTextSubtitleStream":false,"SupportsExternalStream":true,"Level":0}},
          {{"Codec":"srt","Language":"ger","DisplayTitle":"Deutsch - SRT - External","IsInterlaced":false,"IsDefault":false,
            "IsForced":true,"IsHearingImpaired":false,"Type":"Subtitle","Index":3,"IsExternal":true,"DeliveryMethod":"External",
            "DeliveryUrl":"/Videos/{ITEM}/{SOURCE}/Subtitles/3/0/Stream.srt?ApiKey={TOKEN}","IsExternalUrl":false,
            "IsTextSubtitleStream":true,"SupportsExternalStream":true,"Path":"/media/example/file.de.forced.srt","Level":0}}
        ]{extra}}}"#
    )
}

fn answer(source: &str) -> String {
    format!(r#"{{"MediaSources":[{source}],"PlaySessionId":"{PLAY_SESSION}"}}"#)
}

/// `TranscodingUrl` as `StreamInfo.ToUrl` builds it for an HLS fMP4 remux of the source above.
fn transcoding_url() -> String {
    format!(
        "/videos/{ITEM}/master.m3u8?DeviceId={DEVICE_ID}&MediaSourceId={SOURCE}&VideoCodec=hevc,h264&AudioCodec=aac,ac3,eac3,opus,flac\
         &AudioStreamIndex=1&VideoBitrate=139360000&AudioBitrate=640000&AudioSampleRate=48000&MaxFramerate=23.976025\
         &PlaySessionId={PLAY_SESSION}&ApiKey={TOKEN}&SubtitleMethod=Encode&SegmentContainer=mp4&MinSegments=1\
         &BreakOnNonKeyFrames=False&hevc-level=153&hevc-videobitdepth=10&hevc-profile=main10&TranscodeReasons=ContainerNotSupported,SubtitleCodecNotSupported"
    )
}

fn handler(playback_info: String) -> impl Fn(&Received) -> (u16, String) + Send + Sync + 'static {
    move |received| match (received.method.as_str(), received.path.as_str()) {
        ("POST", path) if path == format!("/Items/{ITEM}/PlaybackInfo") => {
            (200, playback_info.clone())
        }
        (
            "POST",
            "/Sessions/Playing" | "/Sessions/Playing/Progress" | "/Sessions/Playing/Stopped",
        ) => (204, String::new()),
        _ => (404, String::new()),
    }
}

#[tokio::test]
async fn direct_play_builds_the_static_url_and_sends_the_profile() {
    let server = StandIn::start(handler(answer(&media_source(
        r#","SupportsDirectPlay":true,"TranscodingSubProtocol":"http""#,
    ))))
    .await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let answer = client
        .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
        .await
        .unwrap();
    assert_eq!(answer.play_session_id, PLAY_SESSION);
    let (source, stream) = answer.playable(Some(SOURCE)).unwrap();
    assert_eq!(stream.route, Route::DirectPlay);
    assert!(stream.reasons.is_empty());
    assert_eq!(
        stream.url.expose(),
        format!(
            "{}Videos/{ITEM}/stream?static=true&mediaSourceId={SOURCE}&playSessionId={PLAY_SESSION}&tag=5e7a9c0f&ApiKey={TOKEN}",
            server.address
        )
    );

    // The source, with the server's indexes and no path.
    assert_eq!(source.id, SOURCE);
    assert_eq!(source.container.as_deref(), Some("mkv"));
    assert_eq!(source.size, Some(4_831_838_208));
    assert_eq!(source.duration, Some(Duration::from_secs(7200)));
    assert_eq!(source.default_audio, Some(1));
    assert_eq!(source.default_subtitle, None);
    assert!(source.has_segments);
    let video = source.video().unwrap();
    assert_eq!(video.codec, Some(Codec::HEVC));
    let facts = video.video.as_ref().unwrap();
    assert_eq!(
        (facts.width, facts.height, facts.bit_depth),
        (Some(3840), Some(2160), Some(10))
    );
    assert_eq!(facts.range_type, Some(VideoRangeType::DOVI_WITH_HDR10));
    assert_eq!(facts.dv_profile, Some(8));
    let audio = source.stream(1).unwrap();
    assert_eq!(audio.kind, StreamKind::AUDIO);
    assert_eq!(audio.audio.as_ref().unwrap().channels, Some(6));
    assert_eq!(audio.language.as_deref(), Some("eng"));
    assert_eq!(source.stream(2).unwrap().codec, Some(Codec::new("PGSSUB")));
    let external = source.stream(3).unwrap();
    assert_eq!(external.codec, Some(Codec::new("srt")));
    assert!(external.is_external && external.is_forced);
    assert_eq!(external.subtitle_delivery, Some(SubtitleDelivery::EXTERNAL));

    // The request: everything in the body, the profile included, the bitrate always set.
    let body = server.body(&format!("/Items/{ITEM}/PlaybackInfo"));
    assert_eq!(body["MaxStreamingBitrate"], 2_147_483_647);
    assert_eq!(body["DeviceProfile"]["MaxStreamingBitrate"], 2_147_483_647);
    assert_eq!(body["StartTimeTicks"], 900_000_000);
    assert_eq!(body["AudioStreamIndex"], 1);
    assert_eq!(body["SubtitleStreamIndex"], -1);
    assert_eq!(body["MediaSourceId"], SOURCE);
    assert_eq!(body["EnableDirectPlay"], true);
    assert_eq!(
        body["DeviceProfile"]["TranscodingProfiles"][0]["Container"],
        "mp4"
    );
    assert_eq!(
        body["DeviceProfile"]["TranscodingProfiles"][1]["Container"],
        "ts"
    );
    let received = server.received();
    assert!(received[0].query.is_empty());
    assert!(received[0].authorization.contains(TOKEN));

    assert_no_token(&log, &format!("{answer:?}"));
}

#[tokio::test]
async fn an_hls_transcode_reads_its_reasons_from_the_url() {
    let extra = format!(
        r#","SupportsDirectPlay":false,"TranscodingUrl":"{}","TranscodingSubProtocol":"hls","TranscodingContainer":"mp4""#,
        transcoding_url()
    );
    let server = StandIn::start_at("/jellyfin", handler(answer(&media_source(&extra)))).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let answer = client
        .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
        .await
        .unwrap();
    let (_, stream) = answer.playable(None).unwrap();
    assert_eq!(
        stream.route,
        Route::ServerStream {
            protocol: StreamProtocol::Hls,
            container: Some("mp4".into()),
            // The PGS subtitle is burned in, so the video is converted although HEVC could be copied.
            video: StreamAction::Transcode,
            audio: StreamAction::Copy,
        }
    );
    assert_eq!(stream.route.label(), RouteLabel::Transcode);
    assert_eq!(
        stream.reasons,
        vec![
            RouteReason::CONTAINER_NOT_SUPPORTED,
            RouteReason::SUBTITLE_CODEC_NOT_SUPPORTED
        ]
    );
    // On the address, under its base path, with the server's query (and its one ApiKey) unchanged.
    let url = stream.url.expose();
    let expected_prefix = format!(
        "http://127.0.0.1:{}/jellyfin/videos/{ITEM}/master.m3u8?",
        server.address.port().unwrap()
    );
    assert!(url.starts_with(&expected_prefix), "{url}");
    assert!(url.ends_with(&transcoding_url().split_once('?').unwrap().1.to_owned()));
    assert_eq!(url.matches("ApiKey=").count(), 1);
    assert_eq!(
        server.received()[0].path,
        format!("/Items/{ITEM}/PlaybackInfo")
    );

    assert_no_token(&log, &format!("{answer:?}"));
}

#[tokio::test]
async fn a_source_without_a_route_is_not_playable() {
    let server = StandIn::start(handler(answer(&media_source(
        r#","SupportsDirectPlay":false"#,
    ))))
    .await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let answer = client
        .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
        .await
        .unwrap();
    assert!(answer.sources[0].stream.is_none());
    assert_eq!(
        answer.playable(Some(SOURCE)),
        Err(PlaybackError::NotPlayable {
            reasons: Vec::new()
        })
    );
}

#[tokio::test]
async fn the_servers_error_codes_are_told_apart() {
    for (code, expected) in [
        (
            "NoCompatibleStream",
            PlaybackError::NotPlayable {
                reasons: Vec::new(),
            },
        ),
        ("NotAllowed", PlaybackError::NotAllowed),
        ("RateLimitExceeded", PlaybackError::RateLimited),
    ] {
        let body = format!(r#"{{"MediaSources":[],"ErrorCode":"{code}"}}"#);
        let server = StandIn::start(handler(body)).await;
        let log = Arc::new(RequestLog::default());
        let error = client(&server, &log)
            .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
            .await
            .unwrap_err();
        assert_eq!(error, expected);
    }
}

#[tokio::test]
async fn a_transcoding_url_on_another_host_is_refused() {
    let extra = format!(
        r#","SupportsDirectPlay":false,"TranscodingUrl":"https://media.example.com/videos/{ITEM}/master.m3u8?ApiKey={TOKEN}""#
    );
    let server = StandIn::start(handler(answer(&media_source(&extra)))).await;
    let log = Arc::new(RequestLog::default());
    let answer = client(&server, &log)
        .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
        .await
        .unwrap();
    assert!(answer.sources[0].stream.is_none());
}

#[tokio::test]
async fn a_401_ends_the_session() {
    let server = StandIn::start(|_| (401, String::new())).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let error = client
        .playback_info(&ItemId::new(ITEM), &request(), &capabilities())
        .await
        .unwrap_err();
    assert_eq!(error, PlaybackError::Auth(AuthError::SessionRevoked));
    assert!(!client.has_token());
    assert_no_token(&log, &format!("{error:?}"));
}

#[tokio::test]
async fn segments_map_to_kinds_and_durations() {
    let body = format!(
        r#"{{"Items":[
          {{"Id":"4f000000000000000000000000000001","ItemId":"{ITEM}","Type":"Recap","StartTicks":0,"EndTicks":450000000}},
          {{"Id":"4f000000000000000000000000000002","ItemId":"{ITEM}","Type":"Intro","StartTicks":600000000,"EndTicks":1500000000}},
          {{"Id":"4f000000000000000000000000000003","ItemId":"{ITEM}","Type":"Unknown","StartTicks":1600000000,"EndTicks":1700000000}},
          {{"Id":"4f000000000000000000000000000004","ItemId":"{ITEM}","Type":"Outro","StartTicks":69000000000,"EndTicks":72000000000}}
        ],"TotalRecordCount":4,"StartIndex":0}}"#
    );
    let server = StandIn::start(move |received| {
        if received.path == format!("/MediaSegments/{ITEM}") {
            (200, body.clone())
        } else {
            (404, String::new())
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let segments = client(&server, &log)
        .media_segments(&ItemId::new(ITEM))
        .await
        .unwrap();
    let kinds: Vec<_> = segments.iter().map(|s| s.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            SegmentKind::RECAP,
            SegmentKind::INTRO,
            SegmentKind::new("Unknown"),
            SegmentKind::OUTRO
        ]
    );
    assert_eq!(segments[1].start, Duration::from_secs(60));
    assert_eq!(segments[1].end, Duration::from_secs(150));
    assert_eq!(server.received()[0].method, "GET");
}

#[tokio::test]
async fn playstate_reports_carry_the_session_tracks_and_method() {
    let server = StandIn::start(handler(String::new())).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    let report = ProgressReport {
        item: ItemId::new(ITEM),
        media_source: SOURCE.into(),
        play_session_id: PLAY_SESSION.into(),
        position: Duration::from_millis(12_345),
        audio: Some(1),
        subtitle: None,
        paused: false,
        muted: false,
        volume: Some(80),
        can_seek: true,
        route: Route::DirectPlay,
    };
    client.report_start(&report).await.unwrap();
    let transcode = ProgressReport {
        paused: true,
        subtitle: Some(3),
        route: Route::ServerStream {
            protocol: StreamProtocol::Hls,
            container: Some("mp4".into()),
            video: StreamAction::Copy,
            audio: StreamAction::Copy,
        },
        ..report.clone()
    };
    client.report_progress(&transcode).await.unwrap();
    client
        .report_stopped(&StopReport {
            item: ItemId::new(ITEM),
            media_source: SOURCE.into(),
            play_session_id: PLAY_SESSION.into(),
            position: Duration::from_secs(600),
            failed: true,
        })
        .await
        .unwrap();

    assert_eq!(
        server.body("/Sessions/Playing"),
        serde_json::json!({
            "ItemId": ITEM, "MediaSourceId": SOURCE, "PlaySessionId": PLAY_SESSION, "PositionTicks": 123_450_000,
            "AudioStreamIndex": 1, "SubtitleStreamIndex": -1, "IsPaused": false, "IsMuted": false, "VolumeLevel": 80,
            "CanSeek": true, "PlayMethod": "DirectPlay"
        })
    );
    let progress = server.body("/Sessions/Playing/Progress");
    assert_eq!(progress["PlayMethod"], "Transcode");
    assert_eq!(progress["IsPaused"], true);
    assert_eq!(progress["SubtitleStreamIndex"], 3);
    assert_eq!(
        server.body("/Sessions/Playing/Stopped"),
        serde_json::json!({
            "ItemId": ITEM, "MediaSourceId": SOURCE, "PlaySessionId": PLAY_SESSION, "PositionTicks": 6_000_000_000_i64,
            "Failed": true
        })
    );
    assert_no_token(&log, "");
}

#[tokio::test]
async fn the_sessions_transcoding_facts_are_read_for_this_device() {
    let body = format!(
        r#"[
          {{"Id":"s0","DeviceId":"another-device","TranscodingInfo":{{"IsVideoDirect":false,"IsAudioDirect":false,"TranscodeReasons":["VideoCodecNotSupported"]}}}},
          {{"Id":"s1","UserId":"5d1c0000000000000000000000000001","Client":"Current","DeviceName":"Office PC","DeviceId":"{DEVICE_ID}",
            "PlayState":{{"PositionTicks":900000000,"CanSeek":true,"IsPaused":false,"IsMuted":false,"MediaSourceId":"{SOURCE}","PlayMethod":"Transcode","RepeatMode":"RepeatNone","PlaybackOrder":"Default"}},
            "TranscodingInfo":{{"AudioCodec":"eac3","VideoCodec":"hevc","Container":"mp4","IsVideoDirect":true,"IsAudioDirect":true,
              "Bitrate":50640000,"Framerate":23.976,"CompletionPercentage":4.2,"Width":3840,"Height":2160,"AudioChannels":6,
              "HardwareAccelerationType":"none","TranscodeReasons":["ContainerNotSupported"]}}}}
        ]"#
    );
    let server = StandIn::start(move |received| {
        if received.path == "/Sessions" {
            (200, body.clone())
        } else {
            (404, String::new())
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let facts = client(&server, &log)
        .transcoding_facts()
        .await
        .unwrap()
        .unwrap();
    assert!(facts.video_direct && facts.audio_direct);
    assert_eq!(facts.video_codec, Some(Codec::HEVC));
    assert_eq!(facts.hardware_acceleration, None);
    assert_eq!(facts.reasons, vec![RouteReason::CONTAINER_NOT_SUPPORTED]);
    assert_eq!(server.received()[0].query, format!("deviceId={DEVICE_ID}"));
}

#[tokio::test]
async fn the_users_playback_preferences_are_read() {
    let server = StandIn::start(|received| {
        if received.path == "/Users/Me" {
            (
                200,
                r#"{"Name":"Example User","Id":"5d1c0000000000000000000000000001","Configuration":{"AudioLanguagePreference":"ger",
                "PlayDefaultAudioTrack":false,"SubtitleLanguagePreference":"","DisplayMissingEpisodes":false,"GroupedFolders":[],
                "SubtitleMode":"Smart","DisplayCollectionsView":false,"EnableLocalPassword":false,"OrderedViews":[],
                "LatestItemsExcludes":[],"MyMediaExcludes":[],"HidePlayedInLatest":true,"RememberAudioSelections":true,
                "RememberSubtitleSelections":false,"EnableNextEpisodeAutoPlay":true}}"#
                    .into(),
            )
        } else {
            (404, String::new())
        }
    })
    .await;
    let log = Arc::new(RequestLog::default());
    let preferences = client(&server, &log).playback_preferences().await.unwrap();
    assert_eq!(preferences.audio_language.as_deref(), Some("ger"));
    assert!(!preferences.play_default_audio_track);
    assert_eq!(preferences.subtitle_language, None);
    assert_eq!(preferences.subtitle_mode, SubtitleMode::SMART);
    assert!(preferences.remember_audio && !preferences.remember_subtitles);
    assert!(preferences.next_episode_autoplay);
}

#[tokio::test]
async fn a_playstate_report_that_gets_a_401_answers_within_its_timeout() {
    let server = StandIn::start(|_| (401, String::new())).await;
    let log = Arc::new(RequestLog::default());
    let client = client(&server, &log);
    // A session hook that never decides: the app's check of the 401 has not ended.
    client.on_event(Arc::new(|_| {}));
    let started = tokio::time::Instant::now();
    let stopped = client
        .report_stopped(&StopReport {
            item: ItemId::new(ITEM),
            media_source: SOURCE.into(),
            play_session_id: PLAY_SESSION.into(),
            position: Duration::from_secs(600),
            failed: false,
        })
        .await;
    // The player awaits it in its loop and at exit: it answers at once, well within STOP_REPORT_TIMEOUT, and keeps the token.
    assert_eq!(stopped, Err(AuthError::Unconfirmed));
    assert!(started.elapsed() < current_jellyfin::STOP_REPORT_TIMEOUT);
    assert!(client.has_token());
}
