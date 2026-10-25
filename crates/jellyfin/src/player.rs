//! What a player needs from Jellyfin besides PlaybackInfo and the reports (ADR 0019): the gateway credentials and secrets for a stream
//! URL, the user's transcoding policy (read only to explain an outcome), and external subtitle files, which the core fetches itself so
//! the engine never does.
//!
//! Server behaviour relied on, from Jellyfin `v12.2` (commit ca0f16eb, read 2026-10-22 and 2026-10-25; quotes in
//! `docs/notes/playback.md` and `docs/notes/macos-subtitles.md`):
//! - Subtitles: `GET Videos/{itemId}/{mediaSourceId}/Subtitles/{index}/Stream.{format}` (`SubtitleController.cs:208`). Asking for the
//!   codec's own format returns the file as it is ("Return the original if the same format is being requested",
//!   `SubtitleEncoder.cs:139-146`); PGS and VobSub are always delivered raw (`SubtitleController.cs:236-247`), embedded PGS as `.sup`
//!   and VobSub as `.mks` (`SubtitleEncoder.cs:487-497`).
//! - Policy: `UserPolicy` has `EnableVideoPlaybackTranscoding`, `EnablePlaybackRemuxing` (default true) and
//!   `ForceRemoteSourceTranscoding` (default false), and `RemoteClientBitrateLimit` in bits per second.

use current_model::{Codec, ItemId};
use current_net::{Request, StreamCookie, Url};
use serde::Deserialize;

use crate::auth;
use crate::client::{AuthError, JellyfinClient};
use crate::playback::StreamUrl;

/// What the engine needs besides the URL to open a stream: the address's gateway credentials for it, and the values the URL holds
/// (the session token) so the engine can redact them from its log. Debug shows names and counts only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StreamAccess {
    /// Header name and value: the gateway's Basic `Authorization` header and its extra headers.
    pub headers: Vec<(String, String)>,
    pub cookies: Vec<StreamCookie>,
    pub secrets: Vec<String>,
}

impl std::fmt::Debug for StreamAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamAccess")
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("cookies", &self.cookies)
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// What the user's policy says about transcoding. The server enforces it; the player reads it only to explain an outcome (ADR 0019,
/// "Permissions").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TranscodePolicy {
    pub video_transcoding: bool,
    pub remuxing: bool,
    pub force_remote_source_transcoding: bool,
    /// Bits per second for addresses the server counts as remote; `None` without a limit.
    pub remote_bitrate_limit: Option<u32>,
}

/// `UserDto` with its `Policy`, the transcoding fields only.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserWithPolicy {
    policy: Option<PolicyDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyDto {
    enable_video_playback_transcoding: Option<bool>,
    enable_playback_remuxing: Option<bool>,
    force_remote_source_transcoding: Option<bool>,
    remote_client_bitrate_limit: Option<i64>,
}

impl JellyfinClient {
    /// The gateway credentials and secrets for `url`, a stream URL this client made: those of the address in use, when `url` is on it
    /// (`current-net`'s rule, ADR 0017), and the session token, which the URL carries as `ApiKey`.
    pub fn stream_access(&self, url: &StreamUrl) -> StreamAccess {
        let credentials = Url::parse(url.expose())
            .map(|parsed| self.http.stream_credentials(&parsed))
            .unwrap_or_default();
        StreamAccess {
            headers: credentials.headers,
            cookies: credentials.cookies,
            secrets: auth::read(&self.token)
                .map(|token| vec![token.expose().to_owned()])
                .unwrap_or_default(),
        }
    }

    /// The signed-in user's transcoding policy (`GET /Users/Me`, its `Policy`). A field the server leaves out reads as its default.
    pub async fn transcode_policy(&self) -> Result<TranscodePolicy, AuthError> {
        let user: UserWithPolicy = self
            .send_authenticated(Request::get("/Users/Me"))
            .await?
            .json()?;
        let policy = user.policy;
        let flag = |read: fn(&PolicyDto) -> Option<bool>, default: bool| {
            policy.as_ref().and_then(read).unwrap_or(default)
        };
        Ok(TranscodePolicy {
            video_transcoding: flag(|p| p.enable_video_playback_transcoding, true),
            remuxing: flag(|p| p.enable_playback_remuxing, true),
            force_remote_source_transcoding: flag(|p| p.force_remote_source_transcoding, false),
            // Zero is no limit (the policy's default; that the server treats it so is not verified).
            remote_bitrate_limit: policy
                .as_ref()
                .and_then(|p| p.remote_client_bitrate_limit)
                .filter(|limit| *limit > 0)
                .map(|limit| u32::try_from(limit).unwrap_or(u32::MAX)),
        })
    }

    /// An external or embedded subtitle stream of `item`'s source as a file, in its own format (module doc), through this client: the
    /// address's credentials apply, and the request never leaves the address. A subtitle the server keeps on another host (an external
    /// file whose path is an http URL) is not fetched from there.
    pub async fn subtitle(
        &self,
        item: &ItemId,
        media_source: &str,
        index: u32,
        codec: Option<&Codec>,
    ) -> Result<Vec<u8>, AuthError> {
        let path = format!(
            "/Videos/{}/{}/Subtitles/{index}/Stream.{}",
            item.as_str(),
            media_source,
            subtitle_format(codec),
        );
        Ok(self.send_authenticated(Request::get(path)).await?.body)
    }
}

/// The format to ask for: the codec's own, so the server passes the file through (module doc). Jellyfin's codec names for PGS and
/// VobSub map to the containers the server extracts them to; SubRip asks for `srt`; no codec asks for `srt` (that the server converts
/// to it is not verified).
pub(crate) fn subtitle_format(codec: Option<&Codec>) -> String {
    let codec = codec.map(|codec| codec.as_str().to_ascii_lowercase());
    match codec.as_deref() {
        None | Some("") => "srt".into(),
        Some("pgssub" | "pgs") => "sup".into(),
        Some("dvdsub" | "vobsub") => "mks".into(),
        Some("subrip") => "srt".into(),
        Some(other) => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_are_asked_for_in_their_own_format() {
        let format = |codec: &str| subtitle_format(Some(&Codec::new(codec)));
        assert_eq!(format("ass"), "ass");
        assert_eq!(format("SSA"), "ssa");
        assert_eq!(format("subrip"), "srt");
        assert_eq!(format("PGSSUB"), "sup");
        assert_eq!(format("dvdsub"), "mks");
        assert_eq!(format("webvtt"), "webvtt");
        assert_eq!(subtitle_format(None), "srt");
    }

    #[test]
    fn stream_access_debug_shows_no_value() {
        let access = StreamAccess {
            headers: vec![("Authorization".into(), "Basic header-secret".into())],
            cookies: vec![StreamCookie {
                name: "gateway".into(),
                value: "cookie-secret".into(),
                path: "/".into(),
                secure: true,
            }],
            secrets: vec!["token-secret".into()],
        };
        let shown = format!("{access:?} {access:#?}");
        for secret in ["header-secret", "cookie-secret", "token-secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
}
