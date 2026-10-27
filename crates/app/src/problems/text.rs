//! The problems in plain words, the same on every screen (docs/notes/windows-shell.md, "Screens"). Never an address, path, token or
//! network key: the problems hold none, and the texts add none. A few words name the platform ("this PC", "Windows"): the UI passes
//! its [`Platform`] once ([`crate::App::set_platform`]).

use super::{
    AddressProblem, BrowseProblem, DiscoveryProblem, GatewayInputProblem, GatewayProblem,
    GatewaySignInProblem, NetProblem, ProbeProblem, SignInProblem, StoreProblem,
};

/// The platform the UI runs on, for the few texts that name it ("this PC", "Windows"). Only Windows's words exist so far: before
/// the UI passes its platform, and on macOS and Linux, the texts use them; macOS's and Linux's words come with their apps (M3,
/// Linux).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    #[default]
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    /// What the texts call this computer: "this PC".
    fn this_computer(self) -> &'static str {
        // macOS's and Linux's words come with their apps (M3, Linux); until then, Windows's.
        match self {
            Self::Windows | Self::MacOs | Self::Linux => "this PC",
        }
    }

    /// The system's name, as in "Windows didn't allow Current to search the network.".
    fn system(self) -> &'static str {
        match self {
            Self::Windows | Self::MacOs | Self::Linux => "Windows",
        }
    }
}

pub const UNEXPECTED: &str = "Something went wrong in Current. Try again.";

/// A gateway that asks for a sign-in method Current does not support (ADR 0017).
pub const GATEWAY_UNSUPPORTED: &str =
    "The server asks for a sign-in method Current does not support.";

/// The add-server page's text for a web sign-in that is not a redirect (its gateway sign-in state).
pub const GATEWAY_WEB_PAGE: &str = "This address shows a web page. If it is your server's sign-in, sign in; otherwise check the address.";

/// A gateway that refuses without a sign-in (the add-server page's gateway refused state): extra headers are the way in.
pub const GATEWAY_REFUSED: &str = "This address refuses Current without a sign-in. If your gateway gave you a service token or \
                                   another secret header, add it under Advanced.";

/// The sign-in window's text in place of Google's sign-in, which refuses apps' web views (ADR 0017, "Google"). The web view is never
/// disguised as a browser.
pub const GOOGLE_REFUSES_WEB_VIEWS: &str = "Google doesn't allow signing in inside apps. Use another sign-in method of your gateway \
                                            (for example a one-time code), or a service token.";

/// The sign-in's client details sent in `X-Emby-Authorization` were ignored ([`SignInProblem::EmbyHeaderIgnored`]): the two
/// server-side fixes from docs/notes/gateway-auth.md, "Which headers carry the token".
pub const EMBY_HEADER_IGNORED: &str = "The server ignored Current's sign-in because the gateway in front of it uses the \
    Authorization header for its own password. Change one thing on the server side: turn on EnableLegacyAuthorization in the \
    server's system.xml and set the gateway not to pass its own Authorization header on (in Nginx Proxy Manager, leave \"Pass Auth \
    to Upstream\" off); or add a gateway rule that lets the server's API through without the password.";

const NEWER_SETTINGS: &str = "These settings are from a newer version of Current, so this sign-in can't be saved. Update Current.";
const SAVE_FAILED: &str = "Current couldn't save the sign-in to its settings folder.";
const SERVER_REMOVED: &str = "This server was removed.";
const UNCONFIRMED: &str = "The server's answer is being checked. Try again in a moment.";

/// What a gateway in front of the address wants, as setup says it.
pub fn gateway_problem_text(problem: GatewayProblem) -> String {
    match problem {
        GatewayProblem::Password => {
            "This address asks for a password of its own before the server."
        }
        GatewayProblem::SignIn => "This address shows a sign-in page before the server.",
        GatewayProblem::Refused => "This address refuses Current without a sign-in.",
        GatewayProblem::Unsupported => GATEWAY_UNSUPPORTED,
    }
    .to_owned()
}

/// Why the gateway credentials were not used: nothing was sent.
pub fn gateway_input_problem_text(problem: GatewayInputProblem) -> String {
    match problem {
        GatewayInputProblem::PlainHttpNotConfirmed => "Confirm the connection without encryption first.",
        GatewayInputProblem::NoHeaders => "Add at least one header.",
        GatewayInputProblem::InvalidUserName => "The user name can't contain a colon or control characters.",
        GatewayInputProblem::InvalidPassword => "The password can't contain control characters.",
        GatewayInputProblem::InvalidHeaderName => {
            "A header name can only have letters, digits and the characters ! # $ % & ' * + - . ^ _ ` | ~."
        }
        GatewayInputProblem::ReservedHeader => {
            "Current sets Authorization, Cookie, Host, Content-Length and the other connection headers itself, so they can't be \
             used here."
        }
        GatewayInputProblem::InvalidHeaderValue => {
            "A header value can only have visible ASCII characters and spaces."
        }
        GatewayInputProblem::InvalidCookie => "The sign-in page's cookies couldn't be used.",
    }
    .to_owned()
}

/// Why setup found no usable server at an address.
pub fn probe_problem_text(problem: &ProbeProblem, platform: Platform) -> String {
    match problem {
        ProbeProblem::InvalidAddress { reason } => match reason {
            AddressProblem::Empty => "Enter the server's address.",
            AddressProblem::UnsupportedScheme => {
                "Use an address that starts with http:// or https://."
            }
            AddressProblem::Credentials => {
                "Leave the user name and password out of the address. You'll sign in next."
            }
            AddressProblem::Invalid => "That isn't a valid address.",
        }
        .to_owned(),
        ProbeProblem::Unreachable => {
            "Nothing answered at that address. Check the address and that the server is on.".to_owned()
        }
        ProbeProblem::Timeout => "The server took too long to answer.".to_owned(),
        ProbeProblem::Tls => tls_text(platform),
        ProbeProblem::NotJellyfin => "That address doesn't lead to a Jellyfin server.".to_owned(),
        ProbeProblem::Unavailable => {
            "The server is starting or doesn't accept connections from this network. Try again in a moment.".to_owned()
        }
        ProbeProblem::Redirected { status } => {
            format!("The server sent Current to another address (HTTP {status}).")
        }
        ProbeProblem::UnexpectedStatus { status } => {
            format!("The server answered with an unexpected error (HTTP {status}).")
        }
        ProbeProblem::TooOld { version } => {
            format!("This server runs Jellyfin {version}, which Current doesn't support. Update the server.")
        }
        ProbeProblem::SetupIncomplete => {
            "This server hasn't finished its first-time setup. Finish it in the server's web page.".to_owned()
        }
        ProbeProblem::DifferentServer => "That address now leads to a different server.".to_owned(),
        ProbeProblem::Gateway { answer } => gateway_problem_text(*answer),
    }
}

fn tls_text(platform: Platform) -> String {
    format!(
        "The secure connection failed. The server's certificate may not be trusted by {}.",
        platform.this_computer()
    )
}

/// Why a sign-in failed.
pub fn sign_in_problem_text(problem: &SignInProblem, platform: Platform) -> String {
    match problem {
        SignInProblem::WrongCredentials => "The user name or password is incorrect.".to_owned(),
        SignInProblem::Refused => {
            "The server refused this user. Ask the server's administrator.".to_owned()
        }
        SignInProblem::QuickConnectDisabled => {
            "Quick Connect is turned off on this server.".to_owned()
        }
        SignInProblem::QuickConnectExpired => "The code expired. Try again for a new one.".to_owned(),
        SignInProblem::SessionRevoked | SignInProblem::NotSignedIn => {
            "The server ended the session. Sign in again.".to_owned()
        }
        SignInProblem::Network { problem } => net_problem_text(*problem, platform),
        SignInProblem::Probe { problem } => probe_problem_text(problem, platform),
        SignInProblem::Save { problem } => save_text(problem),
        SignInProblem::ServerRemoved => SERVER_REMOVED.to_owned(),
        SignInProblem::Gateway { answer } => match answer {
            GatewayProblem::Password => {
                "The gateway in front of the server asks for its own password first."
            }
            GatewayProblem::SignIn => {
                "The gateway in front of the server wants you to sign in on its page first."
            }
            GatewayProblem::Refused => {
                "The gateway in front of the server refuses Current. If it gave you a service token or another secret header, add \
                 it as extra headers."
            }
            GatewayProblem::Unsupported => GATEWAY_UNSUPPORTED,
        }
        .to_owned(),
        SignInProblem::EmbyHeaderIgnored => EMBY_HEADER_IGNORED.to_owned(),
        SignInProblem::Unconfirmed => UNCONFIRMED.to_owned(),
    }
}

fn save_text(problem: &StoreProblem) -> String {
    match problem {
        StoreProblem::NewerVersion { .. } => NEWER_SETTINGS,
        StoreProblem::Io { .. } | StoreProblem::Random => SAVE_FAILED,
    }
    .to_owned()
}

/// Why signing in to a saved server's gateway again failed.
pub fn gateway_sign_in_problem_text(problem: &GatewaySignInProblem, platform: Platform) -> String {
    match problem {
        GatewaySignInProblem::Probe { problem } => probe_problem_text(problem, platform),
        GatewaySignInProblem::Save { problem } => save_text(problem),
        GatewaySignInProblem::ServerRemoved => SERVER_REMOVED.to_owned(),
    }
}

/// Why a request failed. The status is the only thing of the server's answer it says.
pub fn net_problem_text(problem: NetProblem, platform: Platform) -> String {
    match problem {
        NetProblem::Unreachable => "Can't reach the server.".to_owned(),
        NetProblem::Timeout => "The server took too long to answer.".to_owned(),
        NetProblem::Tls => tls_text(platform),
        NetProblem::Auth { status }
        | NetProblem::UnexpectedStatus { status }
        | NetProblem::Redirected { status } => {
            format!("The server answered with an error (HTTP {status}).")
        }
        NetProblem::Decode => "The server's answer couldn't be read.".to_owned(),
        NetProblem::InvalidRequest => UNEXPECTED.to_owned(),
    }
}

/// Why the search for servers on the local network failed.
pub fn discovery_problem_text(problem: DiscoveryProblem, platform: Platform) -> String {
    match problem {
        DiscoveryProblem::NotPermitted => format!(
            "{} didn't allow Current to search the network.",
            platform.system()
        ),
        DiscoveryProblem::NoNetwork => "You're not connected to a network.".to_owned(),
        DiscoveryProblem::Failed => "The search didn't work. Enter the address instead.".to_owned(),
    }
}

/// The step and kind of a store error. No path: it holds the user's account name.
pub fn store_problem_text(problem: &StoreProblem) -> String {
    match problem {
        StoreProblem::Io { step, kind } => format!("Step: {step}. Error: {kind}."),
        StoreProblem::NewerVersion { found } => {
            format!("The settings are from a newer version of Current (format {found}).")
        }
        StoreProblem::Random => "No secure random numbers were available.".to_owned(),
    }
}

/// Why a browse request failed.
pub fn browse_problem_text(problem: &BrowseProblem, platform: Platform) -> String {
    match problem {
        BrowseProblem::NotSignedIn => "You're no longer signed in to this server.".to_owned(),
        BrowseProblem::SessionRevoked => {
            "The server ended the session. Sign in again in Settings.".to_owned()
        }
        BrowseProblem::Refused => "The server refused it.".to_owned(),
        BrowseProblem::Unconfirmed => UNCONFIRMED.to_owned(),
        BrowseProblem::NotAvailable => {
            "It's no longer available to you on this server.".to_owned()
        }
        BrowseProblem::Gateway { .. } => {
            "The gateway in front of the server wants you to sign in again. Sign in to the gateway in Settings.".to_owned()
        }
        BrowseProblem::Network { problem } => net_problem_text(*problem, platform),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Platform = Platform::Windows;

    #[test]
    fn probe_problems_are_worded() {
        let text = |problem: ProbeProblem| probe_problem_text(&problem, WIN);
        assert_eq!(
            text(ProbeProblem::InvalidAddress {
                reason: AddressProblem::Credentials
            }),
            "Leave the user name and password out of the address. You'll sign in next."
        );
        assert_eq!(
            text(ProbeProblem::Tls),
            "The secure connection failed. The server's certificate may not be trusted by this PC."
        );
        assert_eq!(
            text(ProbeProblem::Redirected { status: 302 }),
            "The server sent Current to another address (HTTP 302)."
        );
        assert_eq!(
            text(ProbeProblem::TooOld {
                version: "10.8.0".into()
            }),
            "This server runs Jellyfin 10.8.0, which Current doesn't support. Update the server."
        );
        assert_eq!(
            text(ProbeProblem::Gateway {
                answer: GatewayProblem::Unsupported
            }),
            GATEWAY_UNSUPPORTED
        );
        assert_eq!(
            text(ProbeProblem::Gateway {
                answer: GatewayProblem::Password
            }),
            "This address asks for a password of its own before the server."
        );
    }

    #[test]
    fn sign_in_problems_are_worded() {
        let text = |problem: SignInProblem| sign_in_problem_text(&problem, WIN);
        assert_eq!(
            text(SignInProblem::NotSignedIn),
            "The server ended the session. Sign in again."
        );
        assert_eq!(
            text(SignInProblem::Save {
                problem: StoreProblem::NewerVersion { found: 3 }
            }),
            "These settings are from a newer version of Current, so this sign-in can't be saved. Update Current."
        );
        assert_eq!(
            text(SignInProblem::Save {
                problem: StoreProblem::Random
            }),
            "Current couldn't save the sign-in to its settings folder."
        );
        assert_eq!(
            text(SignInProblem::Gateway {
                answer: GatewayProblem::Refused
            }),
            "The gateway in front of the server refuses Current. If it gave you a service token or another secret header, add it \
             as extra headers."
        );
        assert_eq!(
            text(SignInProblem::Network {
                problem: NetProblem::Auth { status: 403 }
            }),
            "The server answered with an error (HTTP 403)."
        );
        assert!(text(SignInProblem::EmbyHeaderIgnored).contains(
            "(in Nginx Proxy Manager, leave \"Pass Auth to Upstream\" off); or add a gateway rule"
        ));
        assert_eq!(
            gateway_sign_in_problem_text(&GatewaySignInProblem::ServerRemoved, WIN),
            "This server was removed."
        );
    }

    #[test]
    fn the_long_texts_join_their_lines_with_one_space() {
        for text in [
            GATEWAY_REFUSED,
            GOOGLE_REFUSES_WEB_VIEWS,
            EMBY_HEADER_IGNORED,
            &gateway_input_problem_text(GatewayInputProblem::ReservedHeader),
            &sign_in_problem_text(
                &SignInProblem::Gateway {
                    answer: GatewayProblem::Refused,
                },
                WIN,
            ),
        ] {
            assert!(!text.contains("  ") && !text.contains('\n'), "{text}");
        }
        assert_eq!(
            GATEWAY_REFUSED,
            "This address refuses Current without a sign-in. If your gateway gave you a service token or another secret header, \
             add it under Advanced."
        );
    }

    #[test]
    fn other_problems_are_worded() {
        assert_eq!(
            gateway_input_problem_text(GatewayInputProblem::InvalidHeaderName),
            "A header name can only have letters, digits and the characters ! # $ % & ' * + - . ^ _ ` | ~."
        );
        assert_eq!(
            net_problem_text(NetProblem::InvalidRequest, WIN),
            UNEXPECTED
        );
        assert_eq!(
            discovery_problem_text(DiscoveryProblem::NotPermitted, WIN),
            "Windows didn't allow Current to search the network."
        );
        assert_eq!(
            store_problem_text(&StoreProblem::Io {
                step: "save".into(),
                kind: "permission denied".into()
            }),
            "Step: save. Error: permission denied."
        );
        assert_eq!(
            store_problem_text(&StoreProblem::NewerVersion { found: 4 }),
            "The settings are from a newer version of Current (format 4)."
        );
        assert_eq!(
            browse_problem_text(
                &BrowseProblem::Network {
                    problem: NetProblem::Unreachable
                },
                WIN
            ),
            "Can't reach the server."
        );
        assert_eq!(
            browse_problem_text(&BrowseProblem::Unconfirmed, WIN),
            "The server's answer is being checked. Try again in a moment."
        );
    }

    #[test]
    fn every_platform_uses_windowss_words_for_now() {
        for platform in [Platform::MacOs, Platform::Linux] {
            assert_eq!(
                net_problem_text(NetProblem::Tls, platform),
                net_problem_text(NetProblem::Tls, WIN)
            );
            assert_eq!(
                discovery_problem_text(DiscoveryProblem::NotPermitted, platform),
                discovery_problem_text(DiscoveryProblem::NotPermitted, WIN)
            );
        }
        assert_eq!(Platform::default(), WIN);
    }
}
