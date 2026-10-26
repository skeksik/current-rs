//! `current-jellyfin` as the player's [`MediaServer`] (ADR 0019): the session's client answers PlaybackInfo, takes the reports and
//! fetches segments, the next episode, the user's preferences and policy, the transcode's facts and external subtitles. Its errors
//! become flat [`PlaybackProblem`]s with no server text and no URL.
//!
//! The stream URL moves from `current-jellyfin`'s type into `current-playback`'s without being formatted. On an address with gateway
//! credentials the engine gets a URL on the loopback relay instead (ADR 0020): a route per stream, made when the controller loads
//! it and removed when it ends or another replaces it, whose events go to the player's driver.
//!
//! One player plays items of any signed-in server, one at a time: the driver points it at the item's session before each play, after
//! stopping an item of another server (so its stop report goes to its own server).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use current_jellyfin::{
    AuthError, JellyfinClient, MAXIMUM_STREAMING_BITRATE, PlaybackError, PlaybackRequest,
};
use current_model::{
    EngineCapabilities, ItemRef, ItemSummary, MediaSource, MediaStream, PlaybackPreferences,
    PlaybackProblem, Segment, TranscodingFacts,
};
use current_net::{GatewayShape, NetError};
use current_playback::{
    EngineStream, MediaServer, PlanRequest, ProgressReport, Quality, ServerAnswer, StopReport,
    StreamAuth, StreamUrl, TranscodePermissions,
};
use current_relay::{RelayEvent, Route, RouteConfig};
use tokio::sync::mpsc;

use super::relay::{RelaySlot, RouteEvents, SessionUpstream};

/// The session of the item playing, as a media server for one player.
pub(crate) struct JellyfinMedia {
    /// The session's id in the app and its client.
    session: Mutex<Option<(u64, Arc<JellyfinClient>)>>,
    relay: Arc<RelaySlot>,
    /// The relay route of the stream given last, by its number.
    route: Mutex<Option<(u64, Route)>>,
    last_route: AtomicU64,
    events: mpsc::UnboundedSender<(u64, RelayEvent)>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl JellyfinMedia {
    /// A media server whose relay routes report to `events`.
    pub(crate) fn new(
        relay: Arc<RelaySlot>,
        events: mpsc::UnboundedSender<(u64, RelayEvent)>,
    ) -> Self {
        Self {
            session: Mutex::new(None),
            relay,
            route: Mutex::new(None),
            last_route: AtomicU64::new(0),
            events,
        }
    }

    /// The session in use, if any.
    pub(crate) fn client(&self) -> Option<Arc<JellyfinClient>> {
        lock(&self.session)
            .as_ref()
            .map(|(_, client)| client.clone())
    }

    /// The session in use with its id in the app.
    pub(crate) fn session_with_id(&self) -> Option<(u64, Arc<JellyfinClient>)> {
        lock(&self.session).clone()
    }

    pub(crate) fn set_client(&self, id: u64, client: Arc<JellyfinClient>) {
        *lock(&self.session) = Some((id, client));
    }

    fn session(&self) -> Result<Arc<JellyfinClient>, PlaybackProblem> {
        self.client().ok_or(PlaybackProblem::SignedOut)
    }

    /// Whether `route` is the relay route of the stream given last and still played.
    pub(crate) fn is_current_route(&self, route: u64) -> bool {
        lock(&self.route)
            .as_ref()
            .is_some_and(|(number, _)| *number == route)
    }

    /// Removes the relay route, if any. The route is dropped outside the lock: dropping aborts its connections.
    fn remove_route(&self) {
        let route = lock(&self.route).take();
        drop(route);
    }

    /// A relay route for `path` (on the address in use, without the token) of `item`'s streams.
    fn relay(
        &self,
        client: Arc<JellyfinClient>,
        item: &ItemRef,
        path: &str,
    ) -> Result<EngineStream, PlaybackProblem> {
        let relay = self.relay.relay().ok_or(PlaybackProblem::LocalRelay)?;
        let number = self.last_route.fetch_add(1, Ordering::Relaxed) + 1;
        let id = item.id.as_str();
        let route = relay
            .add_route(RouteConfig {
                upstream: Arc::new(SessionUpstream(client)),
                // Only the item's own streams: Jellyfin's stream, HLS and segment endpoints are under these (ADR 0020).
                prefixes: vec![format!("/Videos/{id}/"), format!("/Audio/{id}/")],
                events: Arc::new(RouteEvents {
                    route: number,
                    sender: self.events.clone(),
                }),
            })
            // No port, no random bytes, or an item id the relay cannot serve: no stream connection for it.
            .map_err(|_| PlaybackProblem::LocalRelay)?;
        let stream = EngineStream {
            url: StreamUrl::new(route.url(path)),
            auth: StreamAuth {
                secrets: vec![route.token().to_owned()],
            },
            relayed: true,
        };
        *lock(&self.route) = Some((number, route));
        Ok(stream)
    }
}

/// What a relay event means for playback (ADR 0020, "Lifecycle"); a refusal is given back with its status and shape, for the 401
/// rule to decide.
pub(crate) fn relay_problem(event: RelayEvent) -> Result<PlaybackProblem, (u16, GatewayShape)> {
    match event {
        RelayEvent::Refused { status, shape } => Err((status, shape)),
        RelayEvent::NotFound => Ok(PlaybackProblem::NotFound),
        RelayEvent::Server { status } => Ok(PlaybackProblem::Server { status }),
        RelayEvent::Network => Ok(PlaybackProblem::Network),
        RelayEvent::PlaylistElsewhere | RelayEvent::PlaylistUnreadable => {
            Ok(PlaybackProblem::PlaylistElsewhere)
        }
        // The request could not be built for the address in use: nothing the user can do but play again.
        RelayEvent::InvalidRequest => Ok(PlaybackProblem::Network),
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
        // Jellyfin refused this item to the user while the session goes on: gone, or hidden from the user.
        AuthError::NotAvailable => PlaybackProblem::NotFound,
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
        let client = self.session()?;
        let answer = client
            .playback_info(&item.id, &wire, capabilities)
            .await
            .map_err(|error| playback_problem(&error))?;
        let (source, stream) = answer
            .playable(request.media_source.as_deref())
            .map_err(|error| playback_problem(&error))?;
        Ok(ServerAnswer {
            play_session_id: answer.play_session_id.clone(),
            source: source.clone(),
            route: stream.route.clone(),
            reasons: stream.reasons.clone(),
            // Moved as it is: the URL is never formatted, logged or shown.
            url: StreamUrl::new(stream.url.expose().to_owned()),
        })
    }

    async fn report_start(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.session()?
            .report_start(&progress(report))
            .await
            .map_err(|error| problem(&error))
    }

    async fn report_progress(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.session()?
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
        self.session()?
            .report_stopped(&report)
            .await
            .map_err(|error| problem(&error))
    }

    async fn segments(&self, item: &ItemRef) -> Result<Vec<Segment>, PlaybackProblem> {
        self.session()?
            .media_segments(&item.id)
            .await
            .map_err(|error| problem(&error))
    }

    async fn next_episode(&self, item: &ItemRef) -> Result<Option<ItemSummary>, PlaybackProblem> {
        self.session()?
            .next_episode(item)
            .await
            .map_err(|error| problem(&error))
    }

    async fn preferences(&self) -> Result<PlaybackPreferences, PlaybackProblem> {
        self.session()?
            .playback_preferences()
            .await
            .map_err(|error| problem(&error))
    }

    async fn permissions(&self) -> Result<TranscodePermissions, PlaybackProblem> {
        let policy = self
            .session()?
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
        self.session()?
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
        self.session()?
            .subtitle(&item.id, &source.id, stream.index, stream.codec.as_ref())
            .await
            .map_err(|error| problem(&error))
    }

    fn stream_url(&self, item: &ItemRef, url: &StreamUrl) -> Result<EngineStream, PlaybackProblem> {
        self.remove_route();
        let client = self.session()?;
        if !client.address_has_gateway() {
            return Ok(EngineStream {
                url: url.clone(),
                auth: StreamAuth {
                    secrets: client.stream_secrets(),
                },
                relayed: false,
            });
        }
        // The server moved to another address since the plan: a new plan gets a URL on the new one.
        let path = client
            .stream_path(url.expose())
            .ok_or(PlaybackProblem::Network)?;
        self.relay(client, item, &path)
    }

    fn end_stream(&self) {
        self.remove_route();
    }

    fn is_local_address(&self) -> bool {
        self.client()
            .is_some_and(|client| client.address_is_local())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_refused_to_a_session_that_goes_on_is_not_found() {
        // ADR 0015 as amended 2026-10-25: a 401 to a token that still works is about the one item.
        assert_eq!(problem(&AuthError::NotAvailable), PlaybackProblem::NotFound);
        assert_eq!(
            problem(&AuthError::SessionRevoked),
            PlaybackProblem::SignedOut
        );
    }
}
