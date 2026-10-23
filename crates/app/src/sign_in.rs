//! The sign-in flow: with a password or Quick Connect, to a server setup just found or one already saved (ADR 0015).
//!
//! The password is borrowed for the one request that sends it and never kept: no state holds it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use current_jellyfin::{
    AuthError, JellyfinClient, ProbeError, QUICK_CONNECT_INTERVAL, QUICK_CONNECT_LIFETIME, SignedIn,
};
use current_model::{SavedServer, ServerId, ServerRef, User};
use current_net::{AddressProvider, NetError, RoamingAddresses};
use current_store::StoreError;
use tokio::sync::{OnceCell, watch};

use crate::session::{LinkError, Shared, Target};

/// How long an attempt on a saved server waits for its address provider to select an address before it gives up.
const SELECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a sign-in failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignInError {
    /// The server refused or could not be reached.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// No address of the saved server answered as that server, but one answered as another, so no credentials were sent; or the
    /// sign-in reported another server's id ([`ProbeError::DifferentServer`]). Nothing was saved.
    #[error(transparent)]
    Probe(#[from] ProbeError),
    /// The server could not be saved in the settings, or the settings belong to a newer version of Current and cannot be saved
    /// ([`StoreError::NewerVersion`], checked before anything is sent).
    #[error(transparent)]
    Save(#[from] StoreError),
    /// The saved server was removed while signing in.
    #[error("the server was removed")]
    ServerRemoved,
}

/// Where the flow is. The UI renders one screen state per variant. No variant holds a password, token or Quick Connect secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInState {
    /// Waiting for the user to choose. `quick_connect` says whether the server offers it, once known.
    Ready {
        quick_connect: Option<bool>,
    },
    /// Talking to the server.
    SigningIn,
    /// The user enters `code` in another client that is signed in to the server; the flow waits for the approval.
    QuickConnectWaiting {
        code: String,
    },
    Failed {
        error: SignInError,
    },
    /// Signed in and saved. `remembered` is false when there is no keyring: the session lasts for this run only (ADR 0013).
    SignedIn {
        server: ServerRef,
        user: User,
        remembered: bool,
    },
}

/// The flow's state and actions. Starting a sign-in, [`SignIn::cancel`] or dropping the future of a running sign-in stops it:
/// Quick Connect stops polling and a superseded result is dropped.
pub struct SignIn {
    shared: Arc<Shared>,
    target: Target,
    /// The client the attempts use. For a saved server it is made on the first attempt, on the runtime, over the server's address
    /// provider.
    link: OnceCell<Link>,
    /// The server's id: the sign-in must report this one.
    expected: ServerId,
    state: watch::Sender<SignInState>,
    generation: watch::Sender<u64>,
    /// The attempt whose accepted sign-in is being saved. From then on [`SignIn::cancel`] cannot undo it: the server made the
    /// session, so the attempt ends signed in. Changed only under the `generation` lock.
    saving: Mutex<Option<u64>>,
    quick_connect: Mutex<Option<bool>>,
    quick_connect_interval: Duration,
    quick_connect_lifetime: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Each value behind these locks is replaced whole, so a poisoned lock still holds a usable one.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

/// A sign-in's client and, for a saved server, the address provider it goes through.
struct Link {
    client: JellyfinClient,
    /// `None` for a server setup has just found: its one address was probed by setup and sits in a `FixedAddress`.
    provider: Option<Arc<RoamingAddresses>>,
}

impl SignIn {
    /// A sign-in to a server setup has just found, through `client` on the address setup probed.
    pub(crate) fn new(
        shared: Arc<Shared>,
        target: Target,
        client: JellyfinClient,
        expected: ServerId,
    ) -> Self {
        let link = Link {
            client,
            provider: None,
        };
        Self::with(shared, target, OnceCell::new_with(Some(link)), expected)
    }

    /// A sign-in to a saved server, through its address provider (made on the first attempt).
    pub(crate) fn saved(shared: Arc<Shared>, saved: SavedServer) -> Self {
        Self::with(
            shared,
            Target::Saved(saved.server),
            OnceCell::new(),
            saved.id,
        )
    }

    fn with(shared: Arc<Shared>, target: Target, link: OnceCell<Link>, expected: ServerId) -> Self {
        Self {
            shared,
            target,
            link,
            expected,
            state: watch::Sender::new(SignInState::Ready {
                quick_connect: None,
            }),
            generation: watch::Sender::new(0),
            saving: Mutex::new(None),
            quick_connect: Mutex::new(None),
            quick_connect_interval: QUICK_CONNECT_INTERVAL,
            quick_connect_lifetime: QUICK_CONNECT_LIFETIME,
        }
    }

    /// Changes how often Quick Connect asks for the approval and how long it waits. The defaults follow Jellyfin: every 5 s, for
    /// the 10 minutes the server keeps a request.
    pub fn set_quick_connect_timing(&mut self, interval: Duration, lifetime: Duration) {
        self.quick_connect_interval = interval;
        self.quick_connect_lifetime = lifetime;
    }

    /// The state now, and every change after.
    pub fn state(&self) -> watch::Receiver<SignInState> {
        self.state.subscribe()
    }

    /// Asks the server whether Quick Connect is on, and shows it in `Ready`. An error leaves it unknown.
    pub async fn check_quick_connect(&self) {
        let Ok(link) = self.link().await else {
            return;
        };
        let Ok(enabled) = link.client.quick_connect_enabled().await else {
            return;
        };
        *self
            .quick_connect
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(enabled);
        self.state.send_if_modified(|state| match state {
            SignInState::Ready { quick_connect } if *quick_connect != Some(enabled) => {
                *quick_connect = Some(enabled);
                true
            }
            _ => false,
        });
    }

    /// Signs in with a user name and password. The password is used for this one request and not kept.
    pub async fn with_password(&self, username: &str, password: &str) {
        let generation = self.begin();
        let Some(client) = self.prepare(generation).await else {
            return;
        };
        let result = self
            .unless_cancelled(generation, client.sign_in_by_name(username, password))
            .await;
        if let Some(result) = result {
            self.finish(generation, result).await;
        }
    }

    /// Signs in with Quick Connect: shows a code, waits until a user approves it in another client (polling every 5 s, for at
    /// most 10 minutes), then signs in.
    pub async fn with_quick_connect(&self) {
        let generation = self.begin();
        let Some(client) = self.prepare(generation).await else {
            return;
        };
        let Some(code) = self
            .unless_cancelled(generation, client.quick_connect_initiate())
            .await
        else {
            return;
        };
        let code = match code {
            Ok(code) => code,
            Err(error) => {
                self.finish(generation, Err(error)).await;
                return;
            }
        };
        self.set_if_current(
            generation,
            SignInState::QuickConnectWaiting {
                code: code.code.clone(),
            },
        );
        let wait = client.quick_connect_wait(
            &code.secret,
            self.quick_connect_interval,
            self.quick_connect_lifetime,
        );
        if let Some(result) = self.unless_cancelled(generation, wait).await {
            self.finish(generation, result).await;
        }
    }

    /// Stops a running sign-in and goes back to `Ready`. A finished sign-in stays saved, and so does one the server already
    /// accepted that is being saved: the attempt then ends `SignedIn`, as if the cancel came too late.
    pub fn cancel(&self) {
        let cancelled = self.generation.send_if_modified(|generation| {
            if *lock(&self.saving) == Some(*generation) {
                false
            } else {
                *generation += 1;
                true
            }
        });
        if !cancelled {
            return;
        }
        let quick_connect = *self
            .quick_connect
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.state.send_if_modified(|state| {
            if matches!(state, SignInState::SignedIn { .. }) {
                return false;
            }
            let next = SignInState::Ready { quick_connect };
            let changed = *state != next;
            *state = next;
            changed
        });
    }

    /// A new attempt: earlier ones are superseded.
    fn begin(&self) -> u64 {
        let mut current = 0;
        self.generation.send_modify(|generation| {
            *generation += 1;
            current = *generation;
        });
        self.state.send_replace(SignInState::SigningIn);
        current
    }

    /// The attempts' client, made on first use for a saved server.
    async fn link(&self) -> Result<&Link, SignInError> {
        self.link
            .get_or_try_init(|| async {
                let Target::Saved(server) = &self.target else {
                    // A new server's link is made with the flow.
                    return Err(SignInError::ServerRemoved);
                };
                let (client, provider) =
                    self.shared
                        .saved_client(server)
                        .await
                        .map_err(|error| match error {
                            LinkError::Removed => SignInError::ServerRemoved,
                            error => SignInError::Auth(AuthError::Network(error.into_net())),
                        })?;
                Ok(Link {
                    client,
                    provider: Some(provider),
                })
            })
            .await
    }

    /// Checks before anything is sent that the result can be saved and, for a saved server, waits until its provider has selected
    /// an address that answers as that server (ADR 0014). Returns the client to go on with; on a failure the state says why.
    ///
    /// The wait lets an attempt fail early with a clear error. Credentials are safe without it: the provider serves no address
    /// before it passed the identity check, so the credentialed request would wait for that check anyway, also when the network
    /// changes after this returns.
    async fn prepare(&self, generation: u64) -> Option<&JellyfinClient> {
        let failed = |error: SignInError| {
            self.set_if_current(generation, SignInState::Failed { error });
            None
        };
        if let Err(error) = self.shared.check_writable() {
            return failed(error.into());
        }
        let link = match self.unless_cancelled(generation, self.link()).await? {
            Ok(link) => link,
            Err(error) => return failed(error),
        };
        let Some(provider) = &link.provider else {
            return Some(&link.client);
        };
        // The user asked to sign in: an unreachable server tries again now rather than at its next retry.
        provider.retry_now();
        let deadline = tokio::time::Instant::now() + SELECT_TIMEOUT;
        match self
            .unless_cancelled(generation, provider.wait(deadline))
            .await?
        {
            Some(_) => Some(&link.client),
            None if !provider.status().borrow().different_server.is_empty() => {
                failed(ProbeError::DifferentServer.into())
            }
            None => failed(AuthError::Network(NetError::Unreachable).into()),
        }
    }

    /// Runs `work` until it ends or the attempt is superseded (`None`).
    async fn unless_cancelled<T>(
        &self,
        generation: u64,
        work: impl Future<Output = T>,
    ) -> Option<T> {
        let mut current = self.generation.subscribe();
        tokio::select! {
            result = work => Some(result),
            _ = current.wait_for(|g| *g != generation) => None,
        }
    }

    fn set_if_current(&self, generation: u64, next: SignInState) {
        self.state.send_if_modified(|state| {
            if *self.generation.borrow() == generation {
                *state = next;
                true
            } else {
                false
            }
        });
    }

    async fn finish(&self, generation: u64, result: Result<SignedIn, AuthError>) {
        let signed_in = match result {
            Ok(signed_in) => signed_in,
            Err(error) => {
                self.set_if_current(
                    generation,
                    SignInState::Failed {
                        error: error.into(),
                    },
                );
                return;
            }
        };
        // A sign-in that reports another server's id is not saved (and its token not kept).
        if signed_in
            .server_id
            .as_ref()
            .is_some_and(|id| *id != self.expected)
        {
            self.set_if_current(
                generation,
                SignInState::Failed {
                    error: ProbeError::DifferentServer.into(),
                },
            );
            return;
        }
        // Claimed under the generation lock, so that a cancel either comes first (and this attempt is not saved) or finds it
        // saving (and leaves it alone).
        let mut claimed = false;
        self.generation.send_if_modified(|current| {
            if *current == generation {
                *lock(&self.saving) = Some(generation);
                claimed = true;
            }
            false
        });
        if !claimed {
            return;
        }
        let user = signed_in.user.clone();
        // Saved in full even if this future is dropped or the attempt cancelled meanwhile: the server already made the session.
        let next = match self
            .shared
            .save_sign_in(self.target.clone(), signed_in)
            .await
        {
            Ok((server, remembered)) => SignInState::SignedIn {
                user: User {
                    server: server.clone(),
                    ..user
                },
                server,
                remembered,
            },
            Err(error) => SignInState::Failed { error },
        };
        self.generation.send_if_modified(|_| {
            *lock(&self.saving) = None;
            false
        });
        self.set_if_current(generation, next);
    }
}
