//! The route planner (ADR 0019, "Route planner"): what to ask the server, and the plan from its answer.
//!
//! Jellyfin 12.2 answers either direct play or a server stream (`TranscodingUrl`); it offers no direct stream, so a remux is a
//! transcode that copies both streams. Direct play goes to the first local route the per-file check allows ([`crate::route`]: native,
//! remux on the device, app engine; ADR 0025), a server stream to the first engine that takes it. With the app engine only (ADR
//! 0019), the answer is taken as it is. The stream's format (HLS with fMP4 segments, then TS) is ranked by the device profile (choice
//! 2). After start the server's transcoding facts correct what the stream copies.

use std::time::Duration;

use current_model::{
    ItemRef, MediaSource, PlaybackPlan, Route, RouteReason, StreamAction, SubtitleChoice,
    TrackChoice, TranscodingFacts,
};

use crate::engines::Engines;
use crate::quality::Quality;
use crate::route::{self, RouteContext};
use crate::server::{PlanRequest, ServerAnswer, TranscodePermissions};
use crate::stream::StreamUrl;

/// Why a plan came out the way it did, from the user's permissions on the server, which the server enforces (ADR 0019,
/// "Permissions"). For the playback information view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlanNote {
    /// The server limits streaming to remote addresses to this many bits per second.
    RemoteBitrateLimit { limit: u32 },
    /// The server does not let the user transcode video.
    TranscodingNotAllowed,
    /// The server does not let the user remux.
    RemuxingNotAllowed,
    /// The server transcodes media that lives elsewhere for this user.
    RemoteSourceTranscodingForced,
}

/// The request for a plan.
pub fn request(
    media_source: Option<String>,
    start: Duration,
    tracks: TrackChoice,
    quality: Quality,
    direct_play: bool,
) -> PlanRequest {
    PlanRequest {
        media_source,
        start,
        tracks,
        quality,
        direct_play,
    }
}

/// A plan with the server's URL for its stream ([`MediaServer::stream_url`](crate::MediaServer::stream_url) makes the engine's).
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub plan: PlaybackPlan,
    pub url: StreamUrl,
    pub notes: Vec<PlanNote>,
}

/// What decides the route besides the server's answer: the player's engines, the display and the user's choices, and the routes
/// that already failed in this play.
#[derive(Clone, Copy, Debug)]
pub struct RouteInputs<'a> {
    pub engines: &'a Engines,
    pub context: RouteContext,
    pub tried: &'a [Route],
}

/// The plan from the server's answer to `request`, or `None` when no route plays it: a direct-play answer that no untried local route
/// plays (the caller asks again without direct play, ADR 0025 step 4), or a stream no untried engine takes. `own` are Current's
/// reasons for a server stream (a fallback, a user's choice), kept only on one, like the quality limit.
pub fn plan(
    item: &ItemRef,
    request: &PlanRequest,
    answer: ServerAnswer,
    own: &[RouteReason],
    permissions: &TranscodePermissions,
    local: bool,
    inputs: RouteInputs<'_>,
) -> Option<Planned> {
    let ServerAnswer {
        play_session_id,
        source,
        offer,
        reasons: server_reasons,
        url,
    } = answer;
    let tracks = resolve_tracks(&source, request.tracks);
    let mut reasons = Vec::new();
    let mut native_lacks = Vec::new();
    let mut notes = Vec::new();
    let route = if offer.is_direct_play() {
        let choice = route::local_routes(
            &source,
            tracks,
            inputs.engines,
            inputs.context,
            inputs.tried,
        )
        .into_iter()
        .next()?;
        reasons = choice.reasons;
        native_lacks = choice.native_lacks;
        choice.route
    } else {
        let route = route::server_routes(
            &offer,
            &source,
            tracks,
            inputs.engines,
            inputs.context,
            inputs.tried,
        )
        .into_iter()
        .next()?;
        reasons.extend(own.iter().cloned());
        if request.quality.limits(source.bitrate) {
            reasons.push(RouteReason::QUALITY_LIMIT);
        }
        for reason in server_reasons {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
        if let Some(limit) = permissions.remote_bitrate_limit
            && !local
            && effective_limit(request.quality) > limit
            && Quality::Bitrate(limit).limits(source.bitrate)
        {
            notes.push(PlanNote::RemoteBitrateLimit { limit });
        }
        if source.is_remote && permissions.force_remote_source_transcoding {
            notes.push(PlanNote::RemoteSourceTranscodingForced);
        }
        route
    };
    Some(Planned {
        plan: PlaybackPlan {
            item: item.clone(),
            source,
            play_session_id,
            route,
            reasons,
            native_lacks,
            failed_routes: inputs.tried.to_vec(),
            start: request.start,
            tracks,
        },
        url,
        notes,
    })
}

fn effective_limit(quality: Quality) -> u32 {
    match quality {
        Quality::Maximum => u32::MAX,
        Quality::Bitrate(bitrate) => bitrate,
    }
}

/// What the permissions explain when the server offers no route.
pub fn not_playable_notes(permissions: &TranscodePermissions) -> Vec<PlanNote> {
    let mut notes = Vec::new();
    if !permissions.video_transcoding {
        notes.push(PlanNote::TranscodingNotAllowed);
    }
    if !permissions.remuxing {
        notes.push(PlanNote::RemuxingNotAllowed);
    }
    notes
}

/// The user's choice with the server's picks filled in: the answer's default audio and subtitle (the Jellyfin user's own preferences
/// and remembered choices, ADR 0019 choice 6) where the user chose nothing.
pub fn resolve_tracks(source: &MediaSource, choice: TrackChoice) -> TrackChoice {
    TrackChoice {
        audio: choice.audio.or(source.default_audio),
        subtitle: match choice.subtitle {
            SubtitleChoice::Default => source
                .default_subtitle
                .map_or(SubtitleChoice::Off, SubtitleChoice::Stream),
            other => other,
        },
    }
}

/// The route corrected by the server's facts about its transcode: what it really copies.
pub fn correct(route: &Route, facts: &TranscodingFacts) -> Route {
    match route {
        Route::Native | Route::DeviceRemux | Route::AppEngine => route.clone(),
        Route::ServerStream {
            engine,
            protocol,
            container,
            ..
        } => Route::ServerStream {
            engine: *engine,
            protocol: *protocol,
            container: container.clone(),
            video: action(facts.video_direct),
            audio: action(facts.audio_direct),
        },
    }
}

fn action(direct: bool) -> StreamAction {
    if direct {
        StreamAction::Copy
    } else {
        StreamAction::Transcode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{answer, hls, item, source};
    use current_model::{EngineRole, RouteLabel, ServerOffer, StreamProtocol};
    use std::sync::LazyLock;

    /// Today's player: the app engine only.
    fn app_only() -> RouteInputs<'static> {
        static ENGINES: LazyLock<Engines> = LazyLock::new(Engines::default);
        RouteInputs {
            engines: &ENGINES,
            context: RouteContext::default(),
            tried: &[],
        }
    }

    fn ask(quality: Quality) -> PlanRequest {
        request(
            None,
            Duration::from_secs(30),
            TrackChoice::default(),
            quality,
            true,
        )
    }

    #[test]
    fn direct_play_keeps_no_reasons() {
        let planned = plan(
            &item("e1"),
            &ask(Quality::Bitrate(2_000_000)),
            answer(ServerOffer::DirectPlay, vec![]),
            &[RouteReason::FALLBACK_AFTER_ENGINE_ERROR],
            &TranscodePermissions::default(),
            true,
            app_only(),
        )
        .expect("a route");
        assert_eq!(planned.plan.route, Route::AppEngine);
        assert!(planned.plan.reasons.is_empty());
        assert_eq!(planned.plan.start, Duration::from_secs(30));
        assert_eq!(planned.plan.play_session_id, "ps-1");
        assert!(planned.notes.is_empty());
    }

    #[test]
    fn a_server_stream_keeps_the_servers_reasons_and_ours() {
        let planned = plan(
            &item("e1"),
            &ask(Quality::Bitrate(8_000_000)),
            answer(
                hls(StreamAction::Copy, StreamAction::Transcode),
                vec![
                    RouteReason::AUDIO_CODEC_NOT_SUPPORTED,
                    RouteReason::new("SomeFutureReason"),
                ],
            ),
            &[RouteReason::FALLBACK_AFTER_ENGINE_ERROR],
            &TranscodePermissions::default(),
            true,
            app_only(),
        )
        .expect("a route");
        assert_eq!(planned.plan.route.label(), RouteLabel::ServerDirectStream);
        assert_eq!(
            planned.plan.reasons,
            vec![
                RouteReason::FALLBACK_AFTER_ENGINE_ERROR,
                RouteReason::QUALITY_LIMIT,
                RouteReason::AUDIO_CODEC_NOT_SUPPORTED,
                RouteReason::new("SomeFutureReason"),
            ]
        );
    }

    #[test]
    fn maximum_quality_adds_no_limit() {
        let planned = plan(
            &item("e1"),
            &ask(Quality::Maximum),
            answer(
                hls(StreamAction::Transcode, StreamAction::Transcode),
                vec![RouteReason::VIDEO_CODEC_NOT_SUPPORTED],
            ),
            &[],
            &TranscodePermissions::default(),
            true,
            app_only(),
        )
        .expect("a route");
        assert_eq!(
            planned.plan.reasons,
            vec![RouteReason::VIDEO_CODEC_NOT_SUPPORTED]
        );
    }

    #[test]
    fn the_answers_defaults_fill_in_the_tracks() {
        let mut source = source();
        source.default_audio = Some(1);
        source.default_subtitle = Some(3);
        assert_eq!(
            resolve_tracks(&source, TrackChoice::default()),
            TrackChoice {
                audio: Some(1),
                subtitle: SubtitleChoice::Stream(3)
            }
        );
        let chosen = TrackChoice {
            audio: Some(2),
            subtitle: SubtitleChoice::Off,
        };
        assert_eq!(resolve_tracks(&source, chosen), chosen);
        source.default_subtitle = None;
        assert_eq!(
            resolve_tracks(&source, TrackChoice::default()).subtitle,
            SubtitleChoice::Off
        );
    }

    #[test]
    fn the_servers_facts_correct_the_label() {
        let predicted = Route::server_stream(
            &hls(StreamAction::Transcode, StreamAction::Transcode),
            EngineRole::App,
        )
        .expect("a stream");
        let facts = TranscodingFacts {
            video_direct: true,
            audio_direct: true,
            ..TranscodingFacts::default()
        };
        assert_eq!(correct(&predicted, &facts).label(), RouteLabel::ServerRemux);
        assert_eq!(correct(&Route::AppEngine, &facts), Route::AppEngine);
        assert_eq!(correct(&Route::Native, &facts), Route::Native);
        let Route::ServerStream {
            protocol, engine, ..
        } = correct(&predicted, &facts)
        else {
            panic!("a server stream stays one");
        };
        assert_eq!((protocol, engine), (StreamProtocol::Hls, EngineRole::App));
    }

    #[test]
    fn permissions_explain_limits_and_refusals() {
        let permissions = TranscodePermissions {
            video_transcoding: false,
            remuxing: false,
            force_remote_source_transcoding: true,
            remote_bitrate_limit: Some(8_000_000),
        };
        assert_eq!(
            not_playable_notes(&permissions),
            vec![
                PlanNote::TranscodingNotAllowed,
                PlanNote::RemuxingNotAllowed
            ]
        );
        let mut remote = answer(hls(StreamAction::Transcode, StreamAction::Copy), vec![]);
        remote.source.is_remote = true;
        remote.source.bitrate = Some(20_000_000);
        let planned = plan(
            &item("e1"),
            &ask(Quality::Maximum),
            remote.clone(),
            &[],
            &permissions,
            false,
            app_only(),
        )
        .expect("a route");
        assert_eq!(
            planned.notes,
            vec![
                PlanNote::RemoteBitrateLimit { limit: 8_000_000 },
                PlanNote::RemoteSourceTranscodingForced
            ]
        );
        // On a local address, or below the user's own lower limit, the server's remote limit explains nothing.
        let local = plan(
            &item("e1"),
            &ask(Quality::Maximum),
            remote.clone(),
            &[],
            &permissions,
            true,
            app_only(),
        )
        .expect("a route");
        assert_eq!(local.notes, vec![PlanNote::RemoteSourceTranscodingForced]);
        let lower = plan(
            &item("e1"),
            &ask(Quality::Bitrate(4_000_000)),
            remote,
            &[],
            &permissions,
            false,
            app_only(),
        )
        .expect("a route");
        assert_eq!(lower.notes, vec![PlanNote::RemoteSourceTranscodingForced]);
    }

    #[test]
    fn with_a_native_engine_the_check_picks_the_route_and_says_why() {
        use crate::engines::{EngineProfile, PlatformRules};
        use current_model::{
            Codec, CodecSupport, Container, EngineCapabilities, GapKind, TrackGap,
        };

        let caps = |audio: &[Codec]| EngineCapabilities {
            containers: vec![Container::MKV, Container::MP4],
            video: vec![CodecSupport::any(Codec::H264)],
            audio: audio.iter().cloned().map(CodecSupport::any).collect(),
            subtitles: vec![],
            external_subtitles: true,
        };
        let engines = Engines {
            native: Some(EngineProfile::new(caps(&[Codec::AAC]), true)),
            app: EngineProfile::new(caps(&[Codec::AAC, Codec::TRUEHD]), true),
            remux: None,
            rules: PlatformRules::WINDOWS,
        };
        let inputs = |tried| RouteInputs {
            engines: &engines,
            context: RouteContext::default(),
            tried,
        };
        let mut direct = answer(ServerOffer::DirectPlay, vec![]);
        direct.source.streams[0].codec = Some(Codec::H264);
        direct.source.streams[1].codec = Some(Codec::AAC);
        direct.source.streams[2].codec = Some(Codec::TRUEHD);
        let permissions = TranscodePermissions::default();
        let request = ask(Quality::Maximum);
        let planned = plan(
            &item("e1"),
            &request,
            direct.clone(),
            &[],
            &permissions,
            true,
            inputs(&[]),
        )
        .expect("a route");
        assert_eq!(planned.plan.route, Route::Native);
        assert_eq!(
            planned.plan.reasons,
            vec![RouteReason::NATIVE_FULLY_COMPATIBLE]
        );
        // The native route failed: the app engine, which lists it as failed.
        let planned = plan(
            &item("e1"),
            &request,
            direct.clone(),
            &[],
            &permissions,
            true,
            inputs(&[Route::Native]),
        )
        .expect("a route");
        assert_eq!(planned.plan.route, Route::AppEngine);
        assert_eq!(planned.plan.failed_routes, vec![Route::Native]);
        // TrueHD selected: the app engine, with what the native one lacks.
        let truehd = PlanRequest {
            tracks: TrackChoice {
                audio: Some(2),
                subtitle: SubtitleChoice::Off,
            },
            ..request.clone()
        };
        let planned = plan(
            &item("e1"),
            &truehd,
            direct.clone(),
            &[],
            &permissions,
            true,
            inputs(&[]),
        )
        .expect("a route");
        assert_eq!(planned.plan.route, Route::AppEngine);
        assert_eq!(planned.plan.reasons, vec![RouteReason::NATIVE_LACKS]);
        assert_eq!(
            planned.plan.native_lacks,
            vec![TrackGap {
                kind: GapKind::Audio,
                name: Some("truehd".into())
            }]
        );
        // Both tried: no local route, so the caller asks for the server's stream.
        assert!(
            plan(
                &item("e1"),
                &request,
                direct,
                &[],
                &permissions,
                true,
                inputs(&[Route::Native, Route::AppEngine]),
            )
            .is_none()
        );
        // A server stream goes to the native engine first, with the server's reasons.
        let mut stream = answer(
            hls(StreamAction::Copy, StreamAction::Transcode),
            vec![RouteReason::AUDIO_CODEC_NOT_SUPPORTED],
        );
        stream.source = planned_source();
        let planned = plan(
            &item("e1"),
            &truehd,
            stream,
            &[],
            &permissions,
            true,
            inputs(&[]),
        )
        .expect("a route");
        assert_eq!(planned.plan.route.engine(), EngineRole::Native);
        assert_eq!(
            planned.plan.reasons,
            vec![RouteReason::AUDIO_CODEC_NOT_SUPPORTED]
        );
    }

    fn planned_source() -> MediaSource {
        let mut source = source();
        source.streams[0].codec = Some(current_model::Codec::H264);
        source
    }
}
