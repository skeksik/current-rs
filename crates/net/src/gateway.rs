//! Credentials for an authentication gateway in front of a server, per address (ADR 0017): an HTTP Basic password, extra headers and
//! the gateway's cookies.
//!
//! [`ServerClient`](crate::ServerClient) adds them to every request and every followed redirect to the address they belong to, after
//! the request log entry is built. Cookies are matched per request by RFC 6265 (domain, path, Secure, expiry), and cookies a gateway
//! extends or rotates are updated from its `Set-Cookie` answers, only when they already apply to the address (the owner's choice 4B).
//! No type here has a `Display`, and their Debug output shows no name and no value.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use reqwest::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use url::{Host, Url};

/// Why a gateway credential was refused. No variant carries the name or value that was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GatewayError {
    /// A Basic user name with `:` (RFC 7617 §2) or a control character.
    #[error("the user name cannot be used")]
    InvalidUserName,
    /// A Basic password with a control character.
    #[error("the password cannot be used")]
    InvalidPassword,
    /// A header name that is not a valid HTTP field name.
    #[error("the header name is not valid")]
    InvalidHeaderName,
    /// A header name that Current sets itself (`Cookie`, `Host`, the framing headers, and `Authorization` when it is taken).
    #[error("Current sets this header itself")]
    ReservedHeader,
    /// A header value that is not valid header text.
    #[error("the header value is not valid")]
    InvalidHeaderValue,
    /// A cookie whose name, value, domain or path cannot be sent.
    #[error("the cookie cannot be used")]
    InvalidCookie,
}

/// Whether the service puts its own credentials in `Authorization`, in which case an extra header may not take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationUse {
    /// The service uses `Authorization` (Jellyfin by default).
    Service,
    /// `Authorization` is free.
    Free,
}

/// Header names an extra header may never take: Current sets them itself, or they frame the message.
const RESERVED: &[&str] = &[
    "cookie",
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "upgrade",
    "te",
    "trailer",
    "expect",
];

/// An HTTP Basic user name and password (RFC 7617).
#[derive(Clone, PartialEq, Eq)]
pub struct BasicCredentials {
    user: String,
    password: String,
}

impl BasicCredentials {
    /// # Errors
    /// [`GatewayError::InvalidUserName`] for a user name with `:` or a control character, [`GatewayError::InvalidPassword`] for a
    /// password with a control character. RFC 7617 §2: "a user-id containing a colon character is invalid", and "the user-id and
    /// password MUST NOT contain any control characters".
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Result<Self, GatewayError> {
        let (user, password) = (user.into(), password.into());
        if user.contains(':') || user.chars().any(char::is_control) {
            return Err(GatewayError::InvalidUserName);
        }
        if password.chars().any(char::is_control) {
            return Err(GatewayError::InvalidPassword);
        }
        Ok(Self { user, password })
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// `Basic <base64 of user:password>`, marked sensitive. RFC 7617 §2.1: with `charset="UTF-8"` the user-id and password are
    /// encoded as UTF-8; servers that do not announce it get UTF-8 as well, which is what browsers send today (not verified per
    /// browser).
    fn header_value(&self) -> Option<HeaderValue> {
        let pair = format!("{}:{}", self.user, self.password);
        let mut value =
            HeaderValue::from_str(&format!("Basic {}", base64(pair.as_bytes()))).ok()?;
        value.set_sensitive(true);
        Some(value)
    }
}

impl std::fmt::Debug for BasicCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BasicCredentials([redacted])")
    }
}

/// A header sent on every request to the address (a service token, a proxy's secret header).
#[derive(Clone, PartialEq, Eq)]
pub struct ExtraHeader {
    name: HeaderName,
    value: HeaderValue,
}

impl ExtraHeader {
    /// # Errors
    /// [`GatewayError::InvalidHeaderName`], [`GatewayError::ReservedHeader`] for a name Current sets itself (`Authorization` only with
    /// [`AuthorizationUse::Service`]), [`GatewayError::InvalidHeaderValue`] for a value that is not header text.
    pub fn new(
        name: &str,
        value: &str,
        authorization: AuthorizationUse,
    ) -> Result<Self, GatewayError> {
        let name = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| GatewayError::InvalidHeaderName)?;
        if RESERVED.contains(&name.as_str())
            || (name == AUTHORIZATION && authorization == AuthorizationUse::Service)
        {
            return Err(GatewayError::ReservedHeader);
        }
        let mut value =
            HeaderValue::from_str(value.trim()).map_err(|_| GatewayError::InvalidHeaderValue)?;
        value.set_sensitive(true);
        Ok(Self { name, value })
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The value as text; `None` for a value with bytes outside visible ASCII.
    pub fn value(&self) -> Option<&str> {
        self.value.to_str().ok()
    }
}

impl std::fmt::Debug for ExtraHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExtraHeader([redacted])")
    }
}

/// A gateway's cookie, with the attributes RFC 6265 §5.3 keeps. Build it with [`GatewayCookie::new`] and the setters.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayCookie {
    name: String,
    value: String,
    /// Lower case, without a leading dot.
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<SystemTime>,
}

impl GatewayCookie {
    /// A cookie for `domain`. A domain written with a leading dot (`.example.com`) is a domain cookie, sent to its subdomains too;
    /// any other is host-only (RFC 6265 §5.3 step 6). Path `/`, not Secure, no expiry until set.
    ///
    /// # Errors
    /// [`GatewayError::InvalidCookie`] for a name that is not a token (RFC 6265 §4.1.1), a value with a control character, `;` or
    /// anything that is not header text, or an empty domain.
    pub fn new(name: &str, value: &str, domain: &str) -> Result<Self, GatewayError> {
        let host_only = !domain.starts_with('.');
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        if !is_token(name)
            || !is_cookie_value(value)
            || domain.is_empty()
            || !is_header_text(&domain)
        {
            return Err(GatewayError::InvalidCookie);
        }
        Ok(Self {
            name: name.to_owned(),
            value: value.to_owned(),
            domain,
            host_only,
            path: "/".to_owned(),
            secure: false,
            http_only: false,
            expires: None,
        })
    }

    /// # Errors
    /// [`GatewayError::InvalidCookie`] for a path that does not start with `/` or is not header text.
    pub fn with_path(mut self, path: &str) -> Result<Self, GatewayError> {
        if !path.starts_with('/') || !is_header_text(path) || path.contains(';') {
            return Err(GatewayError::InvalidCookie);
        }
        path.clone_into(&mut self.path);
        Ok(self)
    }

    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    #[must_use]
    pub fn with_http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    /// When the cookie expires; `None` for a session cookie.
    #[must_use]
    pub fn with_expires(mut self, expires: Option<SystemTime>) -> Self {
        self.expires = expires;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn host_only(&self) -> bool {
        self.host_only
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn secure(&self) -> bool {
        self.secure
    }

    pub fn http_only(&self) -> bool {
        self.http_only
    }

    pub fn expires(&self) -> Option<SystemTime> {
        self.expires
    }

    fn expired(&self, now: SystemTime) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }

    /// Whether the cookie would go with a request to `url` now (RFC 6265 §5.4 step 1): domain, path, Secure and expiry. For
    /// keeping only the cookies of a web sign-in that apply to the server's address (ADR 0017, "What is taken").
    pub fn applies_to(&self, url: &Url) -> bool {
        self.applies(url, SystemTime::now())
    }

    /// Whether the cookie goes with a request to `url` at `now` (RFC 6265 §5.4 step 1): domain, path, Secure and expiry.
    fn applies(&self, url: &Url, now: SystemTime) -> bool {
        self.matches_url(url) && (!self.secure || url.scheme() == "https") && !self.expired(now)
    }

    /// Domain and path only.
    fn matches_url(&self, url: &Url) -> bool {
        let Some(host) = url.host() else {
            return false;
        };
        domain_matches(&host, &self.domain, self.host_only) && path_matches(url.path(), &self.path)
    }
}

impl std::fmt::Debug for GatewayCookie {
    /// Flags only: a cookie's name can name the gateway or an app id, and its value is a session.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayCookie")
            .field("host_only", &self.host_only)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("expires", &self.expires.is_some())
            .finish_non_exhaustive()
    }
}

/// Everything a gateway needs from requests to one address. A value the caller supplies with the address (from the keyring, ADR
/// 0013); hand it to a [`Gateway`] to use it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct GatewayCredentials {
    basic: Option<BasicCredentials>,
    headers: Vec<ExtraHeader>,
    cookies: Vec<GatewayCookie>,
}

impl GatewayCredentials {
    /// # Errors
    /// [`GatewayError::ReservedHeader`] for an extra `Authorization` header next to Basic credentials, which use it.
    pub fn new(
        basic: Option<BasicCredentials>,
        headers: Vec<ExtraHeader>,
        cookies: Vec<GatewayCookie>,
    ) -> Result<Self, GatewayError> {
        if basic.is_some() && headers.iter().any(|header| header.name == AUTHORIZATION) {
            return Err(GatewayError::ReservedHeader);
        }
        Ok(Self {
            basic,
            headers,
            cookies,
        })
    }

    pub fn basic(&self) -> Option<&BasicCredentials> {
        self.basic.as_ref()
    }

    pub fn headers(&self) -> &[ExtraHeader] {
        &self.headers
    }

    pub fn cookies(&self) -> &[GatewayCookie] {
        &self.cookies
    }

    pub fn is_empty(&self) -> bool {
        self.basic.is_none() && self.headers.is_empty() && self.cookies.is_empty()
    }
}

impl std::fmt::Debug for GatewayCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayCredentials")
            .field("basic", &self.basic.is_some())
            .field("headers", &self.headers.len())
            .field("cookies", &self.cookies.len())
            .finish()
    }
}

/// The gateway credentials of one address in use: shared by the clients of that address, updated from `Set-Cookie`. Give it to
/// [`GatewayAddress`](crate::GatewayAddress) or [`RoamingAddresses::with_gateways`](crate::RoamingAddresses::with_gateways).
pub struct Gateway {
    credentials: Mutex<GatewayCredentials>,
    changes: AtomicU64,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("Gateway");
        // `try_lock`: formatting a gateway while `apply` or `update` holds its lock must not deadlock.
        match self.credentials.try_lock() {
            Ok(credentials) => debug.field("credentials", &*credentials),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                debug.field("credentials", &*poisoned.into_inner())
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                debug.field("credentials", &format_args!("<locked>"))
            }
        };
        debug.field("changes", &self.changes()).finish()
    }
}

impl Gateway {
    pub fn new(credentials: GatewayCredentials) -> Self {
        Self {
            credentials: Mutex::new(credentials),
            changes: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GatewayCredentials> {
        // Every change is a single replace or retain, so a poisoned lock leaves a usable value.
        self.credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The credentials now, for storing them, with the [`Gateway::changes`] count they reflect: store that count with them and
    /// store again once `changes()` differs from it. Expired cookies are dropped first (which counts as a change, already included
    /// in the count returned).
    pub fn snapshot(&self) -> (GatewayCredentials, u64) {
        self.snapshot_at(SystemTime::now())
    }

    fn snapshot_at(&self, now: SystemTime) -> (GatewayCredentials, u64) {
        let mut credentials = self.lock();
        let before = credentials.cookies.len();
        credentials.cookies.retain(|cookie| !cookie.expired(now));
        if credentials.cookies.len() != before {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
        // Every change happens under the lock, so the count read here matches the credentials.
        (credentials.clone(), self.changes())
    }

    /// Replaces the credentials, for a new sign-in to the gateway (ADR 0017): every request from now on carries the new ones.
    /// Counts as a change.
    pub fn replace(&self, credentials: GatewayCredentials) {
        let mut current = self.lock();
        *current = credentials;
        self.changes.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times the credentials changed since this handle was made: a cookie updated or removed from `Set-Cookie`, or expired
    /// cookies dropped by [`Gateway::snapshot`]. Store a new snapshot when it differs from the count the last stored one reflects.
    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Relaxed)
    }

    /// Adds the credentials for a request to `url` to `headers`. A header already set (by the request or `RequestAuth`) is never
    /// replaced: Jellyfin's `Authorization` moves away on its own when a Basic gateway is configured (ADR 0017). Every value added is
    /// marked sensitive.
    pub(crate) fn apply(&self, url: &Url, headers: &mut HeaderMap, now: SystemTime) {
        let credentials = self.lock();
        let present: Vec<HeaderName> = headers.keys().cloned().collect();
        if let Some(basic) = &credentials.basic
            && !present.contains(&AUTHORIZATION)
            && let Some(value) = basic.header_value()
        {
            headers.insert(AUTHORIZATION, value);
        }
        for header in &credentials.headers {
            if !present.contains(&header.name) {
                headers.append(header.name.clone(), header.value.clone());
            }
        }
        if present.contains(&COOKIE) {
            return;
        }
        let mut cookies: Vec<&GatewayCookie> = credentials
            .cookies
            .iter()
            .filter(|cookie| cookie.applies(url, now))
            .collect();
        // Longer paths first (RFC 6265 §5.4 step 2); the sort is stable, so otherwise in the order they were given.
        cookies.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        if cookies.is_empty() {
            return;
        }
        let line = cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        if let Ok(mut value) = HeaderValue::from_str(&line) {
            value.set_sensitive(true);
            headers.insert(COOKIE, value);
        }
    }

    /// The credentials a stream at `url` needs, for a player that sends its own requests (ADR 0019, "Gateways, tokens and the
    /// player"): the Basic header, the extra headers and the cookies that apply to `url` now, longer paths first.
    pub(crate) fn for_stream(&self, url: &Url, now: SystemTime) -> StreamCredentials {
        let credentials = self.lock();
        let mut headers = Vec::new();
        if let Some(value) = credentials
            .basic
            .as_ref()
            .and_then(BasicCredentials::header_value)
            .and_then(|value| value.to_str().map(str::to_owned).ok())
        {
            headers.push((AUTHORIZATION.as_str().to_owned(), value));
        }
        for header in &credentials.headers {
            if let Some(value) = header.value() {
                headers.push((header.name().to_owned(), value.to_owned()));
            }
        }
        let mut cookies: Vec<&GatewayCookie> = credentials
            .cookies
            .iter()
            .filter(|cookie| cookie.applies(url, now))
            .collect();
        cookies.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        StreamCredentials {
            headers,
            cookies: cookies
                .into_iter()
                .map(|cookie| StreamCookie {
                    name: cookie.name.clone(),
                    value: cookie.value.clone(),
                    path: cookie.path.clone(),
                    secure: cookie.secure,
                })
                .collect(),
        }
    }

    /// Reads the `Set-Cookie` headers of an answer from `url` and updates the cookies they name that already apply to `url` (same
    /// name, domain and path). New cookies are ignored, and a Secure cookie is never changed by an answer over http.
    pub(crate) fn update(&self, url: &Url, headers: &HeaderMap, now: SystemTime) {
        let set: Vec<SetCookie> = headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|text| SetCookie::parse(text, url, now))
            .collect();
        if set.is_empty() {
            return;
        }
        let secure_answer = url.scheme() == "https";
        let mut credentials = self.lock();
        let mut changed = false;
        for update in set {
            let Some(index) = credentials.cookies.iter().position(|cookie| {
                cookie.name == update.name
                    && cookie.domain == update.domain
                    && cookie.path == update.path
                    && cookie.matches_url(url)
            }) else {
                continue;
            };
            let cookie = &mut credentials.cookies[index];
            if (cookie.secure || update.secure) && !secure_answer {
                continue;
            }
            if update.expires.is_some_and(|expires| expires <= now) {
                credentials.cookies.remove(index);
                changed = true;
                continue;
            }
            let next = GatewayCookie {
                name: update.name,
                value: update.value,
                domain: update.domain,
                host_only: update.host_only,
                path: update.path,
                // Never weakened: a Secure cookie stays Secure.
                secure: cookie.secure || update.secure,
                http_only: update.http_only,
                expires: update.expires,
            };
            if *cookie != next {
                *cookie = next;
                changed = true;
            }
        }
        if changed {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One parsed `Set-Cookie` (RFC 6265 §5.2 and §5.3), with domain and path already resolved against the answer's URL.
struct SetCookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<SystemTime>,
}

impl SetCookie {
    /// `None` for a header the RFC says to ignore, or one whose value could not be sent back.
    fn parse(text: &str, url: &Url, now: SystemTime) -> Option<Self> {
        let (pair, attributes) = text.split_once(';').unwrap_or((text, ""));
        let (name, value) = pair.split_once('=')?;
        let (name, value) = (name.trim(), value.trim());
        if !is_token(name) || !is_cookie_value(value) {
            return None;
        }
        let host = url.host()?;
        let (mut domain, mut path) = (None, None);
        let (mut max_age, mut expires) = (None, None);
        let (mut secure, mut http_only) = (false, false);
        for attribute in attributes.split(';') {
            let (key, value) = attribute.split_once('=').unwrap_or((attribute, ""));
            let (key, value) = (key.trim(), value.trim());
            match key.to_ascii_lowercase().as_str() {
                "expires" => expires = cookie_date(value).or(expires),
                "max-age" => max_age = max_age_at(value, now).or(max_age),
                "domain" if !value.is_empty() => {
                    domain = Some(value.trim_start_matches('.').to_ascii_lowercase());
                }
                "path" => {
                    path = value.starts_with('/').then(|| value.to_owned());
                }
                "secure" => secure = true,
                "httponly" => http_only = true,
                _ => {}
            }
        }
        let (domain, host_only) = match domain {
            Some(domain) if domain_matches(&host, &domain, false) => (domain, false),
            Some(_) => return None,
            None => (host_name(&host), true),
        };
        Some(Self {
            name: name.to_owned(),
            value: value.to_owned(),
            domain,
            host_only,
            path: path.unwrap_or_else(|| default_path(url.path())),
            secure,
            http_only,
            // Max-Age wins over Expires (RFC 6265 §5.3 step 3).
            expires: max_age.or(expires),
        })
    }
}

/// A host as a cookie domain: lower case, IPv6 in brackets as `url` writes it.
fn host_name(host: &Host<&str>) -> String {
    host.to_string().to_ascii_lowercase()
}

/// RFC 6265 §5.1.3 domain-match, or equality for a host-only cookie. IP addresses match only themselves.
fn domain_matches(host: &Host<&str>, domain: &str, host_only: bool) -> bool {
    let name = host_name(host);
    if name == domain {
        return true;
    }
    if host_only {
        return false;
    }
    matches!(host, Host::Domain(_))
        && name.len() > domain.len()
        && name.ends_with(domain)
        && name.as_bytes()[name.len() - domain.len() - 1] == b'.'
}

/// RFC 6265 §5.1.4 path-match.
fn path_matches(request: &str, cookie: &str) -> bool {
    let request = if request.is_empty() { "/" } else { request };
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')))
}

/// RFC 6265 §5.1.4 default-path: the request path up to, not including, its last `/`; `/` if that leaves nothing.
fn default_path(request: &str) -> String {
    if !request.starts_with('/') {
        return "/".to_owned();
    }
    match request.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(end) => request[..end].to_owned(),
    }
}

/// A `Max-Age` value (RFC 6265 §5.2.2): seconds from `now`; zero or less means already expired.
fn max_age_at(value: &str, now: SystemTime) -> Option<SystemTime> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if value.starts_with('-') {
        return Some(SystemTime::UNIX_EPOCH);
    }
    // Too many digits for u64 is far in the future: cap at a century.
    let seconds = digits
        .parse::<u64>()
        .unwrap_or(u64::MAX)
        .min(100 * 365 * 24 * 3600);
    if seconds == 0 {
        return Some(SystemTime::UNIX_EPOCH);
    }
    now.checked_add(Duration::from_secs(seconds))
}

/// RFC 6265 §5.1.1's cookie-date algorithm. Dates before 1970 count as the epoch (expired).
fn cookie_date(text: &str) -> Option<SystemTime> {
    let is_delimiter = |c: char| matches!(c, '\t' | ' '..='/' | ';'..='@' | '['..='`' | '{'..='~');
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in text.split(is_delimiter).filter(|token| !token.is_empty()) {
        if time.is_none()
            && let Some(found) = time_of(token)
        {
            time = Some(found);
        } else if day.is_none()
            && let Some(found) = leading_digits(token, 1, 2)
        {
            day = Some(found);
        } else if month.is_none()
            && let Some(found) = month_of(token)
        {
            month = Some(found);
        } else if year.is_none()
            && let Some(found) = leading_digits(token, 2, 4)
        {
            year = Some(found);
        }
    }
    let ((hour, minute, second), day, month, mut year) = (time?, day?, month?, year?);
    if (70..=99).contains(&year) {
        year += 1900;
    } else if year <= 69 {
        year += 2000;
    }
    if !(1..=31).contains(&day) || year < 1601 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = days * 86_400 + i64::from(hour * 3600 + minute * 60 + second);
    Some(match u64::try_from(seconds) {
        Ok(seconds) => SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
        Err(_) => SystemTime::UNIX_EPOCH,
    })
}

/// `1*2DIGIT ":" 1*2DIGIT ":" 1*2DIGIT ( non-digit *OCTET )`.
fn time_of(token: &str) -> Option<(u32, u32, u32)> {
    let mut parts = token.splitn(3, ':');
    let hour = exact_digits(parts.next()?, 1, 2)?;
    let minute = exact_digits(parts.next()?, 1, 2)?;
    let second = leading_digits(parts.next()?, 1, 2)?;
    Some((hour, minute, second))
}

/// `min*max DIGIT` followed by nothing or a non-digit.
fn leading_digits(token: &str, min: usize, max: usize) -> Option<u32> {
    let count = token.bytes().take_while(u8::is_ascii_digit).count();
    if count < min || count > max {
        return None;
    }
    token[..count].parse().ok()
}

fn exact_digits(token: &str, min: usize, max: usize) -> Option<u32> {
    if token.len() < min || token.len() > max || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

fn month_of(token: &str) -> Option<u32> {
    let prefix = token.get(..3)?.to_ascii_lowercase();
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let index = months.iter().position(|month| *month == prefix)?;
    u32::try_from(index + 1).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date; `None` for a date that does not exist (RFC 6265 §5.1.1 step 5).
fn days_from_civil(year: u32, month: u32, day: u32) -> Option<i64> {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let length = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day > length {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let (year, month, day) = (i64::from(year), i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// RFC 9110 §5.6.2 token.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A cookie value Current can send back: header text without `;` or control characters.
fn is_cookie_value(text: &str) -> bool {
    is_header_text(text) && !text.contains(';')
}

/// Text that fits in a header value: no control characters.
fn is_header_text(text: &str) -> bool {
    !text.chars().any(char::is_control) && HeaderValue::from_str(text).is_ok()
}

/// Standard base64 with padding (RFC 4648 §4).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn base64_matches_rfc_4648_section_10() {
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        // RFC 7617 §2.1's example of a UTF-8 user name.
        assert_eq!(base64("test:123£".as_bytes()), "dGVzdDoxMjPCow==");
    }

    #[test]
    fn basic_credentials_refuse_colons_and_control_characters() {
        assert_eq!(
            BasicCredentials::new("a:b", "x"),
            Err(GatewayError::InvalidUserName)
        );
        assert_eq!(
            BasicCredentials::new("a\nb", "x"),
            Err(GatewayError::InvalidUserName)
        );
        assert_eq!(
            BasicCredentials::new("a", "x\u{7f}"),
            Err(GatewayError::InvalidPassword)
        );
        let basic = BasicCredentials::new("Aladdin", "open sesame").unwrap();
        let value = basic.header_value().unwrap();
        assert_eq!(value, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert!(value.is_sensitive());
    }

    #[test]
    fn extra_headers_refuse_names_current_sets() {
        let header =
            |name: &str, use_: AuthorizationUse| ExtraHeader::new(name, "v", use_).map(|_| ());
        for name in [
            "Cookie",
            "host",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "TE",
            "Upgrade",
        ] {
            assert_eq!(
                header(name, AuthorizationUse::Free),
                Err(GatewayError::ReservedHeader),
                "{name}"
            );
        }
        assert_eq!(
            header("Authorization", AuthorizationUse::Service),
            Err(GatewayError::ReservedHeader)
        );
        assert_eq!(header("Authorization", AuthorizationUse::Free), Ok(()));
        assert_eq!(
            header("CF-Access-Client-Id", AuthorizationUse::Service),
            Ok(())
        );
        assert_eq!(
            header("bad name", AuthorizationUse::Free),
            Err(GatewayError::InvalidHeaderName)
        );
        assert_eq!(
            ExtraHeader::new("x-gate", "a\nb", AuthorizationUse::Free).map(|_| ()),
            Err(GatewayError::InvalidHeaderValue)
        );
        let authorization =
            ExtraHeader::new("Authorization", "Bearer x", AuthorizationUse::Free).unwrap();
        let basic = BasicCredentials::new("u", "p").unwrap();
        assert_eq!(
            GatewayCredentials::new(Some(basic), vec![authorization], Vec::new()),
            Err(GatewayError::ReservedHeader)
        );
    }

    #[test]
    fn cookies_refuse_what_cannot_be_sent() {
        assert!(GatewayCookie::new("a b", "v", "media.example.com").is_err());
        assert!(GatewayCookie::new("", "v", "media.example.com").is_err());
        assert!(GatewayCookie::new("a", "v;w", "media.example.com").is_err());
        assert!(GatewayCookie::new("a", "v\r\n", "media.example.com").is_err());
        assert!(GatewayCookie::new("a", "v", ".").is_err());
        let cookie = GatewayCookie::new("a", "v", "media.example.com").unwrap();
        assert!(cookie.clone().with_path("relative").is_err());
        assert!(cookie.with_path("/jellyfin").is_ok());
    }

    #[test]
    fn domains_match_by_rfc_6265() {
        let domain = |h: &str, d: &str, only: bool| {
            let url = url(&format!("http://{h}/"));
            domain_matches(&url.host().unwrap(), d, only)
        };
        assert!(domain("media.example.com", "media.example.com", true));
        assert!(!domain("a.media.example.com", "media.example.com", true));
        assert!(domain("a.media.example.com", "media.example.com", false));
        assert!(!domain("badexample.com", "example.com", false));
        assert!(!domain("192.168.1.10", "168.1.10", false));
        assert!(domain("192.168.1.10", "192.168.1.10", true));
    }

    #[test]
    fn paths_match_by_rfc_6265() {
        assert!(path_matches("/", "/"));
        assert!(path_matches("/a/b", "/"));
        assert!(path_matches("/jellyfin/Items", "/jellyfin"));
        assert!(path_matches("/jellyfin/Items", "/jellyfin/"));
        assert!(!path_matches("/jellyfinx", "/jellyfin"));
        assert!(!path_matches("/", "/jellyfin"));
        assert_eq!(default_path("/a/b/c"), "/a/b");
        assert_eq!(default_path("/a"), "/");
        assert_eq!(default_path(""), "/");
    }

    fn gateway(cookies: Vec<GatewayCookie>) -> Gateway {
        Gateway::new(GatewayCredentials::new(None, Vec::new(), cookies).unwrap())
    }

    fn cookie_line(gateway: &Gateway, to: &str, now: u64) -> Option<String> {
        let mut headers = HeaderMap::new();
        gateway.apply(&url(to), &mut headers, at(now));
        headers.get(COOKIE).map(|value| {
            assert!(value.is_sensitive());
            value.to_str().unwrap().to_owned()
        })
    }

    #[test]
    fn only_matching_cookies_are_sent() {
        let gateway = gateway(vec![
            GatewayCookie::new("host", "1", "media.example.com").unwrap(),
            GatewayCookie::new("parent", "2", ".example.com").unwrap(),
            GatewayCookie::new("secure", "3", "media.example.com")
                .unwrap()
                .with_secure(true),
            GatewayCookie::new("deep", "4", "media.example.com")
                .unwrap()
                .with_path("/jellyfin")
                .unwrap(),
            GatewayCookie::new("old", "5", "media.example.com")
                .unwrap()
                .with_expires(Some(at(NOW))),
            GatewayCookie::new("other", "6", "other.example.org").unwrap(),
        ]);
        assert_eq!(
            cookie_line(&gateway, "https://media.example.com/jellyfin/Items", NOW).as_deref(),
            Some("deep=4; host=1; parent=2; secure=3")
        );
        assert_eq!(
            cookie_line(&gateway, "http://media.example.com/", NOW - 1).as_deref(),
            Some("host=1; parent=2; old=5")
        );
        assert_eq!(cookie_line(&gateway, "https://192.168.1.10/", NOW), None);
        assert_eq!(
            cookie_line(&gateway, "https://sub.example.com/", NOW).as_deref(),
            Some("parent=2")
        );
    }

    #[test]
    fn headers_already_set_are_kept() {
        let basic = BasicCredentials::new("u", "p").unwrap();
        let extra = ExtraHeader::new("x-gate", "gate-value", AuthorizationUse::Service).unwrap();
        let gateway =
            Gateway::new(GatewayCredentials::new(Some(basic), vec![extra], Vec::new()).unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("MediaBrowser Token=\"t\""),
        );
        gateway.apply(&url("https://media.example.com/"), &mut headers, at(NOW));
        assert_eq!(headers[AUTHORIZATION], "MediaBrowser Token=\"t\"");
        assert_eq!(headers["x-gate"], "gate-value");
        assert!(headers["x-gate"].is_sensitive());
        let mut headers = HeaderMap::new();
        gateway.apply(&url("https://media.example.com/"), &mut headers, at(NOW));
        assert_eq!(headers[AUTHORIZATION], "Basic dTpw");
        assert!(headers[AUTHORIZATION].is_sensitive());
    }

    fn set_cookie(gateway: &Gateway, from: &str, lines: &[&str], now: u64) {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(SET_COOKIE, HeaderValue::from_str(line).unwrap());
        }
        gateway.update(&url(from), &headers, at(now));
    }

    #[test]
    fn set_cookie_updates_only_cookies_that_apply() {
        let gateway = gateway(vec![
            GatewayCookie::new("session", "old", "media.example.com")
                .unwrap()
                .with_secure(true),
            GatewayCookie::new("sso", "a", ".example.com").unwrap(),
        ]);
        set_cookie(
            &gateway,
            "https://media.example.com/web/index.html",
            &[
                "session=new; Path=/; Secure; HttpOnly; Max-Age=3600",
                "sso=b; Domain=.example.com; Path=/",
                "tracker=x; Path=/",
                "elsewhere=y; Domain=other.example.org; Path=/",
            ],
            NOW,
        );
        assert_eq!(gateway.changes(), 1);
        let (snapshot, count) = gateway.snapshot_at(at(NOW));
        assert_eq!(count, 1);
        let cookies = snapshot.cookies();
        assert_eq!(cookies.len(), 2);
        assert_eq!(
            (cookies[0].value(), cookies[0].expires()),
            ("new", Some(at(NOW + 3600)))
        );
        assert!(cookies[0].http_only() && cookies[0].secure());
        assert_eq!(cookies[1].value(), "b");
        // The same values again change nothing.
        set_cookie(
            &gateway,
            "https://media.example.com/",
            &["sso=b; Domain=example.com; Path=/"],
            NOW,
        );
        assert_eq!(gateway.changes(), 1);
    }

    #[test]
    fn set_cookie_without_a_path_uses_the_default_path() {
        let gateway = gateway(vec![
            GatewayCookie::new("s", "1", "media.example.com").unwrap(),
        ]);
        // Default path `/web`: another cookie than the one at `/`.
        set_cookie(&gateway, "https://media.example.com/web/x", &["s=2"], NOW);
        assert_eq!(gateway.changes(), 0);
        set_cookie(&gateway, "https://media.example.com/x", &["s=2"], NOW);
        assert_eq!(gateway.snapshot_at(at(NOW)).0.cookies()[0].value(), "2");
    }

    #[test]
    fn set_cookie_never_touches_a_secure_cookie_over_http() {
        let gateway = gateway(vec![
            GatewayCookie::new("s", "1", "media.example.com")
                .unwrap()
                .with_secure(true),
        ]);
        set_cookie(&gateway, "http://media.example.com/", &["s=2; Path=/"], NOW);
        set_cookie(
            &gateway,
            "http://media.example.com/",
            &["s=; Path=/; Max-Age=0"],
            NOW,
        );
        assert_eq!(gateway.changes(), 0);
        assert_eq!(gateway.snapshot_at(at(NOW)).0.cookies()[0].value(), "1");
    }

    #[test]
    fn set_cookie_can_delete_and_expiry_drops_at_snapshot() {
        let gateway = gateway(vec![
            GatewayCookie::new("a", "1", "media.example.com").unwrap(),
            GatewayCookie::new("b", "2", "media.example.com").unwrap(),
        ]);
        set_cookie(
            &gateway,
            "https://media.example.com/",
            &[
                "a=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
                "b=3; Path=/; Expires=Sat, 15 Jan 2028 08:00:00 GMT",
            ],
            NOW,
        );
        assert_eq!(gateway.changes(), 1);
        let (snapshot, count) = gateway.snapshot_at(at(NOW));
        assert_eq!((snapshot.cookies().len(), count), (1, 1));
        let expires = snapshot.cookies()[0].expires().unwrap();
        // Not sent once expired, and dropped at the next snapshot.
        let after = expires
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(
            cookie_line(&gateway, "https://media.example.com/", after),
            None
        );
        let (snapshot, count) = gateway.snapshot_at(at(after));
        assert!(snapshot.cookies().is_empty());
        // The drop is in the returned count: no extra change after storing it.
        assert_eq!((count, gateway.changes()), (2, 2));
        assert_eq!(gateway.snapshot_at(at(after)).1, 2);
    }

    #[test]
    fn cookie_dates_follow_rfc_6265() {
        let seconds = |text: &str| {
            cookie_date(text).map(|time| {
                time.duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            })
        };
        // 1994-11-06 08:49:37 UTC is 784111777.
        assert_eq!(seconds("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(seconds("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(seconds("Sun Nov  6 08:49:37 1994"), Some(784_111_777));
        assert_eq!(
            seconds("Thu, 29 Feb 2024 00:00:00 GMT"),
            Some(1_709_164_800)
        );
        assert_eq!(seconds("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(seconds("Fri, 31 Dec 1999 23:59:59 GMT"), Some(946_684_799));
        assert_eq!(seconds("Wed, 31 Feb 2024 00:00:00 GMT"), None);
        assert_eq!(seconds("Sun, 06 Nov 1994 24:00:00 GMT"), None);
        assert_eq!(seconds("Sun, 06 Nov 1994"), None);
        assert_eq!(seconds("Mon, 01 Jan 1601 00:00:00 GMT"), Some(0));
        assert_eq!(seconds(""), None);
    }

    #[test]
    fn max_age_is_relative_and_zero_expires() {
        assert_eq!(max_age_at("60", at(NOW)), Some(at(NOW + 60)));
        assert_eq!(max_age_at("0", at(NOW)), Some(SystemTime::UNIX_EPOCH));
        assert_eq!(max_age_at("-1", at(NOW)), Some(SystemTime::UNIX_EPOCH));
        assert_eq!(max_age_at("1x", at(NOW)), None);
        assert_eq!(max_age_at("", at(NOW)), None);
    }

    #[test]
    fn debug_does_not_wait_for_the_lock() {
        let gateway = gateway(Vec::new());
        let _held = gateway.lock();
        assert_eq!(
            format!("{gateway:?}"),
            "Gateway { credentials: <locked>, changes: 0 }"
        );
    }

    #[test]
    fn debug_shows_no_name_or_value() {
        let credentials = GatewayCredentials::new(
            Some(BasicCredentials::new("gate-user", "gate-password").unwrap()),
            vec![
                ExtraHeader::new("x-gate-name", "gate-header-value", AuthorizationUse::Free)
                    .unwrap(),
            ],
            vec![
                GatewayCookie::new("gate_cookie_name", "gate-cookie-value", "media.example.com")
                    .unwrap(),
            ],
        )
        .unwrap();
        let gateway = Gateway::new(credentials.clone());
        let text = format!(
            "{credentials:?} {gateway:?} {:?} {:?} {:?}",
            credentials.basic(),
            credentials.headers(),
            credentials.cookies()
        );
        for secret in [
            "gate-user",
            "gate-password",
            "x-gate-name",
            "gate-header-value",
            "gate_cookie_name",
            "gate-cookie-value",
            "media.example.com",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(text.contains("cookies: 1"), "{text}");
    }

    #[test]
    fn replace_swaps_the_credentials_and_counts() {
        let gateway = gateway(vec![
            GatewayCookie::new("a", "1", "media.example.com").unwrap(),
        ]);
        gateway.replace(
            GatewayCredentials::new(
                None,
                Vec::new(),
                vec![GatewayCookie::new("a", "2", "media.example.com").unwrap()],
            )
            .unwrap(),
        );
        assert_eq!(gateway.changes(), 1);
        assert_eq!(
            cookie_line(&gateway, "https://media.example.com/", NOW),
            Some("a=2".into())
        );
    }

    #[test]
    fn applies_to_checks_domain_path_and_secure() {
        let cookie = GatewayCookie::new("a", "1", ".example.com")
            .unwrap()
            .with_secure(true);
        assert!(cookie.applies_to(&url("https://media.example.com/jellyfin")));
        assert!(!cookie.applies_to(&url("http://media.example.com/")));
        assert!(!cookie.applies_to(&url("https://login.example.net/")));
        let path = GatewayCookie::new("b", "1", "media.example.com")
            .unwrap()
            .with_path("/other")
            .unwrap();
        assert!(!path.applies_to(&url("http://media.example.com/jellyfin")));
    }
}

/// The gateway credentials for one stream that a player fetches itself ([`ServerClient::stream_credentials`](crate::ServerClient::stream_credentials)).
/// Empty for an address without a gateway. Its Debug shows header and cookie names only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StreamCredentials {
    /// Header name and value: the Basic `Authorization` header and the extra headers.
    pub headers: Vec<(String, String)>,
    /// The cookies that apply to the stream's URL.
    pub cookies: Vec<StreamCookie>,
}

impl StreamCredentials {
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.cookies.is_empty()
    }
}

impl std::fmt::Debug for StreamCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCredentials")
            .field("headers", &self.headers.len())
            .field("cookies", &self.cookies)
            .finish()
    }
}

/// A cookie for a stream: the player scopes it to the stream's host.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamCookie {
    pub name: String,
    pub value: String,
    pub path: String,
    /// Sent only over https.
    pub secure: bool,
}

impl std::fmt::Debug for StreamCookie {
    /// Flags only, as [`GatewayCookie`]'s.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCookie")
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}
