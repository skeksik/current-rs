//! HLS playlist rewriting (ADR 0020, "HLS playlists"): every URI in a playlist the relay passes on is resolved against the playlist's
//! upstream URL and either becomes an absolute path on the relay (`/<route>/...`, without `ApiKey`/`api_key`) or fails the playlist.
//! The player therefore never learns another address, and every fetch is rewritten, so a playlist that changes is enforced each time.
//!
//! The parser is the one of `current-jellyfin`'s playlist check (`crates/jellyfin/src/hls.rs`), moved here and turned from finding
//! URIs into replacing them. It is line based, from RFC 8216 (read 2026-10-25, as quoted in `hls.rs`):
//! - §4.1: "Each line is a URI, is blank, or starts with the character '#'. Blank lines are ignored." "Lines that start with the
//!   character '#' are either comments or tags. Tags begin with #EXT." "A URI in a Playlist, whether it is a URI line or part of a
//!   tag, MAY be relative. Any relative URI is considered to be relative to the URI of the Playlist that contains it."
//! - §4.2: "An attribute-list is a comma-separated list of attribute/value pairs with no whitespace." (`AttributeName=AttributeValue`)
//! - §6.3.1: clients MUST "ignore any unrecognized tags" and "ignore any attribute/value pair with an unrecognized AttributeName".
//!
//! Unknown tags and comments are passed on, but not their URIs: every attribute named `URI` or ending in `-URI`, in any tag, is
//! rewritten, so a tag added after RFC 8216 (a partial segment, a preload hint, a rendition report, an interstitial's asset) cannot
//! slip a URI past the relay. Byte ranges (`EXT-X-BYTERANGE`, `EXT-X-MAP`'s `BYTERANGE`) carry no URI and pass unchanged: the player
//! turns them into `Range` requests. A URI with whitespace, a control character or a backslash, and a carriage return that does not
//! end a line, fail the playlist rather than be guessed at, since the player and this parser might read them differently.

use current_net::Url;
use current_net::header::{CONTENT_TYPE, HeaderMap};

use crate::path::Prefixes;

/// The largest playlist the relay reads (ADR 0020, today's limit of the check). A three-hour VOD media playlist with 6-second
/// segments is about 1,800 segment lines.
pub const MAX_PLAYLIST_BYTES: usize = 4 * 1024 * 1024;

/// The content types of an HLS playlist (RFC 8216 §4 names the first two; Jellyfin and others also send `audio/mpegurl`).
const PLAYLIST_TYPES: [&str; 3] = [
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
];

/// Why a playlist was not passed on. No variant holds a URI or the playlist's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaylistError {
    /// A URI is off the server's address, or outside the route's prefixes.
    Elsewhere,
    /// Not an HLS playlist, or a URI the relay cannot judge.
    Malformed,
}

/// Whether an answer is a playlist: an HLS content type, or a request path ending in `.m3u8`.
pub(crate) fn is_playlist(headers: &HeaderMap, path: &str) -> bool {
    let typed = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let essence = value.split(';').next().unwrap_or("").trim();
            PLAYLIST_TYPES
                .iter()
                .any(|known| essence.eq_ignore_ascii_case(known))
        });
    typed
        || path
            .to_ascii_lowercase()
            .trim_end_matches('/')
            .ends_with(".m3u8")
}

/// Where a playlist came from and where its URIs may go.
pub(crate) struct Rewriter<'a> {
    /// The playlist's upstream URL: the address's base plus the request's path.
    pub(crate) playlist: &'a Url,
    /// The address's base URL.
    pub(crate) base: &'a Url,
    pub(crate) prefixes: &'a Prefixes,
    /// The route's token, the first segment of every rewritten path.
    pub(crate) token: &'a str,
}

impl Rewriter<'_> {
    /// `text` with every URI rewritten. Refuses text that is not an HLS playlist (no `#EXTM3U` first, a byte order mark, control
    /// characters: RFC 8216 §4.1, §4.3.1.1).
    pub(crate) fn rewrite(&self, text: &str) -> Result<String, PlaylistError> {
        let malformed = PlaylistError::Malformed;
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
            || text.starts_with('\u{feff}')
        {
            return Err(malformed);
        }
        let mut out = String::with_capacity(text.len() + text.len() / 4);
        for (index, raw) in text.split('\n').enumerate() {
            let (line, end) = match raw.strip_suffix('\r') {
                Some(line) => (line, "\r"),
                None => (raw, ""),
            };
            // Some players end a line at a lone CR, this parser would not.
            if line.contains('\r') {
                return Err(malformed);
            }
            if index > 0 {
                out.push('\n');
            }
            let trimmed = line.trim();
            if index == 0 {
                if trimmed != "#EXTM3U" {
                    return Err(malformed);
                }
                out.push_str(line);
            } else if let Some(tag) = trimmed.strip_prefix('#') {
                if tag.starts_with("EXT") {
                    out.push('#');
                    out.push_str(&self.tag(tag)?);
                } else {
                    out.push_str(line);
                }
            } else if trimmed.is_empty() {
                out.push_str(line);
            } else {
                out.push_str(&self.uri(trimmed)?);
            }
            out.push_str(end);
        }
        Ok(out)
    }

    /// A tag (without its `#`) with every `URI` and `*-URI` attribute rewritten. Values that are not attribute lists (EXTINF's
    /// duration and title) are split too and rewritten only where a piece happens to look like a URI attribute, which then has to
    /// pass like any other: stricter, never looser.
    fn tag(&self, tag: &str) -> Result<String, PlaylistError> {
        let Some((name, value)) = tag.split_once(':') else {
            return Ok(tag.to_owned());
        };
        let mut pieces = Vec::new();
        for piece in split_attributes(value) {
            let attribute = piece.trim().split_once('=').filter(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
            let Some((attribute, quoted)) = attribute else {
                pieces.push(piece.to_owned());
                continue;
            };
            let upper = attribute.to_ascii_uppercase();
            if upper != "URI" && !upper.ends_with("-URI") {
                pieces.push(piece.to_owned());
                continue;
            }
            let uri = quoted
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .filter(|uri| !uri.contains('"'))
                .ok_or(PlaylistError::Malformed)?;
            pieces.push(format!("{attribute}=\"{}\"", self.uri(uri)?));
        }
        Ok(format!("{name}:{}", pieces.join(",")))
    }

    /// `uri` resolved against the playlist and turned into a path on the relay, if it is on the server's address and inside the
    /// route's prefixes.
    fn uri(&self, uri: &str) -> Result<String, PlaylistError> {
        if uri.is_empty()
            || uri
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return Err(PlaylistError::Malformed);
        }
        let resolved = if has_scheme(uri) {
            // `http:path` without `//` is relative to WHATWG URL parsers and absolute to others: neither reading is trusted.
            if !uri
                .split_once(':')
                .is_some_and(|(_, rest)| rest.starts_with("//"))
            {
                return Err(PlaylistError::Elsewhere);
            }
            Url::parse(uri)
        } else {
            self.playlist.join(uri)
        }
        .map_err(|_| PlaylistError::Malformed)?;
        if !same_origin(&resolved, self.base) {
            return Err(PlaylistError::Elsewhere);
        }
        let prefix = self.base.path().trim_end_matches('/');
        let path = resolved
            .path()
            .strip_prefix(prefix)
            .filter(|rest| rest.starts_with('/'))
            .ok_or(PlaylistError::Elsewhere)?;
        if !self.prefixes.allow(path) {
            return Err(PlaylistError::Elsewhere);
        }
        let mut out = format!("/{}{path}", self.token);
        let query = without_api_key(resolved.query().unwrap_or(""));
        if !query.is_empty() {
            out.push('?');
            out.push_str(&query);
        }
        Ok(out)
    }
}

/// A raw query without its `ApiKey` and `api_key` pairs (in any case), the others as they were written.
pub(crate) fn without_api_key(query: &str) -> String {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            url::form_urlencoded::parse(pair.as_bytes())
                .next()
                .is_none_or(|(key, _)| !is_api_key(&key))
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Whether a query key carries the server's token (Jellyfin's `ApiKey`, and its older `api_key`).
pub(crate) fn is_api_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("ApiKey") || key.eq_ignore_ascii_case("api_key")
}

/// A tag's value split at commas outside quotes.
fn split_attributes(value: &str) -> Vec<&str> {
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
}

/// Whether `uri` starts with a scheme (RFC 3986 §3.1: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`).
fn has_scheme(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use current_net::header::HeaderValue;

    use super::*;

    const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn rewrite_at(base: &str, path: &str, text: &str) -> Result<String, PlaylistError> {
        let base = url(base);
        let playlist = url(&format!("{}{path}", base.as_str().trim_end_matches('/')));
        let prefixes = Prefixes::new(vec!["/Videos/abc/".into(), "/Audio/abc/".into()]).unwrap();
        Rewriter {
            playlist: &playlist,
            base: &base,
            prefixes: &prefixes,
            token: TOKEN,
        }
        .rewrite(text)
    }

    fn rewrite(text: &str) -> Result<String, PlaylistError> {
        rewrite_at(
            "https://media.example.com/jf/",
            "/videos/abc/master.m3u8",
            text,
        )
    }

    #[test]
    fn a_master_playlist_has_its_variants_media_keys_and_unknown_tags_rewritten() {
        let text = "#EXTM3U\r\n\
            # a comment, URI=\"https://192.168.1.10/x\"\r\n\
            #EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"session.key?ApiKey=secret\"\r\n\
            #EXT-X-SESSION-DATA:DATA-ID=\"x\",URI=\"data.json\"\r\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A, b\",URI=\"audio.m3u8\"\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,CODECS=\"avc1.640028,mp4a.40.2\",AUDIO=\"a\"\r\n\
            \r\n\
            main.m3u8?DeviceId=d&ApiKey=secret&api_key=secret&APIKEY=secret&PlaySessionId=p\r\n\
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100,uri=\"/jf/Videos/abc/iframe.m3u8\"\r\n\
            #EXT-X-SOMETHING-NEW:FOO=1,ASSET-URI=\"https://media.example.com:443/jf/videos/abc/asset.json\"\r\n";
        let expected = format!(
            "#EXTM3U\r\n\
            # a comment, URI=\"https://192.168.1.10/x\"\r\n\
            #EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"/{TOKEN}/videos/abc/session.key\"\r\n\
            #EXT-X-SESSION-DATA:DATA-ID=\"x\",URI=\"/{TOKEN}/videos/abc/data.json\"\r\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"A, b\",URI=\"/{TOKEN}/videos/abc/audio.m3u8\"\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,CODECS=\"avc1.640028,mp4a.40.2\",AUDIO=\"a\"\r\n\
            \r\n\
            /{TOKEN}/videos/abc/main.m3u8?DeviceId=d&PlaySessionId=p\r\n\
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100,uri=\"/{TOKEN}/Videos/abc/iframe.m3u8\"\r\n\
            #EXT-X-SOMETHING-NEW:FOO=1,ASSET-URI=\"/{TOKEN}/videos/abc/asset.json\"\r\n"
        );
        assert_eq!(rewrite(text).unwrap(), expected);
    }

    #[test]
    fn a_media_playlist_has_its_segments_keys_and_maps_rewritten_and_byte_ranges_kept() {
        let text = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:6\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"k.key\",IV=0x1\n\
            #EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n\
            #EXTINF:6.0,a title, with a comma\n#EXT-X-BYTERANGE:1000@720\n0.mp4?ApiKey=secret\n\
            #EXT-X-PART:DURATION=1,URI=\"p.mp4\"\n\
            #EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"h.mp4\"\n\
            #EXTINF:6.0,\nhttps://media.example.com/jf/Videos/abc/hls1/1.mp4\n#EXT-X-ENDLIST\n";
        let expected = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:6\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"/{TOKEN}/videos/abc/k.key\",IV=0x1\n\
            #EXT-X-MAP:URI=\"/{TOKEN}/videos/abc/init.mp4\",BYTERANGE=\"720@0\"\n\
            #EXTINF:6.0,a title, with a comma\n#EXT-X-BYTERANGE:1000@720\n/{TOKEN}/videos/abc/0.mp4\n\
            #EXT-X-PART:DURATION=1,URI=\"/{TOKEN}/videos/abc/p.mp4\"\n\
            #EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"/{TOKEN}/videos/abc/h.mp4\"\n\
            #EXTINF:6.0,\n/{TOKEN}/Videos/abc/hls1/1.mp4\n#EXT-X-ENDLIST\n"
        );
        let rewritten = rewrite(text).unwrap();
        assert_eq!(rewritten, expected);
        assert!(!rewritten.to_ascii_lowercase().contains("apikey"));
    }

    #[test]
    fn a_uri_off_the_address_or_outside_the_prefixes_fails_the_playlist() {
        for uri in [
            "https://192.168.1.10/jf/videos/abc/0.mp4",
            "https://media.example.com:8443/jf/videos/abc/0.mp4",
            "http://media.example.com/jf/videos/abc/0.mp4",
            "//192.168.1.10/jf/videos/abc/0.mp4",
            "data:text/plain,x",
            "https:media.example.com/x",
            "skd://key",
            "/jf/Users/abc",
            "/other/videos/abc/0.mp4",
            "/jfx/videos/abc/0.mp4",
            "../../Users/x",
            "/jf/videos/abd/0.mp4",
            "/jf/videos/abc/%2e%2e/%2e%2e/Users",
        ] {
            for text in [
                format!("#EXTM3U\n#EXTINF:6,\n{uri}\n"),
                format!("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"{uri}\"\n"),
                format!("#EXTM3U\n#EXT-X-MAP:URI=\"{uri}\",BYTERANGE=\"1@0\"\n"),
                format!("#EXTM3U\n#EXT-X-FUTURE:X=1,REPORT-URI=\"{uri}\"\n"),
            ] {
                assert_eq!(rewrite(&text), Err(PlaylistError::Elsewhere), "{text}");
            }
        }
    }

    #[test]
    fn text_that_is_not_a_playlist_or_an_unreadable_uri_is_malformed() {
        for text in [
            "",
            "<html></html>",
            "\u{feff}#EXTM3U\n0.ts\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=http://192.168.1.10/k\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"a\"b\"\n",
            "#EXTM3U\n0.ts\u{0}\n",
            "#EXTM3U\n#EXTINF:6,\rhttps://192.168.1.10/x\n",
            "#EXTM3U\na b\n",
            "#EXTM3U\n\\\\192.168.1.10\\x\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"\"\n",
        ] {
            assert_eq!(rewrite(text), Err(PlaylistError::Malformed), "{text:?}");
        }
    }

    #[test]
    fn an_address_at_the_root_resolves_like_one_with_a_path() {
        let text = "#EXTM3U\n../abc/0.ts?api_key=x&b=1\n";
        assert_eq!(
            rewrite_at("http://127.0.0.1:8096/", "/Videos/abc/main.m3u8", text).unwrap(),
            format!("#EXTM3U\n/{TOKEN}/Videos/abc/0.ts?b=1\n")
        );
        assert_eq!(
            rewrite_at(
                "http://127.0.0.1:8096/",
                "/Videos/abc/main.m3u8",
                "#EXTM3U\nhttp://127.0.0.1:8097/Videos/abc/0.ts\n"
            ),
            Err(PlaylistError::Elsewhere)
        );
    }

    #[test]
    fn playlists_are_known_by_type_or_path() {
        let mut headers = HeaderMap::new();
        assert!(is_playlist(&headers, "/Videos/abc/master.M3U8"));
        assert!(!is_playlist(&headers, "/Videos/abc/stream.mp4"));
        for value in [
            "application/vnd.apple.mpegurl",
            "application/x-mpegURL; charset=utf-8",
            "audio/mpegurl",
        ] {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(value));
            assert!(is_playlist(&headers, "/Videos/abc/x"), "{value}");
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
        assert!(!is_playlist(&headers, "/Videos/abc/0.ts"));
    }

    #[test]
    fn only_api_key_pairs_leave_a_query() {
        assert_eq!(
            without_api_key("a=1&ApiKey=x&b=%20&api%5Fkey=y&&apikey"),
            "a=1&b=%20"
        );
        assert_eq!(without_api_key(""), "");
    }
}
