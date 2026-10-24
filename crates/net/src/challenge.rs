//! The challenges of a 401 or 403 answer, read from its `WWW-Authenticate` headers (RFC 9110 §11.6.1), for telling a gateway's
//! answer from the service's own (ADR 0017).

use std::borrow::Cow;

use reqwest::header::{HeaderMap, WWW_AUTHENTICATE};

/// Longest realm kept, in characters. A realm is the server's text, shown only as "The site says: ...".
const REALM_LIMIT: usize = 100;

/// An authentication scheme, as a server names it. An open value: unknown schemes pass through. Schemes are case-insensitive (RFC
/// 9110 §11.1), so the name is kept in lower case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthScheme(Cow<'static, str>);

impl AuthScheme {
    /// HTTP Basic (RFC 7617).
    pub const BASIC: Self = Self(Cow::Borrowed("basic"));

    /// A scheme by its name, in any case.
    pub fn new(name: &str) -> Self {
        Self(Cow::Owned(name.to_ascii_lowercase()))
    }

    /// The scheme's name, in lower case.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One challenge of a `WWW-Authenticate` header. Its Debug output shows the scheme only: the realm and any other parameter are the
/// server's text.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthChallenge {
    pub scheme: AuthScheme,
    /// The `realm` parameter, without control characters and cut at 100 characters. Untrusted text: never a title, never logged.
    pub realm: Option<String>,
}

impl std::fmt::Debug for AuthChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.scheme.as_str())
    }
}

/// Every challenge in `headers`, in order. Headers that are not text, and parts that do not parse, are skipped.
pub(crate) fn challenges(headers: &HeaderMap) -> Vec<AuthChallenge> {
    headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(parse)
        .collect()
}

/// A lexical item of a challenge list.
#[derive(Debug, PartialEq, Eq)]
enum Item<'a> {
    /// A token, a token68 or a quoted string's content, and whether it was quoted.
    Word(Cow<'a, str>, bool),
    Equals,
    Comma,
}

/// Splits a header value into words, `=` and `,`. Whitespace separates words.
fn lex(text: &str) -> Vec<Item<'_>> {
    let mut items = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        let Some(first) = rest.chars().next() else {
            return items;
        };
        match first {
            ',' => {
                items.push(Item::Comma);
                rest = &rest[1..];
            }
            '=' => {
                items.push(Item::Equals);
                rest = &rest[1..];
            }
            '"' => {
                let mut value = String::new();
                let mut chars = rest[1..].char_indices();
                let mut end = rest.len();
                while let Some((index, c)) = chars.next() {
                    match c {
                        '"' => {
                            end = index + 2;
                            break;
                        }
                        '\\' => {
                            if let Some((_, escaped)) = chars.next() {
                                value.push(escaped);
                            }
                        }
                        c => value.push(c),
                    }
                }
                items.push(Item::Word(Cow::Owned(value), true));
                rest = &rest[end.min(rest.len())..];
            }
            _ => {
                let end = rest.find([' ', '\t', ',', '=', '"']).unwrap_or(rest.len());
                // A token68 ends in `=` padding: keep the run of `=` that ends a word not followed by a value.
                let mut word_end = end;
                let padding = rest[end..].len() - rest[end..].trim_start_matches('=').len();
                if padding > 0 {
                    let after = rest[end + padding..].trim_start_matches([' ', '\t']);
                    if after.is_empty() || after.starts_with(',') {
                        word_end = end + padding;
                    }
                }
                items.push(Item::Word(Cow::Borrowed(&rest[..word_end]), false));
                rest = &rest[word_end..];
            }
        }
    }
}

/// The challenges of one header value: `scheme [token68 | param *("," param)]`, comma-separated (RFC 9110 §11.6.1).
fn parse(text: &str) -> Vec<AuthChallenge> {
    let items = lex(text);
    let mut challenges: Vec<AuthChallenge> = Vec::new();
    let mut index = 0;
    while index < items.len() {
        match (&items[index], items.get(index + 1)) {
            // `name = value`: a parameter of the challenge before it.
            (Item::Word(name, false), Some(Item::Equals)) => {
                let value = match items.get(index + 2) {
                    Some(Item::Word(value, _)) => {
                        index += 3;
                        Some(value)
                    }
                    _ => {
                        index += 2;
                        None
                    }
                };
                if let (Some(challenge), Some(value)) = (challenges.last_mut(), value)
                    && name.eq_ignore_ascii_case("realm")
                    && challenge.realm.is_none()
                {
                    challenge.realm = clean_realm(value);
                }
            }
            (Item::Word(scheme, false), next) => {
                challenges.push(AuthChallenge {
                    scheme: AuthScheme::new(scheme),
                    realm: None,
                });
                index += 1;
                // A token68 right after the scheme belongs to it.
                if let Some(Item::Word(_, false)) = next
                    && !matches!(items.get(index + 1), Some(Item::Equals))
                {
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    challenges
}

/// A realm as shown to the user: control characters removed, whitespace trimmed, cut at [`REALM_LIMIT`] characters.
fn clean_realm(value: &str) -> Option<String> {
    let text: String = value.chars().filter(|c| !c.is_control()).collect();
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(REALM_LIMIT).collect())
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    fn schemes(text: &str) -> Vec<(String, Option<String>)> {
        parse(text)
            .into_iter()
            .map(|c| (c.scheme.as_str().to_owned(), c.realm))
            .collect()
    }

    fn one(scheme: &str, realm: Option<&str>) -> (String, Option<String>) {
        (scheme.to_owned(), realm.map(str::to_owned))
    }

    #[test]
    fn basic_with_a_realm() {
        assert_eq!(
            schemes(r#"Basic realm="Media, at home""#),
            [one("basic", Some("Media, at home"))]
        );
        assert_eq!(
            schemes(r#"BASIC charset="UTF-8", REALM=simple"#),
            [one("basic", Some("simple"))]
        );
        assert_eq!(schemes("Basic"), [one("basic", None)]);
    }

    #[test]
    fn several_challenges_in_one_header() {
        assert_eq!(
            schemes(r#"Negotiate, Bearer realm="api", error="invalid_token", Basic realm="x\"y""#),
            [
                one("negotiate", None),
                one("bearer", Some("api")),
                one("basic", Some("x\"y"))
            ]
        );
    }

    #[test]
    fn token68_belongs_to_its_scheme() {
        assert_eq!(
            schemes("Negotiate abc123==, Basic realm=r"),
            [one("negotiate", None), one("basic", Some("r"))]
        );
        assert_eq!(schemes("NTLM TlRMTVNTUAACAAAA"), [one("ntlm", None)]);
    }

    #[test]
    fn realms_lose_control_characters_and_are_cut() {
        let long = "r".repeat(REALM_LIMIT + 20);
        let parsed = schemes(&format!("Basic realm=\"a\u{7}b\", Digest realm=\"{long}\""));
        assert_eq!(parsed[0], one("basic", Some("ab")));
        assert_eq!(parsed[1].1.as_ref().map(String::len), Some(REALM_LIMIT));
        assert_eq!(schemes(r#"Basic realm="  ""#), [one("basic", None)]);
    }

    #[test]
    fn broken_input_does_not_panic() {
        for text in [
            "",
            ",",
            "=",
            "\"",
            "Basic realm=",
            "Basic realm=\"open",
            "= x, ,Basic",
        ] {
            let _ = parse(text);
        }
        assert_eq!(schemes("= x, ,Basic"), [one("x", None), one("basic", None)]);
    }

    #[test]
    fn every_header_is_read() {
        let mut headers = HeaderMap::new();
        headers.append(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Digest realm=\"d\""),
        );
        headers.append(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"b\""),
        );
        let found: Vec<_> = challenges(&headers).into_iter().map(|c| c.scheme).collect();
        assert_eq!(found, [AuthScheme::new("Digest"), AuthScheme::BASIC]);
    }

    #[test]
    fn debug_shows_the_scheme_only() {
        let challenge = &parse(r#"Basic realm="Private Realm Text""#)[0];
        assert_eq!(format!("{challenge:?}"), "basic");
    }
}
