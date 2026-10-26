//! HLS playlist rewriting (ADR 0020, "HLS playlists"): every URI in a playlist the relay passes on is resolved against the playlist's
//! upstream URL and either becomes an absolute path on the relay (`/<route>/...`, without `ApiKey`/`api_key`) or fails the playlist.
//! The player therefore never learns another address, and every fetch is rewritten, so a playlist that changes is enforced each time.
//!
//! The parser is the one of `current-jellyfin`'s playlist check (`crates/jellyfin/src/hls.rs`, removed with the check, ADR 0020
//! choice 8), moved here and turned from finding URIs into replacing them. It is line based, from RFC 8216 (read 2026-10-25, as
//! quoted in that file):
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

/// The largest playlist the relay writes. Rewriting makes a URI longer (the token, the prefix's path), so a playlist of very short
/// URI lines could grow many times over; one past this limit fails like one too large to read.
pub const MAX_REWRITTEN_BYTES: usize = 16 * 1024 * 1024;

/// The content types of an HLS playlist (RFC 8216 §4 names the first two; Jellyfin and others also send `audio/mpegurl`, and FFmpeg
/// 6.1.1 also took `audio/x-mpegurl` as HLS, in the manager's review of this crate on 2026-10-26). A body is also sniffed
/// ([`starts_like_playlist`]), whatever its type.
const PLAYLIST_TYPES: [&str; 4] = [
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
];

/// Why a playlist was not passed on. No variant holds a URI or the playlist's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaylistError {
    /// A URI is off the server's address, or outside the route's prefixes.
    Elsewhere,
    /// Not an HLS playlist, or a URI the relay cannot judge.
    Malformed,
    /// Larger than [`MAX_REWRITTEN_BYTES`] once rewritten.
    TooLarge,
}

/// Whether an answer is declared a playlist: an HLS content type, or a request path ending in `.m3u8` or `.m3u`.
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
    let path = path.to_ascii_lowercase();
    let path = path.trim_end_matches('/');
    typed || path.ends_with(".m3u8") || path.ends_with(".m3u")
}

/// How much of a body's start [`starts_like_playlist`] may need: a byte order mark and whitespace before `#EXTM3U` are skipped.
pub(crate) const SNIFF_LIMIT: usize = 4096;

/// Whether a body's first bytes say playlist: `#EXTM3U` (in any case) after an optional byte order mark and whitespace, which a
/// player may accept whatever the content type. `None` while `head` is too short to tell (and shorter than [`SNIFF_LIMIT`]).
pub(crate) fn starts_like_playlist(head: &[u8], complete: bool) -> Option<bool> {
    const MAGIC: &[u8] = b"#EXTM3U";
    let rest = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let rest = rest.trim_ascii_start();
    if rest.len() >= MAGIC.len() {
        return Some(rest[..MAGIC.len()].eq_ignore_ascii_case(MAGIC));
    }
    if !MAGIC[..rest.len()].eq_ignore_ascii_case(rest) {
        return Some(false);
    }
    // So far only whitespace and the start of `#EXTM3U`. A body that ends here is a playlist if it began the magic (and then fails
    // to rewrite); one that is still only whitespace after the limit is refused as a playlist rather than passed on unread.
    if complete {
        Some(!rest.is_empty())
    } else if head.len() >= SNIFF_LIMIT {
        Some(true)
    } else {
        None
    }
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
            // FFmpeg ends a line at a lone CR; this parser would not.
            if line.contains('\r') {
                return Err(malformed);
            }
            if index > 0 {
                append(&mut out, "\n")?;
            }
            let trimmed = line.trim();
            if index == 0 {
                if trimmed != "#EXTM3U" {
                    return Err(malformed);
                }
                append(&mut out, line)?;
            } else if let Some(tag) = trimmed.strip_prefix('#') {
                if tag.starts_with("EXT") {
                    append(&mut out, "#")?;
                    self.tag(tag, &mut out)?;
                } else {
                    append(&mut out, line)?;
                }
            } else if trimmed.is_empty() {
                append(&mut out, line)?;
            } else {
                append(&mut out, &self.uri(trimmed)?)?;
            }
            append(&mut out, end)?;
        }
        Ok(out)
    }

    /// A tag (without its `#`) with every `URI` and `*-URI` attribute rewritten, appended to `out`. A value that is a strict
    /// attribute list (RFC 8216 §4.2) is written out again attribute by attribute, so the player reads exactly what was checked. A
    /// value that is not one (EXTINF's duration and title, a byte range, a date) passes unchanged only if nothing in it could be read
    /// as an attribute or a quote by a laxer parser: FFmpeg's (`ff_parse_key_value`) ends an unquoted value at whitespace, opens a
    /// quote only at a value's start and honours `\` escapes inside quotes, so `IV=0x1 URI="..."` or `X=a"b,URI="..."` would hide a URI
    /// from a loose split.
    ///
    /// The output's limit is checked before every attribute: a single line of many short URIs under a long path must not grow far
    /// past it before it is caught.
    fn tag(&self, tag: &str, out: &mut String) -> Result<(), PlaylistError> {
        let Some((name, value)) = tag.split_once(':') else {
            return append(out, tag);
        };
        let Some(attributes) = attribute_list(value) else {
            let upper = value.to_ascii_uppercase();
            if value.contains(['"', '\\']) || upper.contains("URI=") {
                return Err(PlaylistError::Malformed);
            }
            return append(out, tag);
        };
        append(out, name)?;
        append(out, ":")?;
        for (at, (attribute, value)) in attributes.into_iter().enumerate() {
            if at > 0 {
                append(out, ",")?;
            }
            append(out, attribute)?;
            append(out, "=")?;
            if attribute != "URI" && !attribute.ends_with("-URI") {
                append(out, value)?;
                continue;
            }
            let uri = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .ok_or(PlaylistError::Malformed)?;
            append(out, "\"")?;
            append(out, &self.uri(uri)?)?;
            append(out, "\"")?;
        }
        Ok(())
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

/// Appends `text` to a rewritten playlist, unless that would take it past [`MAX_REWRITTEN_BYTES`].
fn append(out: &mut String, text: &str) -> Result<(), PlaylistError> {
    if out.len() + text.len() > MAX_REWRITTEN_BYTES {
        return Err(PlaylistError::TooLarge);
    }
    out.push_str(text);
    Ok(())
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

/// `value` as an attribute list, strictly by RFC 8216 §4.2, or `None` if it is not one: names are `[A-Z0-9-]+`, a quoted value
/// starts with `"` and holds no `"`, CR, LF (§4.2) or backslash (which FFmpeg would read as an escape), an unquoted value is not
/// empty and holds no `"`, whitespace or backslash, and pairs are separated by single commas with nothing in between.
fn attribute_list(value: &str) -> Option<Vec<(&str, &str)>> {
    let mut attributes = Vec::new();
    let mut rest = value;
    loop {
        let (name, after) = rest.split_once('=')?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        {
            return None;
        }
        let (value, after) = if let Some(inner) = after.strip_prefix('"') {
            let close = inner.find('"')?;
            if inner[..close].contains(['\r', '\n', '\\']) {
                return None;
            }
            (&after[..close + 2], &inner[close + 1..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            let value = &after[..end];
            if value.is_empty()
                || value
                    .bytes()
                    .any(|b| b == b'"' || b == b'\\' || b.is_ascii_whitespace())
            {
                return None;
            }
            (value, &after[end..])
        };
        attributes.push((name, value));
        if after.is_empty() {
            return Some(attributes);
        }
        rest = after.strip_prefix(',')?;
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
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100,URI=\"/jf/Videos/abc/iframe.m3u8\"\r\n\
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
            #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100,URI=\"/{TOKEN}/Videos/abc/iframe.m3u8\"\r\n\
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
            // Read differently by FFmpeg's attribute parser (manager's review, FFmpeg 6.1.1, 2026-10-26): an unquoted value ends at
            // whitespace, a quote opens only at a value's start, a backslash escapes inside quotes.
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,IV=0x1 URI=\"http://192.168.1.10/k\"\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,X=a\"b,URI=\"http://192.168.1.10/k\"\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,X=\"a\\\",Y=\",URI=\"http://192.168.1.10/k\"\n",
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\" ,BYTERANGE=\"1@0\"\n",
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,uri=\"http://192.168.1.10/a.m3u8\"\n",
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,,URI=\"a.m3u8\"\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\r.key\"\n",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=k.key\n",
            "#EXTM3U\n#EXTINF:6,a \"title\"\n0.ts\n",
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\r0.m3u8\n",
        ] {
            assert_eq!(rewrite(text), Err(PlaylistError::Malformed), "{text:?}");
        }
    }

    #[test]
    fn tags_that_are_not_attribute_lists_pass_unchanged() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-PROGRAM-DATE-TIME:2026-10-26T10:00:00.000Z\n\
            #EXTINF:6.0,a title, with a comma\n#EXT-X-BYTERANGE:1000@720\n#EXT-X-ENDLIST\n";
        assert_eq!(rewrite(text).unwrap(), text);
    }

    #[test]
    fn a_rewritten_playlist_past_its_limit_fails() {
        let mut text = String::from("#EXTM3U\n");
        while text.len() < MAX_PLAYLIST_BYTES - 2 {
            text.push_str("a\n");
        }
        assert_eq!(rewrite(&text), Err(PlaylistError::TooLarge));
    }

    #[test]
    fn one_long_tag_line_fails_once_its_output_would_pass_the_limit() {
        // 1,000 two-byte URIs, each becoming a path of over 4 KiB: about 500 times the input, in one line.
        let deep = format!("/videos/abc/{}/main.m3u8", "d".repeat(4096));
        let mut line = String::from("#EXT-X-FUTURE:A=1");
        for _ in 0..1_000 {
            line.push_str(",URI=\"x\"");
        }
        let text = format!("#EXTM3U\n{line}\n");
        let base = url("https://media.example.com/jf/");
        let playlist = url(&format!("https://media.example.com/jf{deep}"));
        let prefixes = Prefixes::new(vec!["/Videos/abc/".into()]).unwrap();
        let rewriter = Rewriter {
            playlist: &playlist,
            base: &base,
            prefixes: &prefixes,
            token: TOKEN,
        };
        // Under the limit the line is rewritten in full; over it, the line fails before the output passes the limit.
        assert!(rewriter.rewrite(&text).unwrap().len() > 4_000_000);
        let mut long = String::from("#EXT-X-FUTURE:A=1");
        for _ in 0..5_000 {
            long.push_str(",URI=\"x\"");
        }
        assert_eq!(
            rewriter.rewrite(&format!("#EXTM3U\n{long}\n")),
            Err(PlaylistError::TooLarge)
        );
        let mut out = "a".repeat(MAX_REWRITTEN_BYTES - 1);
        assert_eq!(append(&mut out, "bc"), Err(PlaylistError::TooLarge));
        assert_eq!(out.len(), MAX_REWRITTEN_BYTES - 1);
        assert_eq!(append(&mut out, "b"), Ok(()));
    }

    #[test]
    fn a_body_that_starts_with_the_magic_is_a_playlist() {
        for head in [
            &b"#EXTM3U\n"[..],
            b"\xef\xbb\xbf#EXTM3U",
            b"  \r\n#extm3u\n",
            b"#EXT",
        ] {
            assert_eq!(starts_like_playlist(head, true), Some(true), "{head:?}");
        }
        for head in [
            &b""[..],
            b"  ",
            b"\x00\x00\x00\x18ftypmp42",
            b"G@\x11",
            b"#EXTX",
            b"WEBVTT",
        ] {
            assert_eq!(starts_like_playlist(head, true), Some(false), "{head:?}");
        }
        assert_eq!(starts_like_playlist(b"  #EX", false), None);
        assert_eq!(starts_like_playlist(b"\x47", false), Some(false));
        assert_eq!(
            starts_like_playlist(&[b' '; SNIFF_LIMIT], false),
            Some(true)
        );
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
        assert!(is_playlist(&headers, "/Audio/abc/list.m3u"));
        assert!(!is_playlist(&headers, "/Videos/abc/stream.mp4"));
        for value in [
            "application/vnd.apple.mpegurl",
            "application/x-mpegURL; charset=utf-8",
            "audio/mpegurl",
            "audio/x-mpegurl",
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
