//! Redaction of mpv's log lines (ADR 0019, "Gateways, tokens and the player"): the session token, every gateway secret value and any
//! `ApiKey=`/`api_key=` value become `<redacted>` before a line leaves the engine.

/// What replaces a secret.
pub const REDACTED: &str = "<redacted>";

/// Query keys whose value is always redacted, matched without regard to case.
const SECRET_KEYS: [&str; 2] = ["apikey=", "api_key="];

/// Shorter secrets are not matched on their own: a three-letter value would shred ordinary text. Tokens, cookies and Basic
/// credentials are far longer.
const MIN_SECRET_LEN: usize = 6;

/// The secret values known to the engine. Values are only added, never removed, so a line about an earlier file is still redacted.
#[derive(Default, Clone)]
pub struct Redactor {
    // Longest first, so a secret that contains another is replaced whole.
    secrets: Vec<String>,
}

// Never print the secrets themselves.
impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

impl Redactor {
    pub fn new() -> Redactor {
        Redactor::default()
    }

    /// Adds a secret value: a token, a cookie value or a header value. Its percent-encoded form is added too, since URLs carry it
    /// encoded, and for a value with a scheme (`Basic <credentials>`) the part after the scheme as well.
    pub fn add(&mut self, secret: &str) {
        let mut forms = vec![secret.to_owned(), percent_encode(secret)];
        if let Some((_, rest)) = secret.split_once(' ') {
            forms.push(rest.trim().to_owned());
        }
        for form in forms {
            if form.len() >= MIN_SECRET_LEN && !self.secrets.contains(&form) {
                self.secrets.push(form);
            }
        }
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// The line with every known secret and every `ApiKey`/`api_key` value replaced.
    pub fn redact(&self, line: &str) -> String {
        let mut out = line.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        redact_keys(&out)
    }
}

fn redact_keys(line: &str) -> String {
    // ASCII lowercasing keeps every byte index, so positions found in `lower` are valid in `line`.
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some((start, key_len)) = SECRET_KEYS
        .iter()
        .filter_map(|key| lower[search..].find(key).map(|i| (search + i, key.len())))
        .min_by_key(|(i, _)| *i)
    {
        let value_start = start + key_len;
        let value_len = line[value_start..]
            .find(is_value_end)
            .unwrap_or(line.len() - value_start);
        out.push_str(&line[copied..value_start]);
        if value_len > 0 {
            out.push_str(REDACTED);
        }
        copied = value_start + value_len;
        search = copied;
    }
    out.push_str(&line[copied..]);
    out
}

/// Where a query value ends in a URL inside a log line.
fn is_value_end(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '&' | '#' | '"' | '\'' | '<' | '>' | ',' | ';' | ')' | ']' | '}'
        )
}

/// RFC 3986 percent-encoding of everything but unreserved characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn replaces_known_secrets_anywhere() {
        let mut r = Redactor::new();
        r.add(TOKEN);
        r.add("gate cookie/value=1");
        let line = format!(
            "Opening http://192.168.1.10/x?t={TOKEN} with cookie gate%20cookie%2Fvalue%3D1 and gate cookie/value=1"
        );
        let red = r.redact(&line);
        assert!(!red.contains(TOKEN), "{red}");
        assert!(!red.contains("gate cookie"), "{red}");
        assert!(!red.contains("gate%20cookie"), "{red}");
        assert_eq!(red.matches(REDACTED).count(), 3, "{red}");
    }

    #[test]
    fn replaces_api_key_values_without_knowing_them() {
        let r = Redactor::new();
        let line = "Playing: https://media.example.com/Videos/1/stream?static=true&ApiKey=secretvalue&tag=x";
        assert_eq!(
            r.redact(line),
            "Playing: https://media.example.com/Videos/1/stream?static=true&ApiKey=<redacted>&tag=x"
        );
        assert_eq!(
            r.redact("a?api_key=abc b API_KEY=\"q\" apikey="),
            "a?api_key=<redacted> b API_KEY=\"q\" apikey="
        );
        assert_eq!(r.redact("x.m3u8?APIKEY=zz#f"), "x.m3u8?APIKEY=<redacted>#f");
    }

    #[test]
    fn basic_credentials_are_redacted_with_and_without_the_scheme() {
        let mut r = Redactor::new();
        r.add("Basic c3RhbmQtaW46bm90LXJlYWw=");
        assert_eq!(
            r.redact("Authorization: Basic c3RhbmQtaW46bm90LXJlYWw="),
            "Authorization: <redacted>"
        );
        assert_eq!(r.redact("got c3RhbmQtaW46bm90LXJlYWw="), "got <redacted>");
    }

    #[test]
    fn short_values_and_ordinary_text_are_left_alone() {
        let mut r = Redactor::new();
        r.add("abc");
        r.add("");
        assert_eq!(r.redact("abc abc"), "abc abc");
        assert_eq!(
            r.redact("[ffmpeg/demuxer] hls: Opening segment 3"),
            "[ffmpeg/demuxer] hls: Opening segment 3"
        );
        assert_eq!(format!("{r:?}"), "Redactor { secrets: 0 }");
    }

    #[test]
    fn non_ascii_text_survives() {
        let r = Redactor::new();
        assert_eq!(
            r.redact("Ünïcode ApiKey=ä€ end"),
            "Ünïcode ApiKey=<redacted> end"
        );
    }
}
