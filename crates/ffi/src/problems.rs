//! Why a flow failed, as the UI acts on it: `current-app`'s problems (flat copies of the core's error enums with nothing a user
//! should not see: no address, no redirect target, no server message, no OS error text; ADR 0006), copied one to one for the bridge,
//! and their texts.

use current_app as app;
use current_app::{GatewaySignInError, GatewaySubmitError, SignInError};
use current_jellyfin::{AddressError, AuthError, DiscoveryError, GatewayAnswer, ProbeError};
use current_net::NetError;
use current_store::StoreError;

use crate::Core;

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
    /// An authentication gateway in front of the server wants something (ADR 0017). The flow's gateway states say what to do;
    /// this problem appears where no flow can ask, e.g. a saved address checked before a sign-in.
    Gateway {
        answer: GatewayProblem,
    },
}

/// What an authentication gateway in front of the server wants (ADR 0017). Never its realm or any other text it sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum GatewaySignInProblem {
    /// The address could not be checked: not reached, another server, or not a usable server.
    Probe { problem: ProbeProblem },
    /// The new credentials could not be saved.
    Save { problem: StoreProblem },
    /// The server was removed meanwhile.
    ServerRemoved,
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
    /// An authentication gateway in front of the server answered instead (ADR 0017): sign in to it first.
    Gateway {
        answer: GatewayProblem,
    },
    /// The server ignored the sign-in's client details sent in `X-Emby-Authorization` (ADR 0017): it reads that header only with
    /// legacy authorization on. The UI explains the two server-side fixes: turn on legacy authorization on the server and stop the
    /// gateway passing its own `Authorization` header on; or add a gateway rule that lets the server's API through.
    EmbyHeaderIgnored,
    /// The server's answer is being checked (mid-session only, never from a sign-in): try again in a moment.
    Unconfirmed,
}

/// Why the search for local servers failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DiscoveryProblem {
    /// The system refused, e.g. the Local Network privilege on Apple platforms.
    NotPermitted,
    NoNetwork,
    Failed,
}

/// From the core's errors through `current-app`'s problems, which hold the rules.
macro_rules! from_core_error {
    ($($error:ty => $problem:ident),* $(,)?) => {$(
        impl From<&$error> for $problem {
            fn from(error: &$error) -> Self {
                app::$problem::from(error).into()
            }
        }
    )*};
}

from_core_error! {
    AddressError => AddressProblem,
    ProbeError => ProbeProblem,
    NetError => NetProblem,
    StoreError => StoreProblem,
    AuthError => SignInProblem,
    SignInError => SignInProblem,
    GatewayAnswer => GatewayProblem,
    GatewaySignInError => GatewaySignInProblem,
    DiscoveryError => DiscoveryProblem,
}

impl GatewayInputProblem {
    /// The problem for `error`; `None` for [`GatewaySubmitError::NotAsked`], which is [`crate::CoreError::NotReady`].
    pub(crate) fn of(error: &GatewaySubmitError) -> Option<Self> {
        app::GatewayInputProblem::of(error).map(Into::into)
    }
}

// One to one, both ways: every match is exhaustive, so a variant added on one side and not the other fails to compile.

impl From<app::AddressProblem> for AddressProblem {
    fn from(problem: app::AddressProblem) -> Self {
        match problem {
            app::AddressProblem::Empty => Self::Empty,
            app::AddressProblem::UnsupportedScheme => Self::UnsupportedScheme,
            app::AddressProblem::Credentials => Self::Credentials,
            app::AddressProblem::Invalid => Self::Invalid,
        }
    }
}

impl From<AddressProblem> for app::AddressProblem {
    fn from(problem: AddressProblem) -> Self {
        match problem {
            AddressProblem::Empty => Self::Empty,
            AddressProblem::UnsupportedScheme => Self::UnsupportedScheme,
            AddressProblem::Credentials => Self::Credentials,
            AddressProblem::Invalid => Self::Invalid,
        }
    }
}

impl From<app::ProbeProblem> for ProbeProblem {
    fn from(problem: app::ProbeProblem) -> Self {
        use app::ProbeProblem as P;
        match problem {
            P::InvalidAddress { reason } => Self::InvalidAddress {
                reason: reason.into(),
            },
            P::Unreachable => Self::Unreachable,
            P::Timeout => Self::Timeout,
            P::Tls => Self::Tls,
            P::NotJellyfin => Self::NotJellyfin,
            P::Unavailable => Self::Unavailable,
            P::Redirected { status } => Self::Redirected { status },
            P::UnexpectedStatus { status } => Self::UnexpectedStatus { status },
            P::TooOld { version } => Self::TooOld { version },
            P::SetupIncomplete => Self::SetupIncomplete,
            P::DifferentServer => Self::DifferentServer,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
        }
    }
}

impl From<ProbeProblem> for app::ProbeProblem {
    fn from(problem: ProbeProblem) -> Self {
        use ProbeProblem as P;
        match problem {
            P::InvalidAddress { reason } => Self::InvalidAddress {
                reason: reason.into(),
            },
            P::Unreachable => Self::Unreachable,
            P::Timeout => Self::Timeout,
            P::Tls => Self::Tls,
            P::NotJellyfin => Self::NotJellyfin,
            P::Unavailable => Self::Unavailable,
            P::Redirected { status } => Self::Redirected { status },
            P::UnexpectedStatus { status } => Self::UnexpectedStatus { status },
            P::TooOld { version } => Self::TooOld { version },
            P::SetupIncomplete => Self::SetupIncomplete,
            P::DifferentServer => Self::DifferentServer,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
        }
    }
}

impl From<app::GatewayProblem> for GatewayProblem {
    fn from(problem: app::GatewayProblem) -> Self {
        match problem {
            app::GatewayProblem::Password => Self::Password,
            app::GatewayProblem::SignIn => Self::SignIn,
            app::GatewayProblem::Refused => Self::Refused,
            app::GatewayProblem::Unsupported => Self::Unsupported,
        }
    }
}

impl From<GatewayProblem> for app::GatewayProblem {
    fn from(problem: GatewayProblem) -> Self {
        match problem {
            GatewayProblem::Password => Self::Password,
            GatewayProblem::SignIn => Self::SignIn,
            GatewayProblem::Refused => Self::Refused,
            GatewayProblem::Unsupported => Self::Unsupported,
        }
    }
}

impl From<app::GatewayInputProblem> for GatewayInputProblem {
    fn from(problem: app::GatewayInputProblem) -> Self {
        use app::GatewayInputProblem as P;
        match problem {
            P::PlainHttpNotConfirmed => Self::PlainHttpNotConfirmed,
            P::NoHeaders => Self::NoHeaders,
            P::InvalidUserName => Self::InvalidUserName,
            P::InvalidPassword => Self::InvalidPassword,
            P::InvalidHeaderName => Self::InvalidHeaderName,
            P::ReservedHeader => Self::ReservedHeader,
            P::InvalidHeaderValue => Self::InvalidHeaderValue,
            P::InvalidCookie => Self::InvalidCookie,
        }
    }
}

impl From<GatewayInputProblem> for app::GatewayInputProblem {
    fn from(problem: GatewayInputProblem) -> Self {
        use GatewayInputProblem as P;
        match problem {
            P::PlainHttpNotConfirmed => Self::PlainHttpNotConfirmed,
            P::NoHeaders => Self::NoHeaders,
            P::InvalidUserName => Self::InvalidUserName,
            P::InvalidPassword => Self::InvalidPassword,
            P::InvalidHeaderName => Self::InvalidHeaderName,
            P::ReservedHeader => Self::ReservedHeader,
            P::InvalidHeaderValue => Self::InvalidHeaderValue,
            P::InvalidCookie => Self::InvalidCookie,
        }
    }
}

impl From<app::GatewaySignInProblem> for GatewaySignInProblem {
    fn from(problem: app::GatewaySignInProblem) -> Self {
        match problem {
            app::GatewaySignInProblem::Probe { problem } => Self::Probe {
                problem: problem.into(),
            },
            app::GatewaySignInProblem::Save { problem } => Self::Save {
                problem: problem.into(),
            },
            app::GatewaySignInProblem::ServerRemoved => Self::ServerRemoved,
        }
    }
}

impl From<GatewaySignInProblem> for app::GatewaySignInProblem {
    fn from(problem: GatewaySignInProblem) -> Self {
        match problem {
            GatewaySignInProblem::Probe { problem } => Self::Probe {
                problem: problem.into(),
            },
            GatewaySignInProblem::Save { problem } => Self::Save {
                problem: problem.into(),
            },
            GatewaySignInProblem::ServerRemoved => Self::ServerRemoved,
        }
    }
}

impl From<app::NetProblem> for NetProblem {
    fn from(problem: app::NetProblem) -> Self {
        use app::NetProblem as P;
        match problem {
            P::Unreachable => Self::Unreachable,
            P::Timeout => Self::Timeout,
            P::Tls => Self::Tls,
            P::Auth { status } => Self::Auth { status },
            P::UnexpectedStatus { status } => Self::UnexpectedStatus { status },
            P::Redirected { status } => Self::Redirected { status },
            P::Decode => Self::Decode,
            P::InvalidRequest => Self::InvalidRequest,
        }
    }
}

impl From<NetProblem> for app::NetProblem {
    fn from(problem: NetProblem) -> Self {
        use NetProblem as P;
        match problem {
            P::Unreachable => Self::Unreachable,
            P::Timeout => Self::Timeout,
            P::Tls => Self::Tls,
            P::Auth { status } => Self::Auth { status },
            P::UnexpectedStatus { status } => Self::UnexpectedStatus { status },
            P::Redirected { status } => Self::Redirected { status },
            P::Decode => Self::Decode,
            P::InvalidRequest => Self::InvalidRequest,
        }
    }
}

impl From<app::StoreProblem> for StoreProblem {
    fn from(problem: app::StoreProblem) -> Self {
        match problem {
            app::StoreProblem::Io { step, kind } => Self::Io { step, kind },
            app::StoreProblem::NewerVersion { found } => Self::NewerVersion { found },
            app::StoreProblem::Random => Self::Random,
        }
    }
}

impl From<StoreProblem> for app::StoreProblem {
    fn from(problem: StoreProblem) -> Self {
        match problem {
            StoreProblem::Io { step, kind } => Self::Io { step, kind },
            StoreProblem::NewerVersion { found } => Self::NewerVersion { found },
            StoreProblem::Random => Self::Random,
        }
    }
}

impl From<app::SignInProblem> for SignInProblem {
    fn from(problem: app::SignInProblem) -> Self {
        use app::SignInProblem as P;
        match problem {
            P::WrongCredentials => Self::WrongCredentials,
            P::Refused => Self::Refused,
            P::QuickConnectDisabled => Self::QuickConnectDisabled,
            P::QuickConnectExpired => Self::QuickConnectExpired,
            P::SessionRevoked => Self::SessionRevoked,
            P::NotSignedIn => Self::NotSignedIn,
            P::Network { problem } => Self::Network {
                problem: problem.into(),
            },
            P::Probe { problem } => Self::Probe {
                problem: problem.into(),
            },
            P::Save { problem } => Self::Save {
                problem: problem.into(),
            },
            P::ServerRemoved => Self::ServerRemoved,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
            P::EmbyHeaderIgnored => Self::EmbyHeaderIgnored,
            P::Unconfirmed => Self::Unconfirmed,
        }
    }
}

impl From<SignInProblem> for app::SignInProblem {
    fn from(problem: SignInProblem) -> Self {
        use SignInProblem as P;
        match problem {
            P::WrongCredentials => Self::WrongCredentials,
            P::Refused => Self::Refused,
            P::QuickConnectDisabled => Self::QuickConnectDisabled,
            P::QuickConnectExpired => Self::QuickConnectExpired,
            P::SessionRevoked => Self::SessionRevoked,
            P::NotSignedIn => Self::NotSignedIn,
            P::Network { problem } => Self::Network {
                problem: problem.into(),
            },
            P::Probe { problem } => Self::Probe {
                problem: problem.into(),
            },
            P::Save { problem } => Self::Save {
                problem: problem.into(),
            },
            P::ServerRemoved => Self::ServerRemoved,
            P::Gateway { answer } => Self::Gateway {
                answer: answer.into(),
            },
            P::EmbyHeaderIgnored => Self::EmbyHeaderIgnored,
            P::Unconfirmed => Self::Unconfirmed,
        }
    }
}

impl From<app::DiscoveryProblem> for DiscoveryProblem {
    fn from(problem: app::DiscoveryProblem) -> Self {
        match problem {
            app::DiscoveryProblem::NotPermitted => Self::NotPermitted,
            app::DiscoveryProblem::NoNetwork => Self::NoNetwork,
            app::DiscoveryProblem::Failed => Self::Failed,
        }
    }
}

impl From<DiscoveryProblem> for app::DiscoveryProblem {
    fn from(problem: DiscoveryProblem) -> Self {
        match problem {
            DiscoveryProblem::NotPermitted => Self::NotPermitted,
            DiscoveryProblem::NoNetwork => Self::NoNetwork,
            DiscoveryProblem::Failed => Self::Failed,
        }
    }
}

/// The platform the UI runs on, for the few texts that name it ("this PC", "Windows"). Only Windows's words exist so far: before
/// the UI passes its platform, and on macOS and Linux, the texts use them; macOS's and Linux's words come with their apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl From<Platform> for app::Platform {
    fn from(platform: Platform) -> Self {
        match platform {
            Platform::Windows => Self::Windows,
            Platform::MacOs => Self::MacOs,
            Platform::Linux => Self::Linux,
        }
    }
}

/// Something went wrong in Current itself: "Something went wrong in Current. Try again."
#[uniffi::export]
pub fn unexpected_problem_text() -> String {
    app::UNEXPECTED.to_owned()
}

/// A gateway that asks for a sign-in method Current does not support (ADR 0017).
#[uniffi::export]
pub fn gateway_unsupported_text() -> String {
    app::GATEWAY_UNSUPPORTED.to_owned()
}

/// The add-server page's text for a web sign-in that is not a redirect (its gateway sign-in state).
#[uniffi::export]
pub fn gateway_web_page_text() -> String {
    app::GATEWAY_WEB_PAGE.to_owned()
}

/// The add-server page's text for a gateway that refuses without a sign-in: extra headers are the way in.
#[uniffi::export]
pub fn gateway_refused_text() -> String {
    app::GATEWAY_REFUSED.to_owned()
}

/// The sign-in window's text in place of Google's sign-in, which refuses apps' web views (ADR 0017, "Google").
#[uniffi::export]
pub fn google_refuses_web_views_text() -> String {
    app::GOOGLE_REFUSES_WEB_VIEWS.to_owned()
}

/// What a gateway in front of the address wants, as setup says it.
#[uniffi::export]
pub fn gateway_problem_text(problem: GatewayProblem) -> String {
    app::gateway_problem_text(problem.into())
}

/// Why the gateway credentials were not used: nothing was sent.
#[uniffi::export]
pub fn gateway_input_problem_text(problem: GatewayInputProblem) -> String {
    app::gateway_input_problem_text(problem.into())
}

/// The step and kind of a store error. No path: it holds the user's account name.
#[uniffi::export]
pub fn store_problem_text(problem: StoreProblem) -> String {
    app::store_problem_text(&problem.into())
}

#[uniffi::export]
impl Core {
    /// The UI's platform, for the few texts that name it ("this PC", "Windows"): passed once at start, as the decimal separator.
    pub fn set_platform(&self, platform: Platform) {
        self.app.set_platform(platform.into());
    }

    /// Why setup found no usable server at an address, in plain words.
    pub fn probe_problem_text(&self, problem: ProbeProblem) -> String {
        app::probe_problem_text(&problem.into(), self.app.platform())
    }

    /// Why a sign-in failed, in plain words.
    pub fn sign_in_problem_text(&self, problem: SignInProblem) -> String {
        app::sign_in_problem_text(&problem.into(), self.app.platform())
    }

    /// Why signing in to a saved server's gateway again failed, in plain words.
    pub fn gateway_sign_in_problem_text(&self, problem: GatewaySignInProblem) -> String {
        app::gateway_sign_in_problem_text(&problem.into(), self.app.platform())
    }

    /// Why a request failed, in plain words.
    pub fn net_problem_text(&self, problem: NetProblem) -> String {
        app::net_problem_text(problem.into(), self.app.platform())
    }

    /// Why the search for servers on the local network failed, in plain words.
    pub fn discovery_problem_text(&self, problem: DiscoveryProblem) -> String {
        app::discovery_problem_text(problem.into(), self.app.platform())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, through the core's shape and back.
    fn round_trip<F, A>(problem: F)
    where
        F: Clone + std::fmt::Debug + PartialEq + Into<A>,
        A: Into<F>,
    {
        let back: F = problem.clone().into().into();
        assert_eq!(back, problem);
    }

    #[test]
    fn every_problem_converts_both_ways() {
        let addresses = [
            AddressProblem::Empty,
            AddressProblem::UnsupportedScheme,
            AddressProblem::Credentials,
            AddressProblem::Invalid,
        ];
        let gateways = [
            GatewayProblem::Password,
            GatewayProblem::SignIn,
            GatewayProblem::Refused,
            GatewayProblem::Unsupported,
        ];
        let probes: Vec<ProbeProblem> = addresses
            .iter()
            .map(|&reason| ProbeProblem::InvalidAddress { reason })
            .chain(
                gateways
                    .iter()
                    .map(|&answer| ProbeProblem::Gateway { answer }),
            )
            .chain([
                ProbeProblem::Unreachable,
                ProbeProblem::Timeout,
                ProbeProblem::Tls,
                ProbeProblem::NotJellyfin,
                ProbeProblem::Unavailable,
                ProbeProblem::Redirected { status: 302 },
                ProbeProblem::UnexpectedStatus { status: 500 },
                ProbeProblem::TooOld {
                    version: "10.8.0".into(),
                },
                ProbeProblem::SetupIncomplete,
                ProbeProblem::DifferentServer,
            ])
            .collect();
        let nets = [
            NetProblem::Unreachable,
            NetProblem::Timeout,
            NetProblem::Tls,
            NetProblem::Auth { status: 401 },
            NetProblem::UnexpectedStatus { status: 500 },
            NetProblem::Redirected { status: 307 },
            NetProblem::Decode,
            NetProblem::InvalidRequest,
        ];
        let stores = [
            StoreProblem::Io {
                step: "save".into(),
                kind: "other".into(),
            },
            StoreProblem::NewerVersion { found: 2 },
            StoreProblem::Random,
        ];
        for problem in &probes {
            round_trip::<_, app::ProbeProblem>(problem.clone());
            round_trip::<_, app::SignInProblem>(SignInProblem::Probe {
                problem: problem.clone(),
            });
            round_trip::<_, app::GatewaySignInProblem>(GatewaySignInProblem::Probe {
                problem: problem.clone(),
            });
        }
        for &problem in &nets {
            round_trip::<_, app::NetProblem>(problem);
            round_trip::<_, app::SignInProblem>(SignInProblem::Network { problem });
        }
        for problem in &stores {
            round_trip::<_, app::StoreProblem>(problem.clone());
            round_trip::<_, app::SignInProblem>(SignInProblem::Save {
                problem: problem.clone(),
            });
            round_trip::<_, app::GatewaySignInProblem>(GatewaySignInProblem::Save {
                problem: problem.clone(),
            });
        }
        for answer in gateways {
            round_trip::<_, app::GatewayProblem>(answer);
            round_trip::<_, app::SignInProblem>(SignInProblem::Gateway { answer });
        }
        for problem in [
            SignInProblem::WrongCredentials,
            SignInProblem::Refused,
            SignInProblem::QuickConnectDisabled,
            SignInProblem::QuickConnectExpired,
            SignInProblem::SessionRevoked,
            SignInProblem::NotSignedIn,
            SignInProblem::ServerRemoved,
            SignInProblem::EmbyHeaderIgnored,
            SignInProblem::Unconfirmed,
        ] {
            round_trip::<_, app::SignInProblem>(problem);
        }
        round_trip::<_, app::GatewaySignInProblem>(GatewaySignInProblem::ServerRemoved);
        for problem in [
            GatewayInputProblem::PlainHttpNotConfirmed,
            GatewayInputProblem::NoHeaders,
            GatewayInputProblem::InvalidUserName,
            GatewayInputProblem::InvalidPassword,
            GatewayInputProblem::InvalidHeaderName,
            GatewayInputProblem::ReservedHeader,
            GatewayInputProblem::InvalidHeaderValue,
            GatewayInputProblem::InvalidCookie,
        ] {
            round_trip::<_, app::GatewayInputProblem>(problem);
        }
        for problem in [
            DiscoveryProblem::NotPermitted,
            DiscoveryProblem::NoNetwork,
            DiscoveryProblem::Failed,
        ] {
            round_trip::<_, app::DiscoveryProblem>(problem);
        }
    }

    #[test]
    fn core_errors_still_flatten_through_the_core() {
        assert_eq!(
            ProbeProblem::from(&ProbeError::Redirected {
                status: 302,
                location: None
            }),
            ProbeProblem::Redirected { status: 302 }
        );
        assert_eq!(GatewayInputProblem::of(&GatewaySubmitError::NotAsked), None);
    }

    #[test]
    fn the_texts_use_the_platform_passed() {
        let core = Core::new().unwrap();
        core.set_platform(Platform::Windows);
        assert_eq!(
            core.net_problem_text(NetProblem::Tls),
            "The secure connection failed. The server's certificate may not be trusted by this PC."
        );
        assert_eq!(
            core.discovery_problem_text(DiscoveryProblem::NotPermitted),
            "Windows didn't allow Current to search the network."
        );
        assert_eq!(
            core.sign_in_problem_text(SignInProblem::QuickConnectExpired),
            "The code expired. Try again for a new one."
        );
        assert_eq!(
            store_problem_text(StoreProblem::Random),
            "No secure random numbers were available."
        );
        assert_eq!(
            unexpected_problem_text(),
            "Something went wrong in Current. Try again."
        );
    }
}
