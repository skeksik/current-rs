//! What a player needs from Jellyfin besides PlaybackInfo and the reports (ADR 0019): the secrets a stream URL holds, the stream's
//! path for the loopback relay and its streaming send (ADR 0020), the user's transcoding policy (read only to explain an outcome),
//! and external subtitle files, which the core fetches itself so the engine never does.
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
use current_net::{HostScope, Request, StreamError, StreamResponse, Url, host_scope};
use serde::Deserialize;

use crate::auth;
use crate::client::{AuthError, JellyfinClient};

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
    /// The secrets a stream URL this client made holds, for the engine to redact from its log: the session token, which the URL
    /// carries as `ApiKey`.
    pub fn stream_secrets(&self) -> Vec<String> {
        auth::read(&self.token)
            .map(|token| vec![token.expose().to_owned()])
            .unwrap_or_default()
    }

    /// Whether requests to the address in use carry gateway credentials (ADR 0017): its streams then go through the loopback relay
    /// (ADR 0020). False while no address is selected.
    pub fn address_has_gateway(&self) -> bool {
        self.http
            .current_address()
            .is_some_and(|selected| selected.has_gateway())
    }

    /// `url` (a stream URL this client made, exposed)'s path and query relative to the address in use (`/Videos/{id}/stream?static=true&...`), without `ApiKey` and `api_key`:
    /// what the loopback relay's URL carries after its route (ADR 0020 choice 4), percent-encoded as in `url`. `None` when `url` is
    /// not on the address in use (the server moved to another address since the URL was made) or is not a URL.
    pub fn stream_path(&self, url: &str) -> Option<String> {
        let url = Url::parse(url).ok()?;
        let selected = self.http.current_address()?;
        let base = selected.url();
        if url.origin() != base.origin() {
            return None;
        }
        let prefix = base.path().trim_end_matches('/');
        let path = url.path().strip_prefix(prefix)?;
        if !path.starts_with('/') {
            return None;
        }
        let query: Vec<&str> = url
            .query()
            .unwrap_or_default()
            .split('&')
            .filter(|pair| !pair.is_empty() && !is_token_pair(pair))
            .collect();
        Some(if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", query.join("&"))
        })
    }

    /// Sends a stream request through this session (`current-net`'s `send_stream`, for the loopback relay, ADR 0020): to the address
    /// selected now, with that address's gateway credentials and the session token in Jellyfin's header, never `ApiKey`. A refusal
    /// starts no check here: the caller takes it to the 401 rule.
    pub async fn send_stream(&self, request: Request) -> Result<StreamResponse, StreamError> {
        self.http.send_stream(request).await
    }

    /// Whether the address in use is on this device or the local network, for the quality remembered per kind of address (ADR 0019,
    /// choice 10). False while no address is selected.
    pub fn address_is_local(&self) -> bool {
        self.http.current_address().is_some_and(|selected| {
            matches!(
                host_scope(selected.url()),
                HostScope::Local | HostScope::Loopback
            )
        })
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
        // The source id and the codec come from the server: escaped, so neither can change the path.
        let path = format!(
            "/Videos/{}/{}/Subtitles/{index}/Stream.{}",
            path_segment(item.as_str()),
            path_segment(media_source),
            path_segment(&subtitle_format(codec)),
        );
        Ok(self.send_authenticated(Request::get(path)).await?.body)
    }
}

/// Whether a raw query pair (`name=value`) is the session token: `ApiKey` or `api_key`, in any case and however encoded.
fn is_token_pair(pair: &str) -> bool {
    let name = pair.split('=').next().unwrap_or_default();
    url::form_urlencoded::parse(name.as_bytes())
        .next()
        .is_some_and(|(name, _)| {
            name.eq_ignore_ascii_case("apikey") || name.eq_ignore_ascii_case("api_key")
        })
}

/// `text` as one path segment: unreserved characters (RFC 3986 section 2.3) as they are, every other byte percent-encoded.
fn path_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
    fn server_values_cannot_change_the_subtitle_path() {
        assert_eq!(path_segment("2f0000b2"), "2f0000b2");
        assert_eq!(path_segment("../x?y=1#z"), "..%2Fx%3Fy%3D1%23z");
        assert_eq!(path_segment("sub rip"), "sub%20rip");
    }

    #[test]
    fn the_token_is_found_in_any_case_and_encoding() {
        for pair in [
            "ApiKey=t",
            "apikey=t",
            "API_KEY=t",
            "api%5Fkey=t",
            "Api%4Bey=t",
            "ApiKey",
        ] {
            assert!(is_token_pair(pair), "{pair}");
        }
        for pair in ["static=true", "ApiKeys=t", "x=ApiKey", "PlaySessionId=1"] {
            assert!(!is_token_pair(pair), "{pair}");
        }
    }
}
