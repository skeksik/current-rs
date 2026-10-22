//! The sign-in flow: with a password or Quick Connect, to a server setup just found or one already saved (ADR 0015).
//!
//! The password is borrowed for the one request that sends it and never kept: no state holds it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use current_jellyfin::{
    AuthError, JellyfinClient, QUICK_CONNECT_INTERVAL, QUICK_CONNECT_LIFETIME, SignedIn,
};
use current_model::{ServerRef, User};
use current_store::StoreError;
use tokio::sync::watch;

use crate::session::{Shared, Target};

/// Why a sign-in failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignInError {
    /// The server refused or could not be reached.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// Signed in, but the server could not be saved in the settings.
    #[error(transparent)]
    Save(#[from] StoreError),
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
    client: JellyfinClient,
    state: watch::Sender<SignInState>,
    generation: watch::Sender<u64>,
    quick_connect: Mutex<Option<bool>>,
    quick_connect_interval: Duration,
    quick_connect_lifetime: Duration,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

impl SignIn {
    pub(crate) fn new(shared: Arc<Shared>, target: Target, client: JellyfinClient) -> Self {
        Self {
            shared,
            target,
            client,
            state: watch::Sender::new(SignInState::Ready {
                quick_connect: None,
            }),
            generation: watch::Sender::new(0),
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
        let Ok(enabled) = self.client.quick_connect_enabled().await else {
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
        let result = self
            .unless_cancelled(generation, self.client.sign_in_by_name(username, password))
            .await;
        if let Some(result) = result {
            self.finish(generation, result).await;
        }
    }

    /// Signs in with Quick Connect: shows a code, waits until a user approves it in another client (polling every 5 s, for at
    /// most 10 minutes), then signs in.
    pub async fn with_quick_connect(&self) {
        let generation = self.begin();
        let Some(code) = self
            .unless_cancelled(generation, self.client.quick_connect_initiate())
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
        let wait = self.client.quick_connect_wait(
            &code.secret,
            self.quick_connect_interval,
            self.quick_connect_lifetime,
        );
        if let Some(result) = self.unless_cancelled(generation, wait).await {
            self.finish(generation, result).await;
        }
    }

    /// Stops a running sign-in and goes back to `Ready`. A finished sign-in stays saved.
    pub fn cancel(&self) {
        self.generation.send_modify(|generation| *generation += 1);
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
        if *self.generation.borrow() != generation {
            return;
        }
        let user = signed_in.user.clone();
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
            Err(error) => SignInState::Failed {
                error: error.into(),
            },
        };
        // Saved even if cancelled meanwhile: the server already made the session.
        self.state.send_replace(next);
    }
}
