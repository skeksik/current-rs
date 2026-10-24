//! Gateway credentials in the app (ADR 0017): what the UI hands over (a Basic password, a web sign-in's cookies, extra headers),
//! how they are kept per address, and the text they are stored as in the keyring.
//!
//! Values only ever go one way: from the UI into the core. No state, error or Debug output here holds a password, a cookie's name
//! or value, or a header's value.

use std::time::{Duration, SystemTime};

use current_jellyfin::HeaderPlacement;
use current_model::{AuthHeader, GatewayKind};
use current_net::{
    AuthorizationUse, BasicCredentials, ExtraHeader, GatewayCookie, GatewayCredentials,
    GatewayError, Url,
};
use serde::{Deserialize, Serialize};

/// A cookie the sign-in window's web view holds, as the UI reads it after a navigation. Debug shows no name and no value.
#[derive(Clone, PartialEq, Eq)]
pub struct CookieInput {
    pub name: String,
    pub value: String,
    /// The cookie's domain as the web view reports it: with a leading dot for a domain cookie, without for a host-only one
    /// (RFC 6265 §5.3; how each platform's web view writes it is not verified).
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// `None` for a session cookie.
    pub expires: Option<SystemTime>,
}

impl std::fmt::Debug for CookieInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieInput([redacted])")
    }
}

/// An extra header the user typed (advanced). Debug shows no name and no value.
#[derive(Clone, PartialEq, Eq)]
pub struct HeaderInput {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for HeaderInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeaderInput([redacted])")
    }
}

/// Why a gateway credential the UI handed over was not used. Carries nothing the user typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GatewaySubmitError {
    /// The flow is not asking for this kind of credential now.
    #[error("nothing asks for this now")]
    NotAsked,
    /// The address is plain http outside the local network and the user has not confirmed it: no credential goes there before
    /// (ADR 0017, choice 5A).
    #[error("plain HTTP to this address must be confirmed first")]
    PlainHttpNotConfirmed,
    /// No header was given.
    #[error("no header was given")]
    NoHeaders,
    #[error(transparent)]
    Invalid(#[from] GatewayError),
}

/// The origin of `url` (scheme, host and port when not the default), for the password prompt and the sign-in window's title.
pub(crate) fn origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

/// The Basic credentials, checked (RFC 7617).
pub(crate) fn basic(user: &str, password: &str) -> Result<BasicCredentials, GatewaySubmitError> {
    Ok(BasicCredentials::new(user, password)?)
}

/// The headers, checked. `Authorization` is refused: Jellyfin uses it unless its header moved (ADR 0017), and a value outside
/// visible ASCII is refused too, since it could not be stored and read back as text.
pub(crate) fn headers(input: &[HeaderInput]) -> Result<Vec<ExtraHeader>, GatewaySubmitError> {
    if input.is_empty() {
        return Err(GatewaySubmitError::NoHeaders);
    }
    input
        .iter()
        .map(|header| {
            let built = ExtraHeader::new(&header.name, &header.value, AuthorizationUse::Service)?;
            if built.value().is_none() {
                return Err(GatewayError::InvalidHeaderValue.into());
            }
            Ok(built)
        })
        .collect()
}

/// The cookies that apply to `base` by RFC 6265 (domain, path, Secure, expiry), checked. Cookies for other hosts (an identity
/// provider's, a portal's) are dropped here, before anything is probed or stored. A cookie that cannot be used is dropped too:
/// a web view may hold cookies Current cannot send, and one such cookie must not stop the sign-in.
pub(crate) fn cookies(base: &Url, input: &[CookieInput]) -> Vec<GatewayCookie> {
    input
        .iter()
        .filter_map(|cookie| {
            GatewayCookie::new(&cookie.name, &cookie.value, &cookie.domain)
                .and_then(|built| built.with_path(&cookie.path))
                .ok()
                .map(|built| {
                    built
                        .with_secure(cookie.secure)
                        .with_http_only(cookie.http_only)
                        .with_expires(cookie.expires)
                })
        })
        .filter(|cookie| cookie.applies_to(base))
        .collect()
}

/// `current` with `basic`, `headers` or `cookies` replacing its own of that kind; `None` leaves a kind as it is.
pub(crate) fn merge(
    current: &GatewayCredentials,
    basic: Option<BasicCredentials>,
    headers: Option<Vec<ExtraHeader>>,
    cookies: Option<Vec<GatewayCookie>>,
) -> Result<GatewayCredentials, GatewayError> {
    GatewayCredentials::new(
        basic.or_else(|| current.basic().cloned()),
        headers.unwrap_or_else(|| current.headers().to_vec()),
        cookies.unwrap_or_else(|| current.cookies().to_vec()),
    )
}

/// The kinds the settings list for an address with `credentials`, never a value.
pub(crate) fn kinds(credentials: &GatewayCredentials) -> Vec<GatewayKind> {
    let mut kinds = Vec::new();
    if credentials.basic().is_some() {
        kinds.push(GatewayKind::BASIC);
    }
    if !credentials.cookies().is_empty() {
        kinds.push(GatewayKind::COOKIES);
    }
    if !credentials.headers().is_empty() {
        kinds.push(GatewayKind::HEADERS);
    }
    kinds
}

/// Jellyfin's header placement as the settings keep it. An unknown value is the default.
pub(crate) fn placement(stored: &AuthHeader) -> HeaderPlacement {
    if *stored == AuthHeader::X_EMBY_AUTHORIZATION {
        HeaderPlacement::EmbyAuthorization
    } else {
        HeaderPlacement::Authorization
    }
}

/// The settings' value for `placement`.
pub(crate) fn stored_placement(placement: HeaderPlacement) -> AuthHeader {
    match placement {
        HeaderPlacement::Authorization => AuthHeader::AUTHORIZATION,
        HeaderPlacement::EmbyAuthorization => AuthHeader::X_EMBY_AUTHORIZATION,
    }
}

/// The version of the stored text. A newer one is not read (the address then asks for a sign-in again).
const FORMAT: u32 = 1;

/// The keyring text of one address's gateway credentials. Private: only [`serialise`] and [`parse`] see it.
#[derive(Serialize, Deserialize)]
struct Stored {
    v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    basic: Option<StoredBasic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cookies: Vec<StoredCookie>,
}

#[derive(Serialize, Deserialize)]
struct StoredBasic {
    user: String,
    password: String,
}

#[derive(Serialize, Deserialize)]
struct StoredCookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    /// Milliseconds since 1970; `None` for a session cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_ms: Option<u64>,
}

/// The text `credentials` are stored as.
pub(crate) fn serialise(credentials: &GatewayCredentials) -> String {
    let stored = Stored {
        v: FORMAT,
        basic: credentials.basic().map(|basic| StoredBasic {
            user: basic.user().to_owned(),
            password: basic.password().to_owned(),
        }),
        headers: credentials
            .headers()
            .iter()
            // Values outside visible ASCII are refused when headers are given (`headers`), so none is lost here.
            .filter_map(|header| Some((header.name().to_owned(), header.value()?.to_owned())))
            .collect(),
        cookies: credentials
            .cookies()
            .iter()
            .map(|cookie| StoredCookie {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: cookie.domain().to_owned(),
                host_only: cookie.host_only(),
                path: cookie.path().to_owned(),
                secure: cookie.secure(),
                http_only: cookie.http_only(),
                expires_ms: cookie.expires().map(|at| {
                    let since = at
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default();
                    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                }),
            })
            .collect(),
    };
    // Serialising plain strings, numbers and booleans into a string cannot fail.
    serde_json::to_string(&stored).unwrap_or_default()
}

/// The credentials stored as `text`; `None` for text this version cannot read (a newer format, or damaged).
pub(crate) fn parse(text: &str) -> Option<GatewayCredentials> {
    let stored: Stored = serde_json::from_str(text).ok()?;
    if stored.v != FORMAT {
        return None;
    }
    let basic = match stored.basic {
        Some(basic) => Some(BasicCredentials::new(basic.user, basic.password).ok()?),
        None => None,
    };
    let headers = stored
        .headers
        .iter()
        .map(|(name, value)| ExtraHeader::new(name, value, AuthorizationUse::Service).ok())
        .collect::<Option<Vec<_>>>()?;
    let cookies = stored
        .cookies
        .iter()
        .map(|cookie| {
            let domain = if cookie.host_only {
                cookie.domain.clone()
            } else {
                format!(".{}", cookie.domain)
            };
            let expires = cookie
                .expires_ms
                .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
            GatewayCookie::new(&cookie.name, &cookie.value, &domain)
                .and_then(|built| built.with_path(&cookie.path))
                .ok()
                .map(|built| {
                    built
                        .with_secure(cookie.secure)
                        .with_http_only(cookie.http_only)
                        .with_expires(expires)
                })
        })
        .collect::<Option<Vec<_>>>()?;
    GatewayCredentials::new(basic, headers, cookies).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn cookie(name: &str, domain: &str) -> CookieInput {
        CookieInput {
            name: name.into(),
            value: format!("{name}-value"),
            domain: domain.into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires: Some(SystemTime::now() + Duration::from_secs(3600)),
        }
    }

    #[test]
    fn only_cookies_for_the_servers_address_are_kept() {
        let base = url("https://media.example.com/jellyfin");
        let kept = cookies(
            &base,
            &[
                cookie("host", "media.example.com"),
                cookie("parent", ".example.com"),
                // The identity provider's and another site's cookies stay in the web view.
                cookie("provider", "login.example.net"),
                cookie("sibling", "other.example.com"),
                CookieInput {
                    expires: Some(SystemTime::UNIX_EPOCH),
                    ..cookie("expired", "media.example.com")
                },
                CookieInput {
                    path: "/elsewhere".into(),
                    ..cookie("path", "media.example.com")
                },
                cookie("bad name", "media.example.com"),
            ],
        );
        let names: Vec<_> = kept.iter().map(GatewayCookie::name).collect();
        assert_eq!(names, ["host", "parent"]);
        // A Secure cookie never applies to plain http.
        assert!(
            cookies(
                &url("http://media.example.com/"),
                &[cookie("host", "media.example.com")]
            )
            .is_empty()
        );
    }

    #[test]
    fn credentials_round_trip_through_the_stored_text() {
        let credentials = GatewayCredentials::new(
            Some(BasicCredentials::new("gate-user", "gate password").unwrap()),
            headers(&[HeaderInput {
                name: "X-Gate-Token".into(),
                value: "header-secret".into(),
            }])
            .unwrap(),
            cookies(
                &url("https://media.example.com/"),
                &[
                    cookie("host", "media.example.com"),
                    CookieInput {
                        expires: None,
                        secure: false,
                        ..cookie("parent", ".example.com")
                    },
                ],
            ),
        )
        .unwrap();
        let text = serialise(&credentials);
        let read = parse(&text).unwrap();
        // Expiry is kept to the millisecond.
        assert_eq!(serialise(&read), text);
        assert_eq!(read.cookies().len(), 2);
        assert!(!read.cookies()[1].host_only());
        assert_eq!(read.cookies()[1].expires(), None);
        assert_eq!(read.basic().unwrap().password(), "gate password");
        assert_eq!(
            kinds(&read),
            [
                GatewayKind::BASIC,
                GatewayKind::COOKIES,
                GatewayKind::HEADERS
            ]
        );
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"v": 2}"#), None);
        assert_eq!(parse(r#"{"v": 1}"#), Some(GatewayCredentials::default()));
    }

    #[test]
    fn headers_refuse_authorization_and_text_that_cannot_be_stored() {
        let header = |name: &str, value: &str| HeaderInput {
            name: name.into(),
            value: value.into(),
        };
        assert_eq!(headers(&[]), Err(GatewaySubmitError::NoHeaders));
        assert_eq!(
            headers(&[header("Authorization", "Bearer x")]),
            Err(GatewaySubmitError::Invalid(GatewayError::ReservedHeader))
        );
        assert_eq!(
            headers(&[header("X-Token", "caf\u{e9}")]),
            Err(GatewaySubmitError::Invalid(
                GatewayError::InvalidHeaderValue
            ))
        );
        assert_eq!(headers(&[header("X-Token", "abc")]).unwrap().len(), 1);
    }

    #[test]
    fn merging_replaces_one_kind_at_a_time() {
        let base = url("https://media.example.com/");
        let first = merge(
            &GatewayCredentials::default(),
            Some(basic("u", "p").unwrap()),
            None,
            None,
        )
        .unwrap();
        let second = merge(
            &first,
            None,
            None,
            Some(cookies(&base, &[cookie("a", "media.example.com")])),
        )
        .unwrap();
        assert!(second.basic().is_some());
        assert_eq!(second.cookies().len(), 1);
        let third = merge(&second, None, None, Some(Vec::new())).unwrap();
        assert!(third.cookies().is_empty());
    }

    #[test]
    fn origins_and_placements() {
        assert_eq!(
            origin(&url("https://media.example.com/jellyfin")),
            "https://media.example.com"
        );
        assert_eq!(
            origin(&url("http://192.168.1.10:8096/")),
            "http://192.168.1.10:8096"
        );
        assert_eq!(
            placement(&AuthHeader::new("a-newer-header")),
            HeaderPlacement::Authorization
        );
        for placement_now in [
            HeaderPlacement::Authorization,
            HeaderPlacement::EmbyAuthorization,
        ] {
            assert_eq!(placement(&stored_placement(placement_now)), placement_now);
        }
    }

    #[test]
    fn inputs_show_nothing_in_debug() {
        let text = format!(
            "{:?} {:?} {:?}",
            cookie("cookie-name", "media.example.com"),
            HeaderInput {
                name: "X-Secret-Name".into(),
                value: "header-secret".into()
            },
            GatewaySubmitError::Invalid(GatewayError::InvalidCookie)
        );
        for secret in [
            "cookie-name",
            "X-Secret-Name",
            "header-secret",
            "media.example.com",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}
