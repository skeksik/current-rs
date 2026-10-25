//! The HLS playlist same-origin check (ADR 0019, "Playlists and headers"): before an HLS stream that carries gateway headers goes
//! to the engine, the core fetches its master playlist and every playlist it names, and checks that every URI in them is relative
//! or on the stream's own scheme, host and port. FFmpeg copies the `headers` option into every playlist, segment and key request,
//! to whatever host the playlist names (FFmpeg 9.0.2 `libavformat/aviobuf.c`, `ffio_copy_url_options`), so a playlist that points
//! elsewhere would hand the gateway's credentials to that host.
//!
//! A check, not enforcement: mpv fetches the playlists again itself, so a playlist that changes after the check could still name
//! another host. Jellyfin's VOD transcode playlists do not change; the loopback relay planned for M3 enforces the rule.
//!
//! The parser is line based, from RFC 8216 (read 2026-10-25):
//! - §4.1: "Each line is a URI, is blank, or starts with the character '#'. Blank lines are ignored." "Lines that start with the
//!   character '#' are either comments or tags. Tags begin with #EXT." "A URI in a Playlist, whether it is a URI line or part of a
//!   tag, MAY be relative. Any relative URI is considered to be relative to the URI of the Playlist that contains it."
//! - §4.2: "An attribute-list is a comma-separated list of attribute/value pairs with no whitespace." (`AttributeName=AttributeValue`)
//! - §4.3.4.2: "The URI line that follows the EXT-X-STREAM-INF tag specifies a Media Playlist"; §4.3.4.1 (EXT-X-MEDIA) and §4.3.4.3
//!   (EXT-X-I-FRAME-STREAM-INF) name Media Playlists in their `URI` attribute; §4.3.2.4 (EXT-X-KEY), §4.3.2.5 (EXT-X-MAP), §4.3.4.4
//!   (EXT-X-SESSION-DATA) and §4.3.4.5 (EXT-X-SESSION-KEY) name other resources in theirs.
//! - §6.3.1: clients MUST "ignore any unrecognized tags" and "ignore any attribute/value pair with an unrecognized AttributeName".
//!
//! Unknown tags and comments are therefore ignored, but not their URIs: every attribute named `URI` or ending in `-URI`, in any tag,
//! is checked, so a tag added after RFC 8216 (a partial segment, a preload hint, a rendition report, an interstitial's asset) cannot
//! slip a URI past the check. A URI with whitespace, a control character or a backslash is refused rather than guessed at, since
//! FFmpeg and this parser might resolve it differently.

use current_net::{NetError, Request, Url};

use crate::client::{AuthError, JellyfinClient};

/// The largest playlist the check reads. A three-hour VOD media playlist with 6-second segments is about 1,800 segment lines.
pub const MAX_PLAYLIST_BYTES: usize = 4 * 1024 * 1024;

/// The most media playlists a master playlist may name: each is fetched and checked.
pub const MAX_MEDIA_PLAYLISTS: usize = 32;

/// Why the check refused a stream. No variant holds a URL or the playlist's text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlaylistCheckError {
    /// A URI in a playlist is on another scheme, host or port than the stream.
    #[error("the server's playlist points to another address")]
    Elsewhere,
    /// A playlist is larger than [`MAX_PLAYLIST_BYTES`], or the master playlist names more than [`MAX_MEDIA_PLAYLISTS`].
    #[error("the server's playlist is too large")]
    TooLarge,
    /// A playlist is not an HLS playlist, or a URI in it is not one the check can judge.
    #[error("the server's playlist could not be read")]
    Malformed,
    /// The stream URL is not on the address the client uses now: the server moved to another address after the URL was built. Ask
    /// for a new URL.
    #[error("the server's address changed")]
    AddressChanged,
    #[error(transparent)]
    Auth(#[from] AuthError),
}

impl JellyfinClient {
    /// Checks the HLS stream at `url` (a stream URL this client built, with its `ApiKey`) before an engine opens it with gateway
    /// headers. Fetches the master playlist and every media playlist it names through this client, with the address's credentials
    /// and the token in its header (the `ApiKey` is taken out of the query, so it never reaches the request log), and checks every
    /// URI in them. `Ok` means every URI was relative or on the stream's own scheme, host and port.
    pub async fn check_playlist_origin(&self, url: &str) -> Result<(), PlaylistCheckError> {
        let stream = Url::parse(url).map_err(|_| PlaylistCheckError::Malformed)?;
        let base = self.stream_base()?;
        if !is_under(&base, &stream) {
            return Err(PlaylistCheckError::AddressChanged);
        }
        let (location, text) = self.fetch_playlist(&base, &stream).await?;
        let mut playlists = Vec::new();
        for reference in references(&text)? {
            let resolved = check_uri(&location, &stream, reference.uri)?;
            if reference.playlist && !playlists.contains(&resolved) {
                playlists.push(resolved);
            }
        }
        if playlists.len() > MAX_MEDIA_PLAYLISTS {
            return Err(PlaylistCheckError::TooLarge);
        }
        for playlist in &playlists {
            if !is_under(&base, playlist) {
                return Err(PlaylistCheckError::AddressChanged);
            }
            let (location, text) = self.fetch_playlist(&base, playlist).await?;
            // FFmpeg reads two levels (master and media); a playlist named in a media playlist is checked, not fetched.
            for reference in references(&text)? {
                check_uri(&location, &stream, reference.uri)?;
            }
        }
        Ok(())
    }

    /// The playlist at `url`, which is under `base`, and where it came from (after redirects on the same host).
    async fn fetch_playlist(
        &self,
        base: &Url,
        url: &Url,
    ) -> Result<(Url, String), PlaylistCheckError> {
        let path = &url.path()[base.path().trim_end_matches('/').len()..];
        let mut request = Request::get(path).max_body(MAX_PLAYLIST_BYTES);
        for (key, value) in url.query_pairs() {
            if !key.eq_ignore_ascii_case("ApiKey") && !key.eq_ignore_ascii_case("api_key") {
                request = request.query(key, value);
            }
        }
        let response = match self.send_authenticated(request).await {
            Ok(response) => response,
            Err(AuthError::Network(NetError::TooLarge)) => {
                return Err(PlaylistCheckError::TooLarge);
            }
            Err(error) => return Err(error.into()),
        };
        let location = response.url.url().clone();
        // A redirect to another host is not followed (current-net); another origin here means the client moved to another address.
        if !same_origin(&location, url) {
            return Err(PlaylistCheckError::AddressChanged);
        }
        let text = String::from_utf8(response.body).map_err(|_| PlaylistCheckError::Malformed)?;
        Ok((location, text))
    }
}

/// A URI a playlist names, and whether it names a media playlist (to fetch and check in turn).
#[derive(Debug, PartialEq, Eq)]
struct Reference<'a> {
    uri: &'a str,
    playlist: bool,
}

/// Every URI in a playlist: URI lines and URI attributes of any tag. Refuses text that is not an HLS playlist (no `#EXTM3U` first,
/// a byte order mark, control characters: RFC 8216 §4.1, §4.3.1.1).
fn references(text: &str) -> Result<Vec<Reference<'_>>, PlaylistCheckError> {
    let malformed = PlaylistCheckError::Malformed;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
        || text.starts_with('\u{feff}')
    {
        return Err(malformed);
    }
    let mut lines = text.split('\n').map(str::trim);
    if lines.next() != Some("#EXTM3U") {
        return Err(malformed);
    }
    let mut found = Vec::new();
    let mut variant_next = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some(tag) = line.strip_prefix('#') else {
            found.push(Reference {
                uri: line,
                playlist: variant_next,
            });
            variant_next = false;
            continue;
        };
        if !tag.starts_with("EXT") {
            continue;
        }
        let (name, value) = tag.split_once(':').unwrap_or((tag, ""));
        if name == "EXT-X-STREAM-INF" {
            variant_next = true;
        }
        let playlist = matches!(name, "EXT-X-MEDIA" | "EXT-X-I-FRAME-STREAM-INF");
        for (attribute, value) in attributes(value) {
            let attribute = attribute.to_ascii_uppercase();
            if attribute != "URI" && !attribute.ends_with("-URI") {
                continue;
            }
            let uri = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .filter(|uri| !uri.contains('"'))
                .ok_or(PlaylistCheckError::Malformed)?;
            found.push(Reference { uri, playlist });
        }
    }
    Ok(found)
}

/// A tag's value split into `name=value` pairs at commas outside quotes. Values that are not attribute lists (EXTINF's duration and
/// title) yield pairs only where a piece happens to look like one, which the caller then checks: stricter, never looser.
fn attributes(value: &str) -> Vec<(&str, &str)> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (at, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                pieces.push(&value[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    pieces.push(&value[start..]);
    pieces
        .into_iter()
        .filter_map(|piece| piece.trim().split_once('='))
        .filter(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .collect()
}

/// `uri` resolved against the playlist it is in (`location`), if it is on `stream`'s origin.
fn check_uri(location: &Url, stream: &Url, uri: &str) -> Result<Url, PlaylistCheckError> {
    if uri.is_empty()
        || uri
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return Err(PlaylistCheckError::Malformed);
    }
    let resolved = if has_scheme(uri) {
        // `http:path` without `//` is relative to WHATWG URL parsers and absolute to others: neither reading is trusted.
        if !uri
            .split_once(':')
            .is_some_and(|(_, rest)| rest.starts_with("//"))
        {
            return Err(PlaylistCheckError::Elsewhere);
        }
        Url::parse(uri)
    } else {
        location.join(uri)
    }
    .map_err(|_| PlaylistCheckError::Malformed)?;
    if same_origin(&resolved, stream) {
        Ok(resolved)
    } else {
        Err(PlaylistCheckError::Elsewhere)
    }
}

/// Whether `uri` starts with a scheme (RFC 3986 §3.1: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`).
fn has_scheme(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !scheme.contains('/')
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Whether `url` is on `base`'s origin and under its path: a URL the client can fetch on the address it uses now.
fn is_under(base: &Url, url: &Url) -> bool {
    let prefix = base.path().trim_end_matches('/');
    same_origin(base, url)
        && url
            .path()
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn uris(text: &str) -> Vec<(&str, bool)> {
        references(text)
            .unwrap()
            .into_iter()
            .map(|r| (r.uri, r.playlist))
            .collect()
    }

    #[test]
    fn a_master_playlist_names_its_variants_media_and_keys() {
        let text = "#EXTM3U\r\n\
            # a comment\r\n\
            #EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"session.key\"\r\n\
            #EXT-X-SESSION-DATA:DATA-ID=\"x\",URI=\"data.json\"\r\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A, b\",URI=\"audio.m3u8\"\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,CODECS=\"avc1.640028,mp4a.40.2\",AUDIO=\"a\"\r\n\
            \r\n\
            main.m3u8?DeviceId=d\r\n\
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100,URI=\"iframe.m3u8\"\r\n\
            #EXT-X-SOMETHING-NEW:FOO=1,ASSET-URI=\"asset.json\"\r\n";
        assert_eq!(
            uris(text),
            [
                ("session.key", false),
                ("data.json", false),
                ("audio.m3u8", true),
                ("main.m3u8?DeviceId=d", true),
                ("iframe.m3u8", true),
                ("asset.json", false),
            ]
        );
    }

    #[test]
    fn a_media_playlist_names_its_segments_keys_and_maps() {
        let text = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:6\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"k.key\",IV=0x1\n\
            #EXT-X-MAP:URI=\"init.mp4\"\n\
            #EXTINF:6.0,a title, with a comma\n0.mp4\n\
            #EXT-X-PART:DURATION=1,URI=\"p.mp4\"\n\
            #EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"h.mp4\"\n\
            #EXTINF:6.0,\n/abs/1.mp4\n#EXT-X-ENDLIST\n";
        assert_eq!(
            uris(text),
            [
                ("k.key", false),
                ("init.mp4", false),
                ("0.mp4", false),
                ("p.mp4", false),
                ("h.mp4", false),
                ("/abs/1.mp4", false),
            ]
        );
    }

    #[test]
    fn text_that_is_not_a_playlist_or_an_unquoted_uri_is_malformed() {
        for text in [
            "",
            "<html></html>",
            "\u{feff}#EXTM3U\n0.ts\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=http://192.168.1.10/k\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"a\"b\"\n",
            "#EXTM3U\n0.ts\u{0}\n",
        ] {
            assert_eq!(
                references(text),
                Err(PlaylistCheckError::Malformed),
                "{text:?}"
            );
        }
    }

    #[test]
    fn only_relative_or_same_origin_uris_pass() {
        let stream = url("https://media.example.com/jf/videos/1/master.m3u8?ApiKey=x");
        let location = url("https://media.example.com/jf/videos/1/master.m3u8");
        for uri in [
            "main.m3u8",
            "../2/main.m3u8",
            "/jf/videos/1/0.mp4",
            "?x=1",
            "https://media.example.com/jf/videos/1/0.mp4",
            "https://media.example.com:443/x",
            "//media.example.com/x",
        ] {
            assert!(check_uri(&location, &stream, uri).is_ok(), "{uri}");
        }
        for uri in [
            "https://192.168.1.10/x",
            "https://media.example.com:8443/x",
            "http://media.example.com/x",
            "//192.168.1.10/x",
            "data:text/plain,x",
            "https:media.example.com/x",
            "skd://key",
        ] {
            assert_eq!(
                check_uri(&location, &stream, uri),
                Err(PlaylistCheckError::Elsewhere),
                "{uri}"
            );
        }
        for uri in ["", "a b", "\\\\192.168.1.10\\x", "a\tb"] {
            assert_eq!(
                check_uri(&location, &stream, uri),
                Err(PlaylistCheckError::Malformed),
                "{uri:?}"
            );
        }
    }

    #[test]
    fn a_url_is_under_the_address_only_below_its_path() {
        let base = url("https://media.example.com/jf/");
        assert!(is_under(
            &base,
            &url("https://media.example.com/jf/videos/1")
        ));
        assert!(!is_under(
            &base,
            &url("https://media.example.com/jfx/videos/1")
        ));
        assert!(!is_under(&base, &url("https://media.example.com/videos/1")));
        assert!(!is_under(&base, &url("https://192.168.1.10/jf/videos/1")));
        let root = url("http://127.0.0.1:8096/");
        assert!(is_under(&root, &url("http://127.0.0.1:8096/videos/1")));
        assert!(!is_under(&root, &url("http://127.0.0.1:8097/videos/1")));
    }
}
