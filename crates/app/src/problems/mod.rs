//! Why a flow or a request failed, as every UI shows it: flat copies of the core's errors with nothing a user should not see (no
//! address, no redirect target, no server message, no OS error text; ADR 0006), and the problems in plain words ([`text`]).
//! `current-ffi` hands the same shapes across the bridge.

mod text;

use current_jellyfin::{AddressError, AuthError, DiscoveryError, GatewayAnswer, ProbeError};
use current_net::{GatewayError, NetError};
use current_store::StoreError;

use crate::{GatewaySignInError, GatewaySubmitError, SignInError};

pub use text::{
    EMBY_HEADER_IGNORED, GATEWAY_REFUSED, GATEWAY_UNSUPPORTED, GATEWAY_WEB_PAGE,
    GOOGLE_REFUSES_WEB_VIEWS, Platform, UNEXPECTED, browse_problem_text, discovery_problem_text,
    gateway_input_problem_text, gateway_problem_text, gateway_sign_in_problem_text,
    net_problem_text, probe_problem_text, sign_in_problem_text, store_problem_text,
};

/// Why the typed address is not one: the UI asks the user to fix it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressProblem {
    Empty,
    /// A scheme other than http or https.
    UnsupportedScheme,
    /// A user name or password in the address.
    Credentials,
    Invalid,
}

/// Why server setup found no usable server at an address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeProblem {
    InvalidAddress {
        reason: AddressProblem,
    },
    /// Nothing answered: offline, the name did not resolve, or nothing listens there.
    Unreachable,
    Timeout,
    /// The secure connection failed, typically an untrusted or mismatched certificate.
    Tls,
    /// Something answered, but not a Jellyfin server.
    NotJellyfin,
    /// The server is starting, or does not accept connections from this network.
    Unavailable,
    /// The server points elsewhere. Where it points stays in the core.
    Redirected {
        status: u16,
    },
    UnexpectedStatus {
        status: u16,
    },
    /// Older than the oldest version Current supports.
    TooOld {
        version: String,
    },
    /// The server's first-run setup is not finished.
    SetupIncomplete,
    /// The address leads to another server than the one expected.
    DifferentServer,
    /// An authentication gateway in front of the server wants something (ADR 0017). The flow's gateway states say what to do;
    /// this problem appears where no flow can ask, e.g. a saved address checked before a sign-in.
    Gateway {
        answer: GatewayProblem,
    },
}

/// What an authentication gateway in front of the server wants (ADR 0017). Never its realm or any other text it sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayProblem {
    /// A password (HTTP Basic).
    Password,
    /// A sign-in in a web page. Also what a plain web site at the address looks like.
    SignIn,
    /// It refuses without a sign-in: it wants something else, such as a service token in extra headers.
    Refused,
    /// A sign-in method Current does not support.
    Unsupported,
}

/// Why the gateway credentials the UI handed over were not used (ADR 0017). Nothing of what the user typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayInputProblem {
    /// The address is plain http outside the local network: confirm it first.
    PlainHttpNotConfirmed,
    /// No header was given.
    NoHeaders,
    /// A user name with `:` or a control character.
    InvalidUserName,
    /// A password with a control character.
    InvalidPassword,
    InvalidHeaderName,
    /// A header Current sets itself (`Authorization`, `Cookie`, `Host`, the framing headers).
    ReservedHeader,
    /// A header value that is not visible ASCII text.
    InvalidHeaderValue,
    InvalidCookie,
}

/// Why signing in to a saved server's gateway again failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewaySignInProblem {
    /// The address could not be checked: not reached, another server, or not a usable server.
    Probe { problem: ProbeProblem },
    /// The new credentials could not be saved.
    Save { problem: StoreProblem },
    /// The server was removed meanwhile.
    ServerRemoved,
}

/// Why a request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetProblem {
    Unreachable,
    Timeout,
    Tls,
    /// 401 or 403.
    Auth {
        status: u16,
    },
    UnexpectedStatus {
        status: u16,
    },
    /// A redirect that was not followed.
    Redirected {
        status: u16,
    },
    /// The response did not have the expected shape.
    Decode,
    /// The request could not be built: a bug, not a server problem.
    InvalidRequest,
}

/// Why the settings could not be read or saved. No path: it holds the user's account name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreProblem {
    /// Reading or writing the data folder failed. `step` says which (`read`, `save`...), `kind` the kind of I/O error.
    Io { step: String, kind: String },
    /// The settings belong to a newer version of Current: they are used read-only.
    NewerVersion { found: u64 },
    /// The system's random number generator failed.
    Random,
}

/// Why a sign-in failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInProblem {
    /// Unknown user or wrong password.
    WrongCredentials,
    /// The server knows the user but refuses them (disabled, locked out, not allowed from here, a time limit).
    Refused,
    QuickConnectDisabled,
    QuickConnectExpired,
    SessionRevoked,
    NotSignedIn,
    Network {
        problem: NetProblem,
    },
    /// The saved address no longer answers as the saved server.
    Probe {
        problem: ProbeProblem,
    },
    /// Signed in, but the sign-in could not be saved.
    Save {
        problem: StoreProblem,
    },
    /// The server was removed while signing in.
    ServerRemoved,
    /// An authentication gateway in front of the server answered instead (ADR 0017): sign in to it first.
    Gateway {
        answer: GatewayProblem,
    },
    /// The server ignored the sign-in's client details sent in `X-Emby-Authorization` (ADR 0017): it reads that header only with
    /// legacy authorization on. The text explains the two server-side fixes: turn on legacy authorization on the server and stop
    /// the gateway passing its own `Authorization` header on; or add a gateway rule that lets the server's API through.
    EmbyHeaderIgnored,
    /// The server's answer is being checked (mid-session only, never from a sign-in): try again in a moment.
    Unconfirmed,
}

/// Why the search for local servers failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryProblem {
    /// The system refused, e.g. the Local Network privilege on Apple platforms.
    NotPermitted,
    NoNetwork,
    Failed,
}

/// Why a browse request (a page, a row, a detail page, a played or favourite change) failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseProblem {
    /// The session ended (a sign-out, a removal, a new sign-in): a new `Browse` is needed.
    NotSignedIn,
    /// The server revoked the token: sign in again.
    SessionRevoked,
    /// The server knows the user but refuses the request.
    Refused,
    /// The server's answer is being checked: try again in a moment.
    Unconfirmed,
    /// The session goes on, but the server refused this to the user: the item or its library is gone, or hidden from the user
    /// (ADR 0015 as amended 2026-10-25).
    NotAvailable,
    /// An authentication gateway in front of the server answered instead (ADR 0017): sign in to it again.
    Gateway {
        answer: GatewayProblem,
    },
    Network {
        problem: NetProblem,
    },
}

impl From<&AddressError> for AddressProblem {
    fn from(error: &AddressError) -> Self {
        match error {
            AddressError::Empty => Self::Empty,
            AddressError::UnsupportedScheme => Self::UnsupportedScheme,
            AddressError::Credentials => Self::Credentials,
            AddressError::Invalid => Self::Invalid,
        }
    }
}

impl From<&ProbeError> for ProbeProblem {
    fn from(error: &ProbeError) -> Self {
        match error {
            ProbeError::InvalidAddress(reason) => Self::InvalidAddress {
                reason: reason.into(),
            },
            ProbeError::Unreachable => Self::Unreachable,
            ProbeError::Timeout => Self::Timeout,
            ProbeError::Tls => Self::Tls,
            ProbeError::NotJellyfin => Self::NotJellyfin,
            ProbeError::Unavailable => Self::Unavailable,
            ProbeError::Redirected { status, .. } => Self::Redirected { status: *status },
            ProbeError::Status { status } => Self::UnexpectedStatus { status: *status },
            ProbeError::TooOld { version } => Self::TooOld {
                version: version.clone(),
            },
            ProbeError::SetupIncomplete => Self::SetupIncomplete,
            ProbeError::DifferentServer => Self::DifferentServer,
            // The realm stays in the core: only the add-a-server and gateway sign-in states show it.
            ProbeError::GatewayPassword { .. } => Self::Gateway {
                answer: GatewayProblem::Password,
            },
            ProbeError::GatewaySignIn => Self::Gateway {
                answer: GatewayProblem::SignIn,
            },
            ProbeError::GatewayRefused => Self::Gateway {
                answer: GatewayProblem::Refused,
            },
            ProbeError::UnsupportedChallenge => Self::Gateway {
                answer: GatewayProblem::Unsupported,
            },
        }
    }
}

impl From<&NetError> for NetProblem {
    fn from(error: &NetError) -> Self {
        match error {
            NetError::Unreachable => Self::Unreachable,
            NetError::Timeout => Self::Timeout,
            NetError::Tls => Self::Tls,
            NetError::Auth { status, .. } => Self::Auth { status: *status },
            NetError::Status { status, .. } => Self::UnexpectedStatus { status: *status },
            NetError::Redirected { status, .. } => Self::Redirected { status: *status },
            NetError::Decode { .. } => Self::Decode,
            NetError::InvalidRequest => Self::InvalidRequest,
            // Only a playlist check limits a body so far; for the UI it is an answer that could not be read.
            NetError::TooLarge => Self::Decode,
        }
    }
}

impl From<&StoreError> for StoreProblem {
    fn from(error: &StoreError) -> Self {
        match error {
            StoreError::Io { op, kind } => Self::Io {
                step: (*op).to_owned(),
                kind: kind.to_string(),
            },
            StoreError::NewerVersion { found } => Self::NewerVersion { found: *found },
            StoreError::Random => Self::Random,
        }
    }
}

impl From<&AuthError> for SignInProblem {
    fn from(error: &AuthError) -> Self {
        match error {
            AuthError::WrongCredentials => Self::WrongCredentials,
            AuthError::Refused => Self::Refused,
            AuthError::QuickConnectDisabled => Self::QuickConnectDisabled,
            AuthError::QuickConnectExpired => Self::QuickConnectExpired,
            AuthError::SessionRevoked => Self::SessionRevoked,
            AuthError::NotSignedIn => Self::NotSignedIn,
            AuthError::Unconfirmed => Self::Unconfirmed,
            // Mid-session only, never from a sign-in: the server refused one resource to a user it accepts.
            AuthError::NotAvailable => Self::Refused,
            AuthError::Network(error) => Self::Network {
                problem: error.into(),
            },
            AuthError::Gateway(answer) => Self::Gateway {
                answer: answer.into(),
            },
            AuthError::EmbyHeaderIgnored => Self::EmbyHeaderIgnored,
        }
    }
}

impl From<&SignInError> for SignInProblem {
    fn from(error: &SignInError) -> Self {
        match error {
            SignInError::Auth(error) => error.into(),
            SignInError::Probe(error) => Self::Probe {
                problem: error.into(),
            },
            SignInError::Save(error) => Self::Save {
                problem: error.into(),
            },
            SignInError::ServerRemoved => Self::ServerRemoved,
        }
    }
}

impl From<&GatewayAnswer> for GatewayProblem {
    fn from(answer: &GatewayAnswer) -> Self {
        match answer {
            GatewayAnswer::Password { .. } => Self::Password,
            GatewayAnswer::SignIn => Self::SignIn,
            GatewayAnswer::Refused => Self::Refused,
            GatewayAnswer::UnsupportedChallenge => Self::Unsupported,
        }
    }
}

impl GatewayInputProblem {
    /// The problem for `error`; `None` for [`GatewaySubmitError::NotAsked`], which is not about the input.
    pub fn of(error: &GatewaySubmitError) -> Option<Self> {
        Some(match error {
            GatewaySubmitError::NotAsked => return None,
            GatewaySubmitError::PlainHttpNotConfirmed => Self::PlainHttpNotConfirmed,
            GatewaySubmitError::NoHeaders => Self::NoHeaders,
            GatewaySubmitError::Invalid(error) => match error {
                GatewayError::InvalidUserName => Self::InvalidUserName,
                GatewayError::InvalidPassword => Self::InvalidPassword,
                GatewayError::InvalidHeaderName => Self::InvalidHeaderName,
                GatewayError::ReservedHeader => Self::ReservedHeader,
                GatewayError::InvalidHeaderValue => Self::InvalidHeaderValue,
                GatewayError::InvalidCookie => Self::InvalidCookie,
            },
        })
    }
}

impl From<&GatewaySignInError> for GatewaySignInProblem {
    fn from(error: &GatewaySignInError) -> Self {
        match error {
            GatewaySignInError::Probe(error) => Self::Probe {
                problem: error.into(),
            },
            GatewaySignInError::Save(error) => Self::Save {
                problem: error.into(),
            },
            GatewaySignInError::ServerRemoved => Self::ServerRemoved,
        }
    }
}

impl From<&DiscoveryError> for DiscoveryProblem {
    fn from(error: &DiscoveryError) -> Self {
        match error {
            DiscoveryError::NotPermitted => Self::NotPermitted,
            DiscoveryError::NoNetwork => Self::NoNetwork,
            DiscoveryError::Failed => Self::Failed,
        }
    }
}

impl From<&AuthError> for BrowseProblem {
    fn from(error: &AuthError) -> Self {
        match error {
            AuthError::NotSignedIn => Self::NotSignedIn,
            AuthError::SessionRevoked => Self::SessionRevoked,
            // The others are answers to a sign-in, which a browse request never gets: a refusal is the closest.
            AuthError::Refused
            | AuthError::WrongCredentials
            | AuthError::QuickConnectDisabled
            | AuthError::QuickConnectExpired
            | AuthError::EmbyHeaderIgnored => Self::Refused,
            AuthError::Unconfirmed => Self::Unconfirmed,
            AuthError::NotAvailable => Self::NotAvailable,
            AuthError::Gateway(answer) => Self::Gateway {
                answer: answer.into(),
            },
            AuthError::Network(error) => Self::Network {
                problem: error.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_keeps_only_its_status() {
        assert_eq!(
            ProbeProblem::from(&ProbeError::Redirected {
                status: 302,
                location: None
            }),
            ProbeProblem::Redirected { status: 302 }
        );
    }

    #[test]
    fn sign_in_errors_flatten() {
        assert_eq!(
            SignInProblem::from(&SignInError::Auth(AuthError::Network(NetError::Auth {
                status: 403,
                message: Some("a server message".into()),
                challenges: Vec::new(),
                page: false,
            }))),
            SignInProblem::Network {
                problem: NetProblem::Auth { status: 403 }
            }
        );
        assert_eq!(
            SignInProblem::from(&SignInError::Save(StoreError::NewerVersion { found: 9 })),
            SignInProblem::Save {
                problem: StoreProblem::NewerVersion { found: 9 }
            }
        );
    }

    #[test]
    fn gateway_problems_carry_no_realm() {
        let realm = current_jellyfin::Realm::new("Private Realm Text");
        let probe = ProbeProblem::from(&ProbeError::GatewayPassword {
            realm: realm.clone(),
        });
        assert_eq!(
            probe,
            ProbeProblem::Gateway {
                answer: GatewayProblem::Password
            }
        );
        let sign_in = SignInProblem::from(&AuthError::Gateway(GatewayAnswer::Password { realm }));
        assert_eq!(
            sign_in,
            SignInProblem::Gateway {
                answer: GatewayProblem::Password
            }
        );
        assert!(!format!("{probe:?} {sign_in:?}").contains("Private"));
        assert_eq!(
            SignInProblem::from(&AuthError::EmbyHeaderIgnored),
            SignInProblem::EmbyHeaderIgnored
        );
        assert_eq!(
            GatewayInputProblem::of(&GatewaySubmitError::Invalid(GatewayError::ReservedHeader)),
            Some(GatewayInputProblem::ReservedHeader)
        );
        assert_eq!(GatewayInputProblem::of(&GatewaySubmitError::NotAsked), None);
    }

    #[test]
    fn browse_problems_keep_the_gateway_and_the_network() {
        assert_eq!(
            BrowseProblem::from(&AuthError::WrongCredentials),
            BrowseProblem::Refused
        );
        assert_eq!(
            BrowseProblem::from(&AuthError::Network(NetError::Timeout)),
            BrowseProblem::Network {
                problem: NetProblem::Timeout
            }
        );
    }
}
