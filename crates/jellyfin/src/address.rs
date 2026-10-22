//! What a user types as a server's address, turned into the base URLs to try (ADR 0012).

use std::net::Ipv6Addr;

use current_net::Url;

/// Jellyfin's default http port (`NetworkConfiguration.DefaultHttpPort`).
pub const DEFAULT_HTTP_PORT: u16 = 8096;
/// Jellyfin's default https port (`NetworkConfiguration.DefaultHttpsPort`).
pub const DEFAULT_HTTPS_PORT: u16 = 8920;

/// Why typed text is not an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("enter the server's address")]
    Empty,
    /// A scheme other than http or https.
    #[error("the address must start with http:// or https://, or with the server's name")]
    UnsupportedScheme,
    /// A user name or password in the address: credentials never go in an address.
    #[error("the address must not contain a user name or password")]
    Credentials,
    #[error("this is not a valid address")]
    Invalid,
}

/// The base URLs to try for `input`, most likely first. Each has no query or fragment, and its path is the server's base path
/// without a trailing slash (`/` when there is none).
///
/// - No scheme: `http://` is assumed for parsing. Without a port, https on 443 and 8920 come first, then http on 8096 and 80; with
///   a port, both schemes are tried on it, http first for 80 and 8096.
/// - A scheme without a port: that scheme's default port, then Jellyfin's (8096 for http, 8920 for https).
/// - A scheme and a port: exactly that.
/// - A pasted web client URL (`.../web/index.html#/home`) is cut back to the base path before its last `web` segment.
///
/// # Errors
/// [`AddressError`] when the text is not an http(s) address.
pub fn parse_address(input: &str) -> Result<Vec<Url>, AddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AddressError::Empty);
    }
    let (scheme, rest) = match input.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(AddressError::UnsupportedScheme);
            }
            (Some(scheme), rest)
        }
        None => (None, input),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.contains('@') {
        return Err(AddressError::Credentials);
    }
    // A bare IPv6 literal needs brackets before a URL parser can tell its colons from a port.
    let authority = match authority.parse::<Ipv6Addr>() {
        Ok(_) => format!("[{authority}]"),
        Err(_) => authority.to_owned(),
    };
    let port_given = match authority.rfind(']') {
        Some(end) => authority[end..].contains(':'),
        None => authority.contains(':'),
    };
    let text = format!(
        "{}://{authority}{tail}",
        scheme.as_deref().unwrap_or("http")
    );
    let mut url = Url::parse(&text).map_err(|_| AddressError::Invalid)?;
    if url.host().is_none() || url.cannot_be_a_base() {
        return Err(AddressError::Invalid);
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = base_path(&url);
    url.set_path(&path);

    let with = |scheme: &str, port: Option<u16>| {
        let mut candidate = url.clone();
        // Both only fail for URLs that cannot be a base or have no host, ruled out above.
        let _ = candidate.set_scheme(scheme);
        let _ = candidate.set_port(port);
        candidate
    };
    let port = url.port_or_known_default();
    Ok(match (scheme.as_deref(), port_given) {
        (Some(_), true) => vec![url],
        (Some("https"), false) => {
            vec![with("https", None), with("https", Some(DEFAULT_HTTPS_PORT))]
        }
        (Some(_), false) => vec![with("http", None), with("http", Some(DEFAULT_HTTP_PORT))],
        (None, true) => {
            let (http, https) = (with("http", port), with("https", port));
            if matches!(port, Some(80 | DEFAULT_HTTP_PORT)) {
                vec![http, https]
            } else {
                vec![https, http]
            }
        }
        (None, false) => vec![
            with("https", None),
            with("https", Some(DEFAULT_HTTPS_PORT)),
            with("http", Some(DEFAULT_HTTP_PORT)),
            with("http", None),
        ],
    })
}

/// The URL's path without trailing slashes and without a web client path: everything from the last `web` segment on (Jellyfin
/// serves its web client at `{base}/web/`).
fn base_path(url: &Url) -> String {
    let mut segments: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.collect())
        .unwrap_or_default();
    if let Some(web) = segments
        .iter()
        .rposition(|segment| segment.eq_ignore_ascii_case("web"))
    {
        segments.truncate(web);
    }
    while segments.last().is_some_and(|segment| segment.is_empty()) {
        segments.pop();
    }
    format!("/{}", segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(input: &str) -> Vec<String> {
        match parse_address(input) {
            Ok(urls) => urls.into_iter().map(String::from).collect(),
            Err(error) => panic!("{input}: {error:?}"),
        }
    }

    #[test]
    fn a_name_alone_tries_https_then_http_on_jellyfin_and_default_ports() {
        assert_eq!(
            candidates("media.example.com"),
            [
                "https://media.example.com/",
                "https://media.example.com:8920/",
                "http://media.example.com:8096/",
                "http://media.example.com/",
            ]
        );
    }

    #[test]
    fn a_port_without_a_scheme_tries_both_schemes_on_it() {
        assert_eq!(
            candidates("192.168.1.10:8096"),
            ["http://192.168.1.10:8096/", "https://192.168.1.10:8096/"]
        );
        assert_eq!(
            candidates("192.168.1.10:8920"),
            ["https://192.168.1.10:8920/", "http://192.168.1.10:8920/"]
        );
        assert_eq!(
            candidates("media.example.com:80"),
            ["http://media.example.com/", "https://media.example.com:80/"]
        );
    }

    #[test]
    fn a_scheme_without_a_port_adds_the_jellyfin_port() {
        assert_eq!(
            candidates("http://192.168.1.10"),
            ["http://192.168.1.10/", "http://192.168.1.10:8096/"]
        );
        assert_eq!(
            candidates("HTTPS://Media.Example.com"),
            [
                "https://media.example.com/",
                "https://media.example.com:8920/"
            ]
        );
    }

    #[test]
    fn a_scheme_and_a_port_are_used_as_given() {
        assert_eq!(
            candidates("http://192.168.1.10:8096"),
            ["http://192.168.1.10:8096/"]
        );
        // A default port typed out still counts as given.
        assert_eq!(
            candidates("https://media.example.com:443"),
            ["https://media.example.com/"]
        );
    }

    #[test]
    fn base_paths_are_kept_without_trailing_slashes() {
        assert_eq!(
            candidates("https://media.example.com/jellyfin/"),
            [
                "https://media.example.com/jellyfin",
                "https://media.example.com:8920/jellyfin"
            ]
        );
        assert_eq!(
            candidates("http://192.168.1.10:8096/a/b//?x=1#y"),
            ["http://192.168.1.10:8096/a/b"]
        );
    }

    #[test]
    fn a_pasted_web_client_url_becomes_its_base() {
        assert_eq!(
            candidates("http://192.168.1.10:8096/web/index.html#/home"),
            ["http://192.168.1.10:8096/"]
        );
        assert_eq!(
            candidates("https://media.example.com:443/jellyfin/web/#/details?id=1"),
            ["https://media.example.com/jellyfin"]
        );
        // A server whose base path is itself `/web` keeps it: only the last `web` segment goes.
        assert_eq!(
            candidates("http://192.168.1.10:8096/web/web/"),
            ["http://192.168.1.10:8096/web"]
        );
    }

    #[test]
    fn ipv6_literals_with_and_without_brackets() {
        assert_eq!(
            candidates("[fd00::10]:8096"),
            ["http://[fd00::10]:8096/", "https://[fd00::10]:8096/"]
        );
        assert_eq!(candidates("fd00::10").len(), 4);
        assert_eq!(candidates("fd00::10")[0], "https://[fd00::10]/");
        assert_eq!(candidates("http://[::1]:8096"), ["http://[::1]:8096/"]);
    }

    #[test]
    fn international_names_become_punycode() {
        assert_eq!(
            candidates("https://médias.example.com:443"),
            ["https://xn--mdias-bsa.example.com/"]
        );
    }

    #[test]
    fn bad_input_is_refused() {
        assert_eq!(parse_address("  "), Err(AddressError::Empty));
        assert_eq!(
            parse_address("ftp://media.example.com"),
            Err(AddressError::UnsupportedScheme)
        );
        assert_eq!(
            parse_address("http://user:pass@media.example.com"),
            Err(AddressError::Credentials)
        );
        assert_eq!(
            parse_address("http://media example.com"),
            Err(AddressError::Invalid)
        );
        assert_eq!(
            parse_address("media.example.com:99999"),
            Err(AddressError::Invalid)
        );
        assert_eq!(parse_address("http://"), Err(AddressError::Invalid));
        // A zone id is not part of a URL (WHATWG URL Standard).
        assert_eq!(
            parse_address("http://[fe80::1%25eth0]:8096"),
            Err(AddressError::Invalid)
        );
    }
}
