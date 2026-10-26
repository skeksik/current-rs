//! The route planner (ADR 0019, "Route planner"): what to ask the server, and the plan from its answer.
//!
//! Jellyfin 12.2 answers either direct play or a server stream (`TranscodingUrl`); it offers no direct stream, so a remux is a
//! transcode that copies both streams. The answer's route is taken as it is: direct play when the server offers it, else the server
//! stream, whose format (HLS with fMP4 segments, then TS) the device profile ranks (choice 2). After start the server's transcoding
//! facts correct what the stream copies.

use std::time::Duration;

use current_model::{
    ItemRef, MediaSource, PlaybackPlan, Route, RouteReason, StreamAction, SubtitleChoice,
    TrackChoice, TranscodingFacts,
};

use crate::quality::Quality;
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

/// The plan from the server's answer to `request`. `own` are Current's reasons for this plan (a fallback, a user's choice); they are
/// kept only when the route is not direct play, like the quality limit.
pub fn plan(
    item: &ItemRef,
    request: &PlanRequest,
    answer: ServerAnswer,
    own: &[RouteReason],
    permissions: &TranscodePermissions,
    local: bool,
) -> Planned {
    let ServerAnswer {
        play_session_id,
        source,
        route,
        reasons: server_reasons,
        url,
    } = answer;
    let mut reasons = Vec::new();
    let mut notes = Vec::new();
    if !route.is_direct_play() {
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
    }
    let tracks = resolve_tracks(&source, request.tracks);
    Planned {
        plan: PlaybackPlan {
            item: item.clone(),
            source,
            play_session_id,
            route,
            reasons,
            start: request.start,
            tracks,
        },
        url,
        notes,
    }
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
        Route::DirectPlay => Route::DirectPlay,
        Route::ServerStream {
            protocol,
            container,
            ..
        } => Route::ServerStream {
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
    use current_model::{RouteLabel, StreamProtocol};

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
            answer(Route::DirectPlay, vec![]),
            &[RouteReason::FALLBACK_AFTER_ENGINE_ERROR],
            &TranscodePermissions::default(),
            true,
        );
        assert_eq!(planned.plan.route, Route::DirectPlay);
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
        );
        assert_eq!(planned.plan.route.label(), RouteLabel::DirectStream);
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
        );
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
        let predicted = hls(StreamAction::Transcode, StreamAction::Transcode);
        let facts = TranscodingFacts {
            video_direct: true,
            audio_direct: true,
            ..TranscodingFacts::default()
        };
        assert_eq!(correct(&predicted, &facts).label(), RouteLabel::Remux);
        assert_eq!(correct(&Route::DirectPlay, &facts), Route::DirectPlay);
        let Route::ServerStream { protocol, .. } = correct(&predicted, &facts) else {
            panic!("a server stream stays one");
        };
        assert_eq!(protocol, StreamProtocol::Hls);
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
        );
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
        );
        assert_eq!(local.notes, vec![PlanNote::RemoteSourceTranscodingForced]);
        let lower = plan(
            &item("e1"),
            &ask(Quality::Bitrate(4_000_000)),
            remote,
            &[],
            &permissions,
            false,
        );
        assert_eq!(lower.notes, vec![PlanNote::RemoteSourceTranscodingForced]);
    }
}
