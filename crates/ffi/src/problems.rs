//! Why a flow failed, as the UI acts on it. Flat copies of the core's error enums with nothing a user should not see: no address,
//! no redirect target, no server message, no OS error text (ADR 0006).

use current_app::SignInError;
use current_jellyfin::{AddressError, AuthError, DiscoveryError, GatewayAnswer, ProbeError};
use current_net::NetError;
use current_store::StoreError;

/// Why the typed address is not one: the UI asks the user to fix it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AddressProblem {
    Empty,
    /// A scheme other than http or https.
    UnsupportedScheme,
    /// A user name or password in the address.
    Credentials,
    Invalid,
}

/// Why server setup found no usable server at an address.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
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
}

/// Why a request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StoreProblem {
    /// Reading or writing the data folder failed. `step` says which (`read`, `save`...), `kind` the kind of I/O error.
    Io { step: String, kind: String },
    /// The settings belong to a newer version of Current: they are used read-only.
    NewerVersion { found: u64 },
    /// The system's random number generator failed.
    Random,
}

/// Why a sign-in failed.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
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
}

/// Why the search for local servers failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DiscoveryProblem {
    /// The system refused, e.g. the Local Network privilege on Apple platforms.
    NotPermitted,
    NoNetwork,
    Failed,
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
            // The gateway problems get their own ffi variants with the app's gateway states (ADR 0017); until then, the closest.
            ProbeError::GatewayPassword { .. } | ProbeError::UnsupportedChallenge => {
                Self::UnexpectedStatus { status: 401 }
            }
            ProbeError::GatewayRefused => Self::UnexpectedStatus { status: 403 },
            ProbeError::GatewaySignIn => Self::NotJellyfin,
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
            // Mid-session only, never from a sign-in; its own variant comes with the gateway states (ADR 0017).
            AuthError::Unconfirmed => Self::Network {
                problem: NetProblem::Auth { status: 401 },
            },
            AuthError::Network(error) => Self::Network {
                problem: error.into(),
            },
            // The gateway problems get their own ffi variants with the app's gateway states (ADR 0017); until then, the closest.
            AuthError::Gateway(GatewayAnswer::Refused) => Self::Network {
                problem: NetProblem::Auth { status: 403 },
            },
            AuthError::Gateway(_) => Self::Network {
                problem: NetProblem::Auth { status: 401 },
            },
            AuthError::EmbyHeaderIgnored => Self::Network {
                problem: NetProblem::UnexpectedStatus { status: 400 },
            },
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

impl From<&DiscoveryError> for DiscoveryProblem {
    fn from(error: &DiscoveryError) -> Self {
        match error {
            DiscoveryError::NotPermitted => Self::NotPermitted,
            DiscoveryError::NoNetwork => Self::NoNetwork,
            DiscoveryError::Failed => Self::Failed,
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
}
