//! Who the client is and how it proves it: the `Authorization: MediaBrowser ...` header Jellyfin reads on every request (ADR 0015).

use std::sync::{Arc, PoisonError, RwLock};

use current_net::RequestAuth;
use current_net::header::{AUTHORIZATION, HeaderMap, HeaderValue};

/// The name Jellyfin shows for the app (Dashboard, Devices: the app column).
pub const CLIENT_NAME: &str = "Current";
/// Longest device name sent, in characters. Jellyfin sets no limit; a name this long is already more than its dashboard shows.
const DEVICE_NAME_LIMIT: usize = 64;

/// Declares a secret string from a Jellyfin server: redacted `Debug`, no `Display`, so it cannot reach a log by accident.
macro_rules! secret_string {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The value, for the one place that sends or stores it.
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
    };
}

secret_string! {
    /// A Jellyfin access token: what a sign-in returns and every later request carries.
    AccessToken
}

secret_string! {
    /// The secret half of a Quick Connect request. Whoever holds it can collect the sign-in once a user approves the code.
    QuickConnectSecret
}

/// What the client tells Jellyfin about itself. Jellyfin keeps one device record per device id and user, shown in its dashboard
/// as the device name next to the client and version (ADR 0015).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIdentity {
    pub client: String,
    pub version: String,
    pub device: String,
    pub device_id: String,
}

impl ClientIdentity {
    /// Current's identity: client `Current`, the core's version, and the device name the UI passed, cleaned up by
    /// [`device_name`].
    pub fn current(device: &str, device_id: &str) -> Self {
        Self {
            client: CLIENT_NAME.to_owned(),
            version: current_model::version().to_owned(),
            device: device_name(device),
            device_id: device_id.to_owned(),
        }
    }
}

/// The device name to send: `name` trimmed, with control characters turned into spaces, cut at 64 characters; the platform's
/// name when nothing is left (Jellyfin refuses a sign-in without a device name).
pub fn device_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned: String = cleaned.trim().chars().take(DEVICE_NAME_LIMIT).collect();
    let cleaned = cleaned.trim_end();
    if cleaned.is_empty() {
        platform_name().to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// The platform's name, the device name when the UI has none.
pub fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "ios" => "iOS",
        "tvos" => "tvOS",
        "android" => "Android",
        other => other,
    }
}

/// `MediaBrowser Client="…", Device="…", DeviceId="…", Version="…"`, plus `Token="…"` when signed in. Jellyfin URL-decodes each
/// value (`AuthorizationContext.GetParts`), so values are percent-encoded: a comma or quote in a device name cannot break the header.
pub fn authorization_header(identity: &ClientIdentity, token: Option<&AccessToken>) -> String {
    let mut header = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        encode(&identity.client),
        encode(&identity.device),
        encode(&identity.device_id),
        encode(&identity.version),
    );
    if let Some(token) = token {
        header.push_str(&format!(", Token=\"{}\"", encode(token.expose())));
    }
    header
}

/// Percent-encodes everything but RFC 3986's unreserved characters, as Jellyfin's Kotlin SDK does (it sends a space as `+`; `%20`
/// decodes the same).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Where a client's token is kept while it runs. Shared between the client and its [`JellyfinAuth`].
pub(crate) type TokenSlot = Arc<RwLock<Option<AccessToken>>>;

pub(crate) fn read(slot: &TokenSlot) -> Option<AccessToken> {
    slot.read().unwrap_or_else(PoisonError::into_inner).clone()
}

pub(crate) fn write(slot: &TokenSlot, token: Option<AccessToken>) {
    *slot.write().unwrap_or_else(PoisonError::into_inner) = token;
}

/// Adds the authorization header to every request, with the token once there is one. It runs after the request log entry is built
/// (ADR 0006), so the header never reaches the log.
pub(crate) struct JellyfinAuth {
    pub(crate) identity: ClientIdentity,
    pub(crate) token: TokenSlot,
}

impl RequestAuth for JellyfinAuth {
    fn apply(&self, headers: &mut HeaderMap, _query: &mut Vec<(String, String)>) {
        let header = authorization_header(&self.identity, read(&self.token).as_ref());
        // Every byte is ASCII after encoding, so this cannot fail.
        if let Ok(mut value) = HeaderValue::from_str(&header) {
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn identity(device: &str) -> ClientIdentity {
        ClientIdentity {
            client: "Current".into(),
            version: "0.1.0".into(),
            device: device.into(),
            device_id: "0123456789abcdef0123456789abcdef".into(),
        }
    }

    /// Jellyfin's `AuthorizationContext.GetParts` (v12.2), ported: split on commas outside quotes, `key=value`, trim the quotes,
    /// URL-decode.
    fn parse(header: &str) -> HashMap<String, String> {
        let rest = header.split_once(' ').unwrap().1;
        let mut result = HashMap::new();
        let (mut escaped, mut start, mut key) = (false, 0, String::new());
        let chars: Vec<char> = rest.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c == '"' || c == ',' {
                escaped = (!escaped) == (c == '"');
                if c == ',' && !escaped {
                    if start < i {
                        let value: String = chars[start..i].iter().collect();
                        result.insert(
                            std::mem::take(&mut key),
                            url_decode(value.trim_matches('"')),
                        );
                    }
                    start = i + 1;
                }
            } else if !escaped && c == '=' {
                key = chars[start..i].iter().collect::<String>().trim().to_owned();
                start = i + 1;
            }
        }
        if start < chars.len() {
            let value: String = chars[start..].iter().collect();
            result.insert(key, url_decode(value.trim_matches('"')));
        }
        result
    }

    fn url_decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' => {
                    out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                    i += 2;
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_header_round_trips_through_jellyfins_parser() {
        let device = r#"Living room, "big" TV = Wohnzimmer äö 😀 +%"#;
        let token = AccessToken::new("abc123");
        let header = authorization_header(&identity(device), Some(&token));
        assert!(
            header.starts_with("MediaBrowser Client=\"Current\", Device=\""),
            "{header}"
        );
        assert!(header.is_ascii());
        let parts = parse(&header);
        assert_eq!(parts["Client"], "Current");
        assert_eq!(parts["Device"], device);
        assert_eq!(parts["DeviceId"], "0123456789abcdef0123456789abcdef");
        assert_eq!(parts["Version"], "0.1.0");
        assert_eq!(parts["Token"], "abc123");
    }

    #[test]
    fn without_a_token_the_header_has_no_token() {
        let header = authorization_header(&identity("PC"), None);
        assert!(!header.contains("Token"), "{header}");
        assert_eq!(parse(&header).len(), 4);
    }

    #[test]
    fn device_names_are_cleaned_and_never_empty() {
        assert_eq!(device_name("  Office PC \n"), "Office PC");
        assert_eq!(device_name("a\tb\r\nc"), "a b  c");
        assert_eq!(device_name(" \n "), platform_name());
        assert_eq!(device_name(""), platform_name());
        assert_eq!(device_name(&"é".repeat(100)).chars().count(), 64);
        assert!(!platform_name().is_empty());
    }

    #[test]
    fn secrets_never_show_in_debug() {
        let text = format!(
            "{:?} {:?}",
            AccessToken::new("token-value-123"),
            QuickConnectSecret::new("secret-value-456")
        );
        assert_eq!(
            text,
            "AccessToken(<redacted>) QuickConnectSecret(<redacted>)"
        );
    }

    #[test]
    fn the_hook_adds_a_sensitive_header_with_the_current_token() {
        let slot: TokenSlot = Arc::default();
        let auth = JellyfinAuth {
            identity: identity("PC"),
            token: slot.clone(),
        };
        let mut headers = HeaderMap::new();
        auth.apply(&mut headers, &mut Vec::new());
        assert!(!headers[AUTHORIZATION].to_str().unwrap().contains("Token"));

        write(&slot, Some(AccessToken::new("t1")));
        auth.apply(&mut headers, &mut Vec::new());
        let value = &headers[AUTHORIZATION];
        assert!(value.is_sensitive());
        assert!(value.to_str().unwrap().ends_with(", Token=\"t1\""));
        assert_eq!(headers.len(), 1);
    }
}
