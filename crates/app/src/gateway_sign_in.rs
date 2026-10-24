//! Signing in to a saved server's gateway again (ADR 0017): the server shows "Sign in to the gateway again" when the gateway in
//! front of the address in use no longer lets requests through. This flow asks for what the gateway wants (its password, a web
//! sign-in, extra headers), probes the address with it, and once the server answers with its own id saves the new credentials for
//! that address: the Jellyfin session and its token were kept and go on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use current_jellyfin::{ProbeError, Prober, Realm};
use current_model::{AddressId, ServerId, ServerRef};
use current_net::{Gateway, GatewayCredentials, Location, Url};
use current_store::StoreError;
use tokio::sync::watch;

use crate::gateway::{self, CookieInput, GatewaySubmitError, HeaderInput};
use crate::session::{Shared, gateway_answered};

/// Where the flow is. The UI renders one screen state per variant. No variant holds a password, a cookie or a header value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewaySignInState {
    /// Asking the address what its gateway wants, or trying the credentials just given.
    Checking,
    /// A password gateway: as `AddServerState::GatewayPassword` (`realm` only as "The site says: ...").
    Password {
        origin: String,
        realm: Option<Realm>,
        wrong: bool,
    },
    /// A web sign-in: the UI opens the sign-in window on `url` and hands its cookies over after each navigation landing on
    /// `origin`, as in setup. `redirected` and `location` as in `AddServerState::GatewaySignIn`.
    WebSignIn {
        url: String,
        origin: String,
        redirected: bool,
        location: Option<Location>,
    },
    /// The gateway refuses without a sign-in: extra headers (advanced).
    Refused { origin: String },
    /// A sign-in method Current does not support.
    Unsupported { origin: String },
    /// The gateway lets the server through again and the credentials are saved. `remembered` is false when the keyring refused
    /// them: they last for this run only.
    Done { remembered: bool },
    /// The address could not be checked: not reached, another server, or the settings could not be saved.
    Failed { error: GatewaySignInError },
}

/// Why signing in to the gateway failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GatewaySignInError {
    #[error(transparent)]
    Probe(#[from] ProbeError),
    #[error(transparent)]
    Save(#[from] StoreError),
    /// The server was removed meanwhile.
    #[error("the server was removed")]
    ServerRemoved,
}

/// The flow's state and actions, from [`App::sign_in_to_gateway`](crate::App::sign_in_to_gateway). A newer action supersedes
/// one still running: its result is dropped.
pub struct GatewaySignIn {
    shared: Arc<Shared>,
    server: ServerRef,
    expected: ServerId,
    address: AddressId,
    base: Url,
    plain_http_confirmed: bool,
    prober: Prober,
    /// The credentials given so far in this flow. Never in a state.
    credentials: Mutex<GatewayCredentials>,
    state: watch::Sender<GatewaySignInState>,
    generation: AtomicU64,
}

impl std::fmt::Debug for GatewaySignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewaySignIn")
            .field("server", &self.server)
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

impl GatewaySignIn {
    pub(crate) fn new(
        shared: Arc<Shared>,
        server: ServerRef,
        expected: ServerId,
        (address, base, plain_http_confirmed): (AddressId, Url, bool),
    ) -> Self {
        let prober = Prober::new(server.clone(), shared.request_log.clone());
        // Starts from the address's credentials in use: a password gateway in front of a cookie portal keeps its password.
        let credentials = shared
            .gateway_of(&server, address)
            .map(|gateway| gateway.snapshot().0)
            .unwrap_or_default();
        Self {
            shared,
            server,
            expected,
            address,
            base,
            plain_http_confirmed,
            prober,
            credentials: Mutex::new(credentials),
            state: watch::Sender::new(GatewaySignInState::Checking),
            generation: AtomicU64::new(0),
        }
    }

    /// The state now, and every change after.
    pub fn state(&self) -> watch::Receiver<GatewaySignInState> {
        self.state.subscribe()
    }

    fn lock_credentials(&self) -> std::sync::MutexGuard<'_, GatewayCredentials> {
        // Replaced whole under the lock, so a poisoned one still holds a usable value.
        self.credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the address what its gateway wants, with the credentials it has now: a gateway state, or `Done` when the gateway lets
    /// the server through already (the session simply resumes).
    pub async fn start(&self) {
        let credentials = self.lock_credentials().clone();
        self.attempt(credentials, true).await;
    }

    /// The gateway's user name and password, in `Password`.
    ///
    /// # Errors
    /// [`GatewaySubmitError`] when nothing asks for a password or they cannot be sent. Nothing is sent then.
    pub async fn submit_password(
        &self,
        user: &str,
        password: &str,
    ) -> Result<(), GatewaySubmitError> {
        if !matches!(*self.state.borrow(), GatewaySignInState::Password { .. }) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let basic = gateway::basic(user, password)?;
        let credentials = gateway::merge(&self.lock_credentials(), Some(basic), None, None)?;
        self.attempt(credentials, true).await;
        Ok(())
    }

    /// The sign-in window's cookies, in `WebSignIn`: only those that apply to the address are kept, and the state changes only
    /// once they let the server through (or the gateway asks for something else).
    ///
    /// # Errors
    /// [`GatewaySubmitError::NotAsked`] outside `WebSignIn`.
    pub async fn submit_cookies(&self, cookies: &[CookieInput]) -> Result<(), GatewaySubmitError> {
        if !matches!(*self.state.borrow(), GatewaySignInState::WebSignIn { .. }) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let kept = gateway::cookies(&self.base, cookies);
        if kept.is_empty() {
            return Ok(());
        }
        let credentials = gateway::merge(&self.lock_credentials(), None, None, Some(kept))?;
        self.attempt(credentials, false).await;
        Ok(())
    }

    /// Extra headers (advanced), in `Password`, `WebSignIn` or `Refused`.
    ///
    /// # Errors
    /// [`GatewaySubmitError`] when nothing asks for them, none was given, or a name or value cannot be used.
    pub async fn submit_headers(&self, headers: &[HeaderInput]) -> Result<(), GatewaySubmitError> {
        if !matches!(
            *self.state.borrow(),
            GatewaySignInState::Password { .. }
                | GatewaySignInState::WebSignIn { .. }
                | GatewaySignInState::Refused { .. }
        ) {
            return Err(GatewaySubmitError::NotAsked);
        }
        let headers = gateway::headers(headers)?;
        let credentials = gateway::merge(&self.lock_credentials(), None, Some(headers), None)?;
        self.attempt(credentials, true).await;
        Ok(())
    }

    /// Stops the flow: a running attempt's result is dropped. Nothing given in it is kept.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Probes the address with `credentials` (never the token), then saves them or moves to the state the answer asks for.
    /// `show_checking`: `Checking` meanwhile (not for cookies: the window stays open).
    async fn attempt(&self, credentials: GatewayCredentials, show_checking: bool) {
        let asked_password = matches!(*self.state.borrow(), GatewaySignInState::Password { .. });
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if show_checking {
            self.state.send_replace(GatewaySignInState::Checking);
        }
        let probing = Arc::new(Gateway::new(credentials));
        let result = self
            .prober
            .probe_expecting_with(
                &self.base,
                &self.expected,
                Some(probing.clone()),
                self.plain_http_confirmed,
            )
            .await;
        let (credentials, _) = probing.snapshot();
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let next = match result {
            Ok(_) => {
                *self.lock_credentials() = credentials.clone();
                match self
                    .shared
                    .save_gateway_sign_in(&self.server, self.address, credentials)
                    .await
                {
                    Ok(Some(remembered)) => GatewaySignInState::Done { remembered },
                    Ok(None) => GatewaySignInState::Failed {
                        error: GatewaySignInError::ServerRemoved,
                    },
                    Err(error) => GatewaySignInState::Failed {
                        error: error.into(),
                    },
                }
            }
            Err(error) if gateway_answered(&error) => {
                *self.lock_credentials() = credentials;
                if !show_checking
                    && matches!(
                        error,
                        ProbeError::GatewaySignIn | ProbeError::Redirected { .. }
                    )
                {
                    // Still the sign-in page: keep waiting.
                    return;
                }
                let origin = gateway::origin(&self.base);
                let url = self.base.as_str().trim_end_matches('/').to_owned();
                match error {
                    ProbeError::GatewayPassword { realm } => GatewaySignInState::Password {
                        origin,
                        realm,
                        wrong: asked_password,
                    },
                    ProbeError::Redirected { location, .. } => GatewaySignInState::WebSignIn {
                        url,
                        origin,
                        redirected: true,
                        location,
                    },
                    ProbeError::GatewayRefused => GatewaySignInState::Refused { origin },
                    ProbeError::UnsupportedChallenge => GatewaySignInState::Unsupported { origin },
                    _ => GatewaySignInState::WebSignIn {
                        url,
                        origin,
                        redirected: false,
                        location: None,
                    },
                }
            }
            // Not reached after a web sign-in: the next navigation tries again.
            Err(_) if !show_checking => return,
            Err(error) => GatewaySignInState::Failed {
                error: error.into(),
            },
        };
        self.state.send_if_modified(|state| {
            if self.generation.load(Ordering::SeqCst) == generation {
                *state = next;
                true
            } else {
                false
            }
        });
    }
}
