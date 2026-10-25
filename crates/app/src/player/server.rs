//! `current-jellyfin` as the player's [`MediaServer`] (ADR 0019): the session's client answers PlaybackInfo, takes the reports and
//! fetches segments, the next episode, the user's preferences and policy, the transcode's facts and external subtitles. Its errors
//! become flat [`PlaybackProblem`]s with no server text and no URL.
//!
//! The stream URL moves from `current-jellyfin`'s type into `current-playback`'s without being formatted, with the address's gateway
//! credentials and the session token (for redaction) as its [`StreamAuth`].

use std::sync::Arc;

use current_jellyfin::{
    AuthError, JellyfinClient, MAXIMUM_STREAMING_BITRATE, PlaybackError, PlaybackRequest,
};
use current_model::{
    EngineCapabilities, ItemRef, ItemSummary, MediaSource, MediaStream, PlaybackPreferences,
    PlaybackProblem, Segment, TranscodingFacts,
};
use current_net::NetError;
use current_playback::{
    MediaServer, PlanRequest, ProgressReport, Quality, ServerAnswer, StopReport, StreamAuth,
    StreamCookie, StreamUrl, TranscodePermissions,
};

/// A Jellyfin session as a media server for one player.
pub(crate) struct JellyfinMedia {
    client: Arc<JellyfinClient>,
}

impl JellyfinMedia {
    pub(crate) fn new(client: Arc<JellyfinClient>) -> Self {
        Self { client }
    }

    pub(crate) fn client(&self) -> &Arc<JellyfinClient> {
        &self.client
    }
}

/// What a Jellyfin error means for playback (ADR 0019, "Errors"; the 401 rule is the client's, ADR 0017).
pub(crate) fn problem(error: &AuthError) -> PlaybackProblem {
    match error {
        AuthError::Gateway(_) => PlaybackProblem::GatewaySignInNeeded,
        AuthError::SessionRevoked | AuthError::NotSignedIn | AuthError::WrongCredentials => {
            PlaybackProblem::SignedOut
        }
        AuthError::Refused => PlaybackProblem::Server { status: 403 },
        AuthError::Network(error) => match error {
            NetError::Status { status: 404, .. } => PlaybackProblem::NotFound,
            NetError::Status { status, .. } | NetError::Auth { status, .. } => {
                PlaybackProblem::Server { status: *status }
            }
            _ => PlaybackProblem::Network,
        },
        // A 401 being checked by the session (try again once it is done), and the sign-in errors, which playback never sees.
        _ => PlaybackProblem::Network,
    }
}

fn playback_problem(error: &PlaybackError) -> PlaybackProblem {
    match error {
        PlaybackError::NotPlayable { reasons } => PlaybackProblem::NotPlayable {
            reasons: reasons.clone(),
        },
        // The user's policy does not allow it: the server's own refusal.
        PlaybackError::NotAllowed => PlaybackProblem::Server { status: 403 },
        PlaybackError::RateLimited => PlaybackProblem::Server { status: 429 },
        PlaybackError::Auth(error) => problem(error),
    }
}

fn progress(report: &ProgressReport) -> current_jellyfin::ProgressReport {
    current_jellyfin::ProgressReport {
        item: report.item.id.clone(),
        media_source: report.media_source.clone(),
        play_session_id: report.play_session_id.clone(),
        position: report.position,
        audio: report.audio,
        subtitle: report.subtitle,
        paused: report.paused,
        muted: report.muted,
        volume: Some(report.volume),
        can_seek: report.can_seek,
        route: report.route.clone(),
    }
}

impl MediaServer for JellyfinMedia {
    async fn playback_info(
        &self,
        item: &ItemRef,
        request: &PlanRequest,
        capabilities: &EngineCapabilities,
    ) -> Result<ServerAnswer, PlaybackProblem> {
        let wire = PlaybackRequest {
            media_source: request.media_source.clone(),
            start: request.start,
            tracks: request.tracks,
            max_streaming_bitrate: match request.quality {
                Quality::Maximum => MAXIMUM_STREAMING_BITRATE,
                Quality::Bitrate(bits) => bits,
            },
            direct_play: request.direct_play,
        };
        let answer = self
            .client
            .playback_info(&item.id, &wire, capabilities)
            .await
            .map_err(|error| playback_problem(&error))?;
        let (source, stream) = answer
            .playable(request.media_source.as_deref())
            .map_err(|error| playback_problem(&error))?;
        let access = self.client.stream_access(&stream.url);
        Ok(ServerAnswer {
            play_session_id: answer.play_session_id.clone(),
            source: source.clone(),
            route: stream.route.clone(),
            reasons: stream.reasons.clone(),
            // Moved as it is: the URL is never formatted, logged or shown.
            url: StreamUrl::new(stream.url.expose().to_owned()),
            auth: StreamAuth {
                cookies: access
                    .cookies
                    .into_iter()
                    .map(|cookie| StreamCookie {
                        name: cookie.name,
                        value: cookie.value,
                        path: cookie.path,
                        secure: cookie.secure,
                    })
                    .collect(),
                headers: access.headers,
                secrets: access.secrets,
            },
        })
    }

    async fn report_start(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.client
            .report_start(&progress(report))
            .await
            .map_err(|error| problem(&error))
    }

    async fn report_progress(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.client
            .report_progress(&progress(report))
            .await
            .map_err(|error| problem(&error))
    }

    async fn report_stopped(&self, report: &StopReport) -> Result<(), PlaybackProblem> {
        let report = current_jellyfin::StopReport {
            item: report.item.id.clone(),
            media_source: report.media_source.clone(),
            play_session_id: report.play_session_id.clone(),
            position: report.position,
            failed: report.failed,
        };
        self.client
            .report_stopped(&report)
            .await
            .map_err(|error| problem(&error))
    }

    async fn segments(&self, item: &ItemRef) -> Result<Vec<Segment>, PlaybackProblem> {
        self.client
            .media_segments(&item.id)
            .await
            .map_err(|error| problem(&error))
    }

    async fn next_episode(&self, item: &ItemRef) -> Result<Option<ItemSummary>, PlaybackProblem> {
        self.client
            .next_episode(item)
            .await
            .map_err(|error| problem(&error))
    }

    async fn preferences(&self) -> Result<PlaybackPreferences, PlaybackProblem> {
        self.client
            .playback_preferences()
            .await
            .map_err(|error| problem(&error))
    }

    async fn permissions(&self) -> Result<TranscodePermissions, PlaybackProblem> {
        let policy = self
            .client
            .transcode_policy()
            .await
            .map_err(|error| problem(&error))?;
        Ok(TranscodePermissions {
            video_transcoding: policy.video_transcoding,
            remuxing: policy.remuxing,
            force_remote_source_transcoding: policy.force_remote_source_transcoding,
            remote_bitrate_limit: policy.remote_bitrate_limit,
        })
    }

    async fn transcoding_facts(&self) -> Result<Option<TranscodingFacts>, PlaybackProblem> {
        self.client
            .transcoding_facts()
            .await
            .map_err(|error| problem(&error))
    }

    async fn external_subtitle(
        &self,
        item: &ItemRef,
        source: &MediaSource,
        stream: &MediaStream,
    ) -> Result<Vec<u8>, PlaybackProblem> {
        self.client
            .subtitle(&item.id, &source.id, stream.index, stream.codec.as_ref())
            .await
            .map_err(|error| problem(&error))
    }

    fn is_local_address(&self) -> bool {
        self.client.address_is_local()
    }
}
