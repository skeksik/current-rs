//! An authentication gateway's answer, as `current-jellyfin` reports it (ADR 0017): the shapes of `current-net`'s [`classify`], with
//! the realm kept as untrusted text.

use current_net::{GatewayShape, NetError, Response, classify};

/// Longest realm kept, in characters: the limit `current-net` already applies when it reads the challenge.
const REALM_LIMIT: usize = 100;

/// The realm of a Basic challenge: the server's own text, shown only as "The site says: ...", never as a title and never logged.
/// Debug shows nothing of it and there is no Display.
#[derive(Clone, PartialEq, Eq)]
pub struct Realm(String);

impl Realm {
    /// The realm from `text`, without control characters, trimmed and cut at 100 characters; `None` when nothing is left.
    pub fn new(text: &str) -> Option<Self> {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        Some(Self(text.chars().take(REALM_LIMIT).collect()))
    }

    /// The text, for the one place that shows it ("The site says: ...").
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Realm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Realm(<redacted>)")
    }
}

/// What a gateway in front of the server wants, from an answer that provably came from it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GatewayAnswer {
    /// A password gateway (HTTP Basic): the app asks for a user name and password for the address.
    #[error("the server's address asks for a password")]
    Password { realm: Option<Realm> },
    /// A web sign-in: the app opens the sign-in window on the server's address.
    #[error("the server's address needs a sign-in in a browser window")]
    SignIn,
    /// The gateway wants something else, such as extra headers.
    #[error("the server's address refused access")]
    Refused,
    /// A challenge of a scheme Current does not support (Digest, NTLM, Negotiate, Bearer, ...).
    #[error("the server's address asks for a sign-in method Current does not support")]
    UnsupportedChallenge,
}

impl GatewayAnswer {
    /// The gateway's answer in `result`, by `current-net`'s classification; `None` for the service's own answer or anything that is
    /// not a gateway's. A redirect counts as [`GatewayAnswer::SignIn`]: callers that keep a redirect apart check for it first.
    pub fn of(result: &Result<Response, NetError>) -> Option<Self> {
        match classify(result) {
            GatewayShape::Password { realm } => Some(Self::Password {
                realm: realm.as_deref().and_then(Realm::new),
            }),
            GatewayShape::SignIn => Some(Self::SignIn),
            GatewayShape::Refused => Some(Self::Refused),
            GatewayShape::Unsupported => Some(Self::UnsupportedChallenge),
            GatewayShape::Expected | GatewayShape::Other => None,
        }
    }

    /// The gateway's answer when `error` carries a `WWW-Authenticate` challenge: Jellyfin's own 401 and 403 never do (ADR 0017), so
    /// such an answer did not come from Jellyfin. `None` for any other error.
    pub fn challenged(error: &NetError) -> Option<Self> {
        match error {
            NetError::Auth { challenges, .. } if !challenges.is_empty() => {
                Self::of(&Err(error.clone()))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use current_net::{AuthChallenge, AuthScheme};

    use super::*;

    fn auth(status: u16, challenge: Option<(&str, Option<&str>)>) -> NetError {
        NetError::Auth {
            status,
            message: None,
            challenges: challenge
                .into_iter()
                .map(|(scheme, realm)| AuthChallenge {
                    scheme: AuthScheme::new(scheme),
                    realm: realm.map(str::to_owned),
                })
                .collect(),
            page: false,
        }
    }

    #[test]
    fn realms_are_cleaned_cut_and_hidden() {
        assert_eq!(
            Realm::new(" a\u{7}b ").map(|r| r.as_str().to_owned()),
            Some("ab".into())
        );
        assert_eq!(Realm::new(" \n "), None);
        assert_eq!(
            Realm::new(&"r".repeat(150)).map(|r| r.0.len()),
            Some(REALM_LIMIT)
        );
        let realm = Realm::new("Private Realm Text");
        assert_eq!(format!("{realm:?}"), "Some(Realm(<redacted>))");
        let answer = GatewayAnswer::Password { realm };
        assert!(!format!("{answer:?} {answer}").contains("Private"));
    }

    #[test]
    fn only_a_challenge_proves_a_gateway() {
        assert_eq!(
            GatewayAnswer::challenged(&auth(401, Some(("Basic", Some("Home"))))),
            Some(GatewayAnswer::Password {
                realm: Realm::new("Home")
            })
        );
        assert_eq!(
            GatewayAnswer::challenged(&auth(401, Some(("Negotiate", None)))),
            Some(GatewayAnswer::UnsupportedChallenge)
        );
        assert_eq!(GatewayAnswer::challenged(&auth(401, None)), None);
        assert_eq!(GatewayAnswer::challenged(&auth(403, None)), None);
        assert_eq!(GatewayAnswer::challenged(&NetError::Unreachable), None);
    }
}
